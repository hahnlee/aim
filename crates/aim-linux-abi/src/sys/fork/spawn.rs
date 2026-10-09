//! A guest fork as a fresh `linux-run` (`docs/fork.md`).
//!
//! A Darwin `fork()` without exec leaves the child unable to reach XPC
//! services (launchd refuses its lookups), so Metal cannot compile shaders
//! there, among others. A guest fork is therefore made the way Cygwin makes
//! one on Windows: the parent snapshots its guest memory and layer state,
//! spawns `linux-run --fork-child`, and the child maps the memory at the
//! same addresses, rebuilds the state and resumes the forking thread's
//! registers with 0 in x0. The child is the parent's real Darwin child, so
//! waitpid, pidfds and SIGCHLD work as for any child.
//!
//! - **Memory.** Every VM entry of the guest range (`arena`) becomes a Mach
//!   memory entry: a copy-on-write copy (`MAP_MEM_VM_COPY`) of private
//!   memory, the memory itself (`MAP_MEM_VM_SHARE`) for shared mappings
//!   (MAP_SHARED files and memory, memfds, ashmem, binder buffers), and
//!   nothing for memory never touched, which the child allocates afresh.
//!   The copies are taken before the parent returns, so what it writes
//!   afterwards stays its own.
//! - **Handover.** The parent gives the child a send right to a port of its
//!   own (the child's registered ports) and serves it on a host thread, so
//!   `fork` returns without waiting for the child to start: the child sends
//!   its reply port, and gets the memory entries and the state blob
//!   (`state`) in one message. When the child dies first, the no-senders
//!   notification ends the wait. A parent that exits or execs first waits
//!   for its handovers ([`wait_handovers`]), since the child cannot start
//!   without it.
//! - **Descriptors** are inherited by `posix_spawn` (all but kqueues, which
//!   Darwin cannot pass; the layer rebuilds its epoll, inotify and pidfd
//!   kqueues from its state), and their close-on-exec flags set again.
//! - **The thread.** Its registers, thread pointer and shadow call stack (a
//!   host mapping, copied) go in the blob.

use std::ffi::CString;
use std::sync::{Condvar, Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use super::state::{self, Reader, Writer};
use crate::context::{self, GuestContext};
use crate::errno::{self, EAGAIN, ENOMEM};
use crate::sys::{arena, fdtab, vmmap, window};

type Port = u32;
type Kr = i32;

const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_MSG_TYPE_MOVE_SEND: u32 = 17;
const MACH_MSG_TYPE_MOVE_SEND_ONCE: u32 = 18;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const MACH_MSG_TYPE_MAKE_SEND_ONCE: u32 = 21;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_SEND_MSG: i32 = 1;
const MACH_RCV_MSG: i32 = 2;
const MACH_RCV_TIMEOUT: i32 = 0x100;
const MACH_MSG_OOL_DESCRIPTOR: u8 = 1;
const MACH_MSG_OOL_PORTS_DESCRIPTOR: u8 = 2;
const MACH_MSG_VIRTUAL_COPY: u8 = 1;
const MACH_NOTIFY_NO_SENDERS: i32 = 0o106;
const MAP_MEM_VM_COPY: i32 = 0x20_0000;
const MAP_MEM_VM_SHARE: i32 = 0x40_0000;
const VM_PROT_READ: i32 = 1;
const VM_PROT_EXECUTE: i32 = 4;
const VM_PROT_ALL: i32 = 7;
const VM_FLAGS_FIXED: i32 = 0;
const VM_FLAGS_ANYWHERE: i32 = 1;
const VM_FLAGS_OVERWRITE: i32 = 0x4000;
const VM_INHERIT_SHARE: u32 = 0;
const VM_INHERIT_COPY: u32 = 1;

/// The id of the child's request message.
const HELLO:i32=0x666f_726b;
const READY:i32=0x666f_726c;
const READY_ACK:i32=0x666f_726d;
#[repr(C,packed(4))]
#[derive(Default)]
struct ReadyAck{header:Header,error:i32}
#[repr(C,packed(4))]
#[derive(Default)]
struct Ready{header:Header,member:u64}
/// How long the child has to ask for its state.
const HANDOVER_TIMEOUT_MS: u32 = 60_000;

unsafe extern "C" {
    static mach_task_self_: Port;
    fn mach_port_allocate(task: Port, right: u32, name: *mut Port) -> Kr;
    fn mach_port_insert_right(task: Port, name: Port, poly: Port, kind: u32) -> Kr;
    fn mach_port_deallocate(task: Port, name: Port) -> Kr;
    fn mach_port_mod_refs(task: Port, name: Port, right: u32, delta: i32) -> Kr;
    fn mach_port_request_notification(
        task: Port,
        name: Port,
        id: i32,
        sync: u32,
        notify: Port,
        kind: u32,
        previous: *mut Port,
    ) -> Kr;
    fn mach_msg(
        msg: *mut Header,
        option: i32,
        send_size: u32,
        rcv_size: u32,
        rcv_name: Port,
        timeout: u32,
        notify: Port,
    ) -> Kr;
    fn mach_ports_lookup(task: Port, ports: *mut *mut Port, count: *mut u32) -> Kr;
    fn mach_ports_register(task: Port, ports: *mut Port, count: u32) -> Kr;
    fn mach_make_memory_entry_64(
        task: Port,
        size: *mut u64,
        offset: u64,
        permission: i32,
        handle: *mut Port,
        parent: Port,
    ) -> Kr;
    fn mach_vm_map(
        task: Port,
        address: *mut u64,
        size: u64,
        mask: u64,
        flags: i32,
        object: Port,
        offset: u64,
        copy: i32,
        cur: i32,
        max: i32,
        inheritance: u32,
    ) -> Kr;
    fn mach_vm_allocate(task: Port, address: *mut u64, size: u64, flags: i32) -> Kr;
    fn mach_vm_protect(task: Port, address: u64, size: u64, set_max: i32, prot: i32) -> Kr;
    fn mach_vm_deallocate(task: Port, address: u64, size: u64) -> Kr;
    fn posix_spawnattr_set_registered_ports_np(
        attr: *mut libc::posix_spawnattr_t,
        ports: *mut Port,
        count: u32,
    ) -> i32;
    fn posix_spawn_file_actions_addinherit_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        fd: i32,
    ) -> i32;
    fn _NSGetEnviron() -> *const *const *const libc::c_char;
}

fn task() -> Port {
    // SAFETY: reading the task port global.
    unsafe { mach_task_self_ }
}

#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
struct Header {
    bits: u32,
    size: u32,
    remote: Port,
    local: Port,
    voucher: Port,
    id: i32,
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct OolPorts {
    address: u64,
    deallocate: u8,
    copy: u8,
    disposition: u8,
    kind: u8,
    count: u32,
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Ool {
    address: u64,
    deallocate: u8,
    copy: u8,
    pad: u8,
    kind: u8,
    size: u32,
}

/// The handover message: the memory entries and the state blob.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Payload {
    header: Header,
    descriptors: u32,
    entries: OolPorts,
    blob: Ool,
}

#[repr(C, packed(4))]
struct Received<T> {
    msg: T,
    trailer: [u8; 128],
}

// ---- memory -----------------------------------------------------------------------

/// How the child gets one VM entry of the parent's guest range.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backing {
    /// A copy-on-write copy (memory entry).
    Copy,
    /// The same memory (memory entry).
    Share,
    /// Fresh zero-filled memory: the entry was never touched.
    Fresh,
}

struct Region {
    start: u64,
    len: u64,
    prot: i32,
    max_prot: i32,
    backing: Backing,
}

/// Memory entries of every guest mapping, with their table. The ports are
/// ours until they are sent.
struct Snapshot {
    regions: Vec<Region>,
    entries: Vec<Port>,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for &e in &self.entries {
            // SAFETY: our send rights to the memory entries.
            unsafe { mach_port_deallocate(task(), e) };
        }
    }
}

fn snapshot() -> Result<Snapshot, i64> {
    let mut s = Snapshot {
        regions: Vec::new(),
        entries: Vec::new(),
    };
    let mut at = arena::LO;
    while let Some(i) = vmmap::info_at(at) {
        if i.start >= arena::HI {
            break;
        }
        at = i.end;
        let start = i.start.max(arena::LO);
        let len = i.end.min(arena::HI) - start;
        if i.tag == window::TAG {
            // Reserved heap window pages: the child reserves its own.
            continue;
        }
        let (prot, max_prot) = (i.prot as i32 & VM_PROT_ALL, i.max_prot as i32 & VM_PROT_ALL);
        // The string pages are the process's stack, mapped from its record.
        let backing = if i.shared && !super::super::procrec::is_strings(start) {
            Backing::Share
        } else if i.empty {
            Backing::Fresh
        } else {
            Backing::Copy
        };
        let max_prot = if backing == Backing::Fresh {
            max_prot
        } else {
            let (port, max_prot) = entry(start, len, prot, max_prot, backing)?;
            s.entries.push(port);
            max_prot
        };
        s.regions.push(Region {
            start,
            len,
            prot,
            max_prot,
            backing,
        });
    }
    Ok(s)
}

/// A memory entry for `[start, start+len)`, with the maximum protection
/// the child's mapping of it may have.
///
/// A copy may become anything. The memory itself allows what its mapping
/// here allows at most, but Darwin refuses some of those for an entry (a
/// file mapped shared refuses the execute permission it may be given with
/// mprotect), so less is asked for until the protection it has now.
fn entry(
    start: u64,
    len: u64,
    prot: i32,
    max_prot: i32,
    backing: Backing,
) -> Result<(Port, i32), i64> {
    let perms: &[i32] = match backing {
        Backing::Share => &[max_prot, max_prot & !VM_PROT_EXECUTE, prot],
        _ => &[VM_PROT_ALL],
    };
    let flag = match backing {
        Backing::Share => MAP_MEM_VM_SHARE,
        _ => MAP_MEM_VM_COPY,
    };
    // A copy needs to read the pages.
    let opened = backing == Backing::Copy && prot & VM_PROT_READ == 0;
    // SAFETY: changing the protection of our own guest memory, restored
    // below.
    if opened && unsafe { mach_vm_protect(task(), start, len, 0, prot | VM_PROT_READ) } != 0 {
        return Err(-(ENOMEM as i64));
    }
    let (mut size, mut port, mut kr, mut max) = (len, 0, -1, 0);
    for &perm in perms.iter().filter(|&&p| p & prot == prot) {
        size = len;
        max = perm;
        // SAFETY: a memory entry for our own mapping.
        kr = unsafe {
            mach_make_memory_entry_64(task(), &mut size, start, perm | flag, &mut port, 0)
        };
        if kr == 0 {
            break;
        }
    }
    if opened {
        // SAFETY: as above.
        unsafe { mach_vm_protect(task(), start, len, 0, prot) };
    }
    if kr != 0 || size < len {
        crate::diag!(
            "[linux-abi] fork: no memory entry for {start:#x}..{:#x} (kr {kr:#x})",
            start + len
        );
        if kr == 0 {
            // SAFETY: the entry we just made.
            unsafe { mach_port_deallocate(task(), port) };
        }
        return Err(-(ENOMEM as i64));
    }
    Ok((port, max))
}

fn save_regions(w: &mut Writer, regions: &[Region]) {
    w.seq(regions.iter(), |w, r| {
        w.u64(r.start);
        w.u64(r.len);
        w.i32(r.prot);
        w.i32(r.max_prot);
        w.u32(r.backing as u32);
    });
}

/// Map the parent's guest memory at its addresses.
fn map_regions(r: &mut Reader, entries: &[Port]) -> Result<(), String> {
    let regions = r.seq(|r| Region {
        start: r.u64(),
        len: r.u64(),
        prot: r.i32(),
        max_prot: r.i32(),
        backing: match r.u32() {
            0 => Backing::Copy,
            1 => Backing::Share,
            _ => Backing::Fresh,
        },
    });
    let mut entries = entries.iter();
    for g in regions {
        // Only the heap window is mapped (reserved) here already.
        let in_window = (window::BASE..window::BASE + window::SIZE).contains(&g.start);
        let flags = VM_FLAGS_FIXED | if in_window { VM_FLAGS_OVERWRITE } else { 0 };
        if g.prot == VM_PROT_ALL {
            let src = if g.backing == Backing::Fresh {
                None
            } else {
                let Some(&e) = entries.next() else {
                    return Err("fork: fewer memory entries than regions".into());
                };
                Some(e)
            };
            map_jit(g.start, g.len, src)?;
            continue;
        }
        let mut addr = g.start;
        // SAFETY (all arms): mapping the parent's memory into our own guest
        // range, which nothing else of ours uses.
        let kr = unsafe {
            match g.backing {
                Backing::Fresh => {
                    let kr = mach_vm_allocate(task(), &mut addr, g.len, flags);
                    if kr == 0 {
                        mach_vm_protect(task(), addr, g.len, 0, g.prot);
                    }
                    kr
                }
                b => {
                    let Some(&e) = entries.next() else {
                        return Err("fork: fewer memory entries than regions".into());
                    };
                    let inherit = match b {
                        Backing::Share => VM_INHERIT_SHARE,
                        _ => VM_INHERIT_COPY,
                    };
                    map_entry(&mut addr, g.len, flags, e, g.prot, g.max_prot, inherit)
                }
            }
        };
        if kr != 0 {
            return Err(format!(
                "fork: cannot map {:#x}..{:#x} (kr {kr:#x})",
                g.start,
                g.start + g.len
            ));
        }
    }
    Ok(())
}

/// Map memory entry `e` at `addr` with protection `prot`. Execute is
/// added after the mapping: a page faulted through an entry mapped
/// executable is taken from every other mapping of it on each fault, so
/// the processes sharing a library's code would refault it endlessly
/// (#444). Code made executable with mprotect has no such cost.
fn map_entry(
    addr: &mut u64,
    len: u64,
    flags: i32,
    e: Port,
    prot: i32,
    max: i32,
    inherit: u32,
) -> Kr {
    // SAFETY: the caller owns the target range.
    unsafe {
        let kr = mach_vm_map(
            task(),
            addr,
            len,
            0,
            flags,
            e,
            0,
            0,
            prot & !VM_PROT_EXECUTE,
            max,
            inherit,
        );
        if kr == 0 && prot & VM_PROT_EXECUTE != 0 {
            mach_vm_protect(task(), *addr, len, 0, prot)
        } else {
            kr
        }
    }
}

/// JIT memory (RWX, `jit`): a fresh `MAP_JIT` mapping at the parent's
/// address, filled from the parent's copy `src`.
fn map_jit(start: u64, len: u64, src: Option<Port>) -> Result<(), String> {
    let mut tmp = 0;
    if let Some(e) = src {
        // SAFETY: mapping the parent's copy anywhere, to read it.
        let kr = unsafe {
            mach_vm_map(
                task(),
                &mut tmp,
                len,
                0,
                VM_FLAGS_ANYWHERE,
                e,
                0,
                0,
                VM_PROT_READ,
                VM_PROT_READ,
                VM_INHERIT_COPY,
            )
        };
        if kr != 0 {
            return Err(format!(
                "fork: cannot read JIT memory at {start:#x} (kr {kr:#x})"
            ));
        }
    }
    let r = crate::sys::jit::map_at(start, len, (tmp != 0).then_some(tmp as *const u8));
    if tmp != 0 {
        // SAFETY: the view mapped above.
        unsafe { mach_vm_deallocate(task(), tmp, len) };
    }
    r.map_err(|e| format!("fork: cannot map JIT memory at {start:#x} ({e})"))
}

// ---- descriptors --------------------------------------------------------------------

/// Native operations may hold process-owned lock descriptions while a fork
/// captures its explicit owner receipts. Keep those operations out of that
/// capture; unrelated host descriptors are never selected for inheritance.
static OWN_FDS: RwLock<()> = RwLock::new(());

/// Keep forks out while the layer has a locked fd of its own open.
pub fn own_fds() -> RwLockReadGuard<'static, ()> {
    OWN_FDS.read().unwrap_or_else(|e| e.into_inner())
}

// ---- the parent ---------------------------------------------------------------------

/// What the forking `clone` asked for, as the child applies it.
pub struct ChildSetup {
    pub new_mount_namespace:bool,
    /// New stack pointer (0: the parent's).
    pub stack: u64,
    /// CLONE_SETTLS's thread pointer.
    pub tls: Option<u64>,
    /// CLONE_CHILD_SETTID's word.
    pub set_tid: u64,
    /// CLONE_CHILD_CLEARTID's word.
    pub clear_tid: u64,
    /// A vfork parent's wait pipe (read end, write end): the child keeps
    /// the write end until it execs or exits.
    pub vfork: Option<(i32, i32)>,
    /// The tracer and options the child starts traced with (a clone event).
    pub traced: Option<(i32, u64)>,
    /// For an own-files thread: the process it is a thread of.
    pub thread_of: Option<i32>,
}

fn save_context(w: &mut Writer, ctx: &GuestContext) {
    for v in ctx.x {
        w.u64(v);
    }
    for v in [ctx.sp, ctx.pc, ctx.nzcv, ctx.fpcr, ctx.fpsr, ctx.stub_ret] {
        w.u64(v);
    }
    for v in ctx.v {
        w.u64(v as u64);
        w.u64((v >> 64) as u64);
    }
}

fn load_context(r: &mut Reader, ctx: &mut GuestContext) {
    for v in &mut ctx.x {
        *v = r.u64();
    }
    for v in [
        &mut ctx.sp,
        &mut ctx.pc,
        &mut ctx.nzcv,
        &mut ctx.fpcr,
        &mut ctx.fpsr,
        &mut ctx.stub_ret,
    ] {
        *v = r.u64();
    }
    for v in &mut ctx.v {
        *v = r.u64() as u128 | (r.u64() as u128) << 64;
    }
}

/// The used part of the calling thread's shadow call stack (a mapping of
/// its own between guard pages, filled upwards).
fn shadow_stack() -> Vec<u8> {
    let scs = context::guest_scs();
    match vmmap::info_at(scs) {
        // SAFETY: the used part of this thread's shadow stack.
        Some(i) if i.start <= scs && scs < i.end => unsafe {
            std::slice::from_raw_parts(i.start as *const u8, (scs - i.start) as usize).to_vec()
        },
        _ => Vec::new(),
    }
}

fn save_vfork(w:&mut Writer,pipe:Option<(i32,i32)>){w.opt(pipe,|w,(rd,wr)|{w.retain_private(rd);w.retain_private(wr);w.i32(rd);w.i32(wr)});}
/// Fork the calling guest thread's process; the child's pid.
pub fn fork(ctx: &GuestContext, setup: &ChildSetup, runtime: &[CString]) -> Result<i32, i64> {
    let exe = super::super::exec::launch_exe().ok_or(-(libc::ENOEXEC as i64))?;
    let mount_text=crate::vfs::fork_mounts_text(setup.new_mount_namespace).map_err(|error|-(error as i64))?;
    let mut w = Writer::default();
    w.u64(setup.stack);
    w.opt(setup.tls, |w, v| w.u64(v));
    w.u64(setup.set_tid);
    w.u64(setup.clear_tid);
    save_vfork(&mut w,setup.vfork);
    w.opt(setup.traced, |w, (tracer, options)| {
        w.i32(tracer);
        w.u64(options);
    });
    w.opt(setup.thread_of, |w, p| w.i32(p));
    save_context(&mut w, ctx);
    w.u64(context::guest_tp());
    w.bytes(&shadow_stack());
    w.str(&crate::vfs::cwd());
    w.str(&mount_text);
    let tracked=super::super::verity_pager::tracked_identities().map_err(|error|-(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))as i64))?;
    let identity_count=tracked.len();
    let fork_lease=crate::verity_client().map(|client|client.fork_lease(tracked)).transpose().map_err(|error|{crate::diag!("[linux-abi] fork mapping admission ({identity_count} identities): {error}");-(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))as i64)})?;
    let pager=super::super::verity_pager::ForkSnapshot::capture().map_err(|error|-(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))as i64))?;
    let snap = snapshot()?;
    save_regions(&mut w, &snap.regions);
    let own=OWN_FDS.write().unwrap_or_else(|e|e.into_inner());
    let fd_guard=fdtab::lifecycle();
    let writer_receipts=fdtab::fork_writer_receipts().map_err(|error|-(error as i64))?;
    let(fds,guest_receipts)=fdtab::fork_guest_snapshot().map_err(|error|-(error as i64))?;
    state::save(&mut w,&mount_text);
    let diag=crate::diag::log_fd();if !fds.iter().any(|(fd,_)|*fd==diag){w.retain_private(diag);}
    let private_receipts=w.take_private().map_err(|error|-(error as i64))?;
    pager.write(&mut w).map_err(|error|-(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))as i64))?;
    let mut transfers=pager.private_fds();
    {use std::os::fd::AsRawFd;
     for receipt in writer_receipts.iter().chain(private_receipts.iter()).chain(guest_receipts.iter()){
      if !transfers.iter().any(|(_,target)|*target==receipt.target()){transfers.push((receipt.source().as_raw_fd(),receipt.target()));}
     }
    }
    let port = new_port()?;
    let spawned=spawn(exe,runtime,&fds,&transfers,port).map(|pid|(pid,fds));
    drop(fd_guard);drop(own);

    let pid = match spawned {
        Ok((pid, fds)) => {
            w.seq(fds.iter(), |w, (fd, cloexec)| {
                w.i32(*fd);
                w.bool(*cloexec);
            });
            if let Err(error)=crate::vfs::publish_fork_mounts(pid,&mount_text){
                unsafe{libc::kill(pid,libc::SIGKILL);while libc::waitpid(pid,std::ptr::null_mut(),0)<0&&errno::last()==errno::EINTR{}}
                return Err(-(error as i64));
            }
            // Before the handover: the child keeps this entry.
            crate::sys::cred::note_child(pid);
            if unsafe{libc::kill(pid,libc::SIGCONT)}!=0{
                let error=errno::last();unsafe{libc::kill(pid,libc::SIGKILL);while libc::waitpid(pid,std::ptr::null_mut(),0)<0&&errno::last()==errno::EINTR{}}return Err(-(error as i64));
            }
            pid
        }
        Err(e) => {
            // SAFETY: our port's send and receive rights.
            unsafe {
                mach_port_deallocate(task(), port);
                mach_port_mod_refs(task(), port, MACH_PORT_RIGHT_RECEIVE, -1);
            }
            return Err(e);
        }
    };
    // Only the child holds a send right now: its exit is a no-senders
    // notification.
    // SAFETY: our port; the notification comes to the port itself.
    unsafe {
        mach_port_deallocate(task(), port);
        let mut prev = 0;
        mach_port_request_notification(
            task(),
            port,
            MACH_NOTIFY_NO_SENDERS,
            0,
            port,
            MACH_MSG_TYPE_MAKE_SEND_ONCE,
            &mut prev,
        );
    }
    let blob = w.into_bytes();
    *lock(&HANDOVERS.0) += 1;
    let started = std::thread::Builder::new()
        .name("fork-handover".into())
        .spawn(move || serve(port,snap,blob,fork_lease));
    if let Err(e) = started {
        // The child finds no parent port and ends.
        crate::diag!("[linux-abi] fork: no handover thread: {e}");
        // SAFETY: dropping our receive right.
        unsafe { mach_port_mod_refs(task(), port, MACH_PORT_RIGHT_RECEIVE, -1) };
        handover_done();
    }
    Ok(pid)
}

/// Handovers in progress, and a wakeup when one ends.
static HANDOVERS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn handover_done() {
    *lock(&HANDOVERS.0) -= 1;
    HANDOVERS.1.notify_all();
}

/// Before this process ends or execs: let the children it forked take
/// their state first (each within the handover timeout).
pub fn wait_handovers() {
    let deadline = Instant::now() + Duration::from_millis(HANDOVER_TIMEOUT_MS as u64);
    let mut n = lock(&HANDOVERS.0);
    while *n > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        n = HANDOVERS
            .1
            .wait_timeout(n, left)
            .map_or_else(|e| e.into_inner().0, |r| r.0);
    }
}

fn new_port() -> Result<Port, i64> {
    let mut port = 0;
    // SAFETY: a fresh receive right with one send right.
    unsafe {
        if mach_port_allocate(task(), MACH_PORT_RIGHT_RECEIVE, &mut port) != 0 {
            return Err(-(EAGAIN as i64));
        }
        if mach_port_insert_right(task(), port, port, MACH_MSG_TYPE_MAKE_SEND) != 0 {
            mach_port_mod_refs(task(), port, MACH_PORT_RIGHT_RECEIVE, -1);
            return Err(-(EAGAIN as i64));
        }
    }
    Ok(port)
}

/// `posix_spawn` `linux-run --fork-child KQUEUES` with the fds and `port`.
/// KQUEUES are the fd numbers of the guest's kqueues, which the child holds
/// with placeholders from its start until it has rebuilt them.
fn spawn(exe: &CString, runtime: &[CString], fds: &[(i32, bool)],private_fds:&[(i32,i32)], port: Port) -> Result<i32, i64> {
    let mut kqueues: Vec<i32> = fdtab::kqueue_fds();
    kqueues.extend(super::super::wait::pidfd_fds());
    let kqueues=reservation_text(&kqueues,fds,private_fds)?;
    let mut argv: Vec<*const libc::c_char> = vec![exe.as_ptr()];
    argv.extend(runtime.iter().map(|a| a.as_ptr()));
    let fork_child = c"--fork-child";
    argv.extend([fork_child.as_ptr(), kqueues.as_ptr(), std::ptr::null()]);
    spawn_program_suspended(exe,&argv,private_fds,port,true)
}
fn reservation_text(kqueues:&[i32],fds:&[(i32,bool)],transfers:&[(i32,i32)])->Result<CString,i64>{
    validate_transfers(transfers)?;
    let queues=kqueues.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    let relocations=transfers.iter().filter(|(_,target)|*target>=10240).map(|(source,target)|format!("{source}:{target}")).collect::<Vec<_>>().join(",");
    let private=transfers.iter().filter(|(_,target)|!fds.iter().any(|(fd,_)|fd==target)).map(|(_,target)|target.to_string()).collect::<Vec<_>>().join(",");
    CString::new(format!("{queues};{relocations};{private}")).map_err(|_|-(crate::errno::EINVAL as i64))
}
#[cfg(test)]
fn spawn_program(exe:&CString,argv:&[*const libc::c_char],transfers:&[(i32,i32)],port:Port)->Result<i32,i64>{spawn_program_suspended(exe,argv,transfers,port,false)}
fn spawn_program_suspended(exe:&CString,argv:&[*const libc::c_char],transfers:&[(i32,i32)],port:Port,suspended:bool)->Result<i32,i64>{
    validate_transfers(transfers)?;
    struct Setup{attr:libc::posix_spawnattr_t,actions:libc::posix_spawn_file_actions_t}
    impl Drop for Setup{fn drop(&mut self){unsafe{if !self.actions.is_null(){libc::posix_spawn_file_actions_destroy(&mut self.actions);}if !self.attr.is_null(){libc::posix_spawnattr_destroy(&mut self.attr);}}}}
    let mut setup=Setup{attr:std::ptr::null_mut(),actions:std::ptr::null_mut()};let mut pid=0;
    let checked=|error:i32|if error==0{Ok(())}else{Err(-(errno::from_darwin(error)as i64))};
    unsafe{
        checked(libc::posix_spawnattr_init(&mut setup.attr))?;checked(libc::posix_spawn_file_actions_init(&mut setup.actions))?;
        let mut all:libc::sigset_t=0;libc::sigfillset(&mut all);let none:libc::sigset_t=0;
        checked(libc::posix_spawnattr_setsigdefault(&mut setup.attr,&all))?;checked(libc::posix_spawnattr_setsigmask(&mut setup.attr,&none))?;
        checked(libc::posix_spawnattr_setflags(&mut setup.attr,(libc::POSIX_SPAWN_CLOEXEC_DEFAULT|libc::POSIX_SPAWN_SETSIGDEF|libc::POSIX_SPAWN_SETSIGMASK|if suspended{0x80}else{0})as i16))?;
        let mut ports=[port];checked(posix_spawnattr_set_registered_ports_np(&mut setup.attr,ports.as_mut_ptr(),1))?;
        for &(source,target)in transfers{
            checked(if target>=10240{posix_spawn_file_actions_addinherit_np(&mut setup.actions,source)}else{libc::posix_spawn_file_actions_adddup2(&mut setup.actions,source,target)})?;
        }
        checked(libc::posix_spawn(&mut pid,exe.as_ptr(),&setup.actions,&setup.attr,argv.as_ptr()as*const*mut libc::c_char,*_NSGetEnviron()as*const*mut libc::c_char))?;
    }
    Ok(pid)
}

#[cfg(test)]
mod namespace_receipt_tests{
    use super::*;
    use std::{io::{Read,Write},os::unix::ffi::OsStrExt,path::PathBuf,process::{Command,Stdio}};
    fn fields()->Vec<String>{std::env::args().find_map(|arg|arg.strip_prefix("PREBOUND_NAMESPACE=").map(str::to_owned)).expect("authenticated fixture arguments").split('|').map(str::to_owned).collect()}
    #[test]
    #[ignore="owned suspended child resumed after its publishing parent exits"]
    fn published_child(){
        let fields=fields();let map=PathBuf::from(&fields[1]);crate::vfs::inherit_fork_namespace(Some(&map)).unwrap();crate::vfs::init(std::path::Path::new(&fields[2]),Some(&map)).unwrap();
        assert_eq!(crate::vfs::mount_namespace_id().as_deref(),Some("prebound-child"));
        std::fs::write(&fields[3],b"ACTUAL_PARENT_EXIT_CHILD_EXECUTED").unwrap();
    }
    #[test]
    #[ignore="physical parent publishes actual suspended child namespace then exits"]
    fn publishing_parent(){
        let fields=fields();let marker=std::env::args().find(|arg|arg.starts_with("PREBOUND_NAMESPACE=")).unwrap();
        let exe=CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();let args=[exe.clone(),CString::new("--exact").unwrap(),CString::new("sys::fork::spawn::namespace_receipt_tests::published_child").unwrap(),CString::new("--ignored").unwrap(),CString::new("--nocapture").unwrap(),CString::new("--skip").unwrap(),CString::new(marker).unwrap()];let mut argv=args.iter().map(|arg|arg.as_ptr()).collect::<Vec<_>>();argv.push(std::ptr::null());
        let port=new_port().unwrap();let pid=spawn_program_suspended(&exe,&argv,&[],port,true).unwrap();
        let process=aim_storage::process_namespace::ProcessIdentity::running(pid).unwrap();aim_storage::process_namespace::register_mount_namespace(std::path::Path::new(&fields[0]),process,"prebound-child").unwrap();
        println!("SUSPENDED_CHILD:{pid}");std::io::stdout().flush().unwrap();
        unsafe{mach_port_deallocate(task(),port);mach_port_mod_refs(task(),port,MACH_PORT_RIGHT_RECEIVE,-1);}
    }
    #[test]
    fn published_receipt_survives_actual_parent_exit_before_child_resume(){
        if fdtab::isolated_kernel_test("sys::fork::spawn::namespace_receipt_tests::published_receipt_survives_actual_parent_exit_before_child_resume"){return;}
        let root=std::env::temp_dir().join(format!("aim-prebound-{}",std::process::id()));let _=std::fs::remove_dir_all(&root);let image=root.join("image");let runtime=root.join("run");let table=runtime.join("identity/by-pid");std::fs::create_dir_all(&image).unwrap();std::fs::create_dir_all(&table).unwrap();let map=runtime.join("path-map");let text=format!("root\t/\t{}\n",image.display());std::fs::write(&map,&text).unwrap();
        let namespace=aim_storage::mount_namespace::Namespace::open(&runtime,"prebound-child").unwrap();namespace.initialize(&text).unwrap();let actual=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();aim_storage::process_namespace::InitRegistration::register(&table,actual,namespace.id()).unwrap();
        let output=root.join("executed");let marker=format!("PREBOUND_NAMESPACE={}|{}|{}|{}",table.display(),map.display(),image.display(),output.display());
        let parent=Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::fork::spawn::namespace_receipt_tests::publishing_parent","--ignored","--nocapture","--skip",&marker]).stdout(Stdio::piped()).spawn().unwrap();
        struct Parent(std::process::Child);impl Drop for Parent{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}let mut parent=Parent(parent);let mut text=String::new();parent.0.stdout.take().unwrap().read_to_string(&mut text).unwrap();assert!(parent.0.wait().unwrap().success());let pid=text.lines().find_map(|line|line.strip_prefix("SUSPENDED_CHILD:")).unwrap().parse::<i32>().unwrap();
        struct Owned(aim_storage::process_namespace::ProcessIdentity);impl Drop for Owned{fn drop(&mut self){if self.0.is_live(){unsafe{libc::kill(self.0.host_pid,libc::SIGKILL);}}let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);while self.0.is_live()&&std::time::Instant::now()<deadline{std::thread::sleep(std::time::Duration::from_millis(5));}}}let child=Owned(aim_storage::process_namespace::ProcessIdentity::running(pid).unwrap());
        assert!(!output.exists());assert_eq!(unsafe{libc::kill(pid,libc::SIGCONT)},0);let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);while !output.exists(){assert!(std::time::Instant::now()<deadline,"prebound child did not resume after parent exit");std::thread::sleep(std::time::Duration::from_millis(5));}
        assert_eq!(std::fs::read(&output).unwrap(),b"ACTUAL_PARENT_EXIT_CHILD_EXECUTED");drop(child);std::fs::remove_dir_all(root).unwrap();
    }
}

/// Hand the child its state when it asks, on a host thread of the parent.
fn serve(port:Port,mut snap:Snapshot,blob:Vec<u8>,mut fork_lease:Option<aim_storage::verity_control::ForkLease>){
    let mut hello: Received<Header> = Received {
        msg: Header::default(),
        trailer: [0; 128],
    };
    // SAFETY: receiving into our buffer on our port.
    let kr = unsafe {
        mach_msg(
            &mut hello.msg,
            MACH_RCV_MSG | MACH_RCV_TIMEOUT,
            0,
            std::mem::size_of::<Received<Header>>() as u32,
            port,
            HANDOVER_TIMEOUT_MS,
            0,
        )
    };
    let reply = hello.msg.remote;
    if kr == 0 && hello.msg.id == HELLO && reply != 0 {
        let mut msg = Payload {
            header: Header {
                bits: MACH_MSG_TYPE_MOVE_SEND_ONCE | MACH_MSGH_BITS_COMPLEX,
                size: std::mem::size_of::<Payload>() as u32,
                remote: reply,
                ..Header::default()
            },
            descriptors: 2,
            entries: OolPorts {
                address: snap.entries.as_ptr() as u64,
                deallocate: 0,
                copy: MACH_MSG_VIRTUAL_COPY,
                disposition: MACH_MSG_TYPE_MOVE_SEND as u8,
                kind: MACH_MSG_OOL_PORTS_DESCRIPTOR,
                count: snap.entries.len() as u32,
            },
            blob: Ool {
                address: blob.as_ptr() as u64,
                deallocate: 0,
                copy: MACH_MSG_VIRTUAL_COPY,
                pad: 0,
                kind: MACH_MSG_OOL_DESCRIPTOR,
                size: blob.len() as u32,
            },
        };
        // SAFETY: sending our message; the entries' rights move with it.
        let kr = unsafe {
            mach_msg(
                &mut msg.header,
                MACH_SEND_MSG,
                std::mem::size_of::<Payload>() as u32,
                0,
                0,
                0,
                0,
            )
        };
        if kr == 0 {
            snap.entries.clear();
            let mut ready=Received{msg:Ready::default(),trailer:[0;128]};
            let result=unsafe{mach_msg(&mut ready.msg.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,std::mem::size_of::<Received<Ready>>()as u32,port,HANDOVER_TIMEOUT_MS,0)};
            let valid=result==0&&ready.msg.header.id==READY&&ready.msg.header.remote!=0;
            let verified=if let Some(lease)=fork_lease.take(){
                if valid&&ready.msg.member!=0{lease.child_published(ready.msg.member)}else{let cleanup=lease.abort();cleanup.and_then(|_|Err(std::io::Error::from_raw_os_error(libc::EPROTO)))}
            }else if valid{Ok(())}else{Err(std::io::Error::from_raw_os_error(libc::EPROTO))};
            if valid{
                let error=verified.err().map(|error|crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))).unwrap_or(0);
                let mut ack=ReadyAck{header:Header{bits:MACH_MSG_TYPE_MOVE_SEND_ONCE,size:std::mem::size_of::<ReadyAck>()as u32,remote:ready.msg.header.remote,id:READY_ACK,..Header::default()},error};
                let sent=unsafe{mach_msg(&mut ack.header,MACH_SEND_MSG,std::mem::size_of::<ReadyAck>()as u32,0,0,0,0)};
                if sent!=0{crate::diag!("[linux-abi] fork ready acknowledgement: kr {sent:#x}");}
            }
        } else {
            crate::diag!("[linux-abi] fork: cannot send the child its state (kr {kr:#x})");
        }
    } else if kr != 0 {
        crate::diag!("[linux-abi] fork: the child never asked for its state (kr {kr:#x})");
    }
    if let Some(lease)=fork_lease.take(){if let Err(error)=lease.abort(){crate::diag!("[linux-abi] fork verity abort: {error}");}}
    // SAFETY: dropping our receive right; the child's send right dies.
    unsafe { mach_port_mod_refs(task(), port, MACH_PORT_RIGHT_RECEIVE, -1) };
    handover_done();
}

// ---- the child ----------------------------------------------------------------------

/// The parent's state, received by a `linux-run --fork-child`.
struct Handover {
    parent:Port,
    blob: Vec<u8>,
    entries: Vec<Port>,
}

impl Drop for Handover {
    fn drop(&mut self) {
        unsafe{mach_port_deallocate(task(),self.parent);}
        for &e in &self.entries {
            // SAFETY: our send rights to the memory entries.
            unsafe { mach_port_deallocate(task(), e) };
        }
    }
}

/// Ask the parent for its state.
fn receive() -> Result<Handover, String> {
    let mut ports: *mut Port = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: the registered ports the parent gave us; the array is ours
    // to free, and none are passed on to our own children.
    let parent = unsafe {
        if mach_ports_lookup(task(), &mut ports, &mut count) != 0 || count == 0 {
            return Err("fork child: no parent port".into());
        }
        let p = *ports;
        for i in 1..count as usize {
            if *ports.add(i) != 0 {
                mach_port_deallocate(task(), *ports.add(i));
            }
        }
        mach_vm_deallocate(task(), ports as u64, count as u64 * 4);
        mach_ports_register(task(), std::ptr::null_mut(), 0);
        p
    };
    let reply = new_port().map_err(|_| "fork child: no reply port".to_string())?;
    let mut hello = Header {
        bits: MACH_MSG_TYPE_COPY_SEND | MACH_MSG_TYPE_MAKE_SEND_ONCE << 8,
        size: std::mem::size_of::<Header>() as u32,
        remote: parent,
        local: reply,
        voucher: 0,
        id: HELLO,
    };
    let mut got: Received<Payload> = Received {
        // SAFETY: plain data, overwritten by the receive.
        msg: unsafe { std::mem::zeroed() },
        trailer: [0; 128],
    };
    // SAFETY: our messages and buffers.
    unsafe {
        let kr = mach_msg(
            &mut hello,
            MACH_SEND_MSG,
            std::mem::size_of::<Header>() as u32,
            0,
            0,
            0,
            0,
        );
        if kr != 0 {
            mach_port_deallocate(task(),parent);
            return Err(format!("fork child: cannot reach the parent (kr {kr:#x})"));
        }
        let kr = mach_msg(
            &mut got.msg.header,
            MACH_RCV_MSG | MACH_RCV_TIMEOUT,
            0,
            std::mem::size_of::<Received<Payload>>() as u32,
            reply,
            HANDOVER_TIMEOUT_MS,
            0,
        );
        mach_port_deallocate(task(), reply);
        mach_port_mod_refs(task(), reply, MACH_PORT_RIGHT_RECEIVE, -1);
        if kr != 0 {
            mach_port_deallocate(task(),parent);
            return Err(format!("fork child: no state from the parent (kr {kr:#x})"));
        }
        let m = got.msg;
        if m.header.bits & MACH_MSGH_BITS_COMPLEX == 0 || m.descriptors != 2 {
            mach_port_deallocate(task(),parent);
            return Err("fork child: malformed state message".into());
        }
        let (ea, en) = (m.entries.address, m.entries.count as usize);
        let entries = std::slice::from_raw_parts(ea as *const Port, en).to_vec();
        mach_vm_deallocate(task(), ea, en as u64 * 4);
        let (ba, bn) = (m.blob.address, m.blob.size as usize);
        let blob = std::slice::from_raw_parts(ba as *const u8, bn).to_vec();
        mach_vm_deallocate(task(), ba, bn as u64);
        Ok(Handover{parent,blob,entries})
    }
}

/// Hold the fd numbers the guest's kqueues had (a comma-separated list)
/// with /dev/null until they are rebuilt, so nothing the runtime opens
/// meanwhile takes them. First thing in a `linux-run --fork-child`.
fn validate_transfers(transfers:&[(i32,i32)])->Result<(),i64>{
    let mut sources=std::collections::HashSet::new();let mut targets=std::collections::HashSet::new();
    for &(source,target)in transfers{
        if source<3||source>=10240||target<0||!sources.insert(source)||!targets.insert(target)||unsafe{libc::fcntl(source,libc::F_GETFD)}<0{return Err(-(crate::errno::EBADF as i64));}
    }
    if transfers.iter().any(|(_,target)|sources.contains(target)){return Err(-(crate::errno::EINVAL as i64));}
    Ok(())
}
pub fn reserve_fds(list: &str)->Result<(),crate::errno::Errno>{
    use std::os::fd::{FromRawFd,OwnedFd};
    let fields=list.split(';').collect::<Vec<_>>();if fields.len()>3{return Err(crate::errno::EINVAL);}
    let queues=fdtab::parse_guest_fds(fields[0])?;let relocations=fields.get(1).copied().unwrap_or("");
    let private=fdtab::parse_guest_fds(fields.get(2).copied().unwrap_or(""))?;
    if private.iter().any(|fd|queues.contains(fd)){return Err(crate::errno::EINVAL);}
    let mut transfers=Vec::new();
    if !relocations.is_empty(){for record in relocations.split(','){
        let(source,target)=record.split_once(':').ok_or(crate::errno::EINVAL)?;
        let source=source.parse::<i32>().map_err(|_|crate::errno::EINVAL)?;let target=target.parse::<i32>().map_err(|_|crate::errno::EINVAL)?;
        if target<10240||queues.contains(&target){return Err(crate::errno::EINVAL);}transfers.push((source,target));
    }}
    validate_transfers(&transfers).map_err(|error|(-error)as i32)?;
    if private.iter().any(|fd|*fd>=10240&&!transfers.iter().any(|(_,target)|target==fd)){return Err(crate::errno::EINVAL);}
    for &(_,target)in &transfers{if unsafe{libc::fcntl(target,libc::F_GETFD)}>=0{return Err(crate::errno::EBADF);}}
    let mut installed=Vec::<OwnedFd>::new();
    for &(source,target)in &transfers{
        if unsafe{libc::dup2(source,target)}<0{return Err(crate::errno::last());}
        installed.push(unsafe{OwnedFd::from_raw_fd(target)});
        if unsafe{libc::fcntl(target,libc::F_SETFD,libc::FD_CLOEXEC)}<0{return Err(crate::errno::last());}
    }
    if !queues.is_empty(){
        let raw=unsafe{libc::open(c"/dev/null".as_ptr(),libc::O_RDONLY|libc::O_CLOEXEC)};if raw<0{return Err(crate::errno::last());}
        let null=unsafe{OwnedFd::from_raw_fd(raw)};
        for fd in &queues{
            if *fd==raw{continue;}
            if unsafe{libc::fcntl(*fd,libc::F_GETFD)}>=0{return Err(crate::errno::EBADF);}
            if unsafe{libc::dup2(raw,*fd)}<0{return Err(crate::errno::last());}
            installed.push(unsafe{OwnedFd::from_raw_fd(*fd)});
            if unsafe{libc::fcntl(*fd,libc::F_SETFD,libc::FD_CLOEXEC)}<0{return Err(crate::errno::last());}
        }
        if queues.contains(&raw){installed.push(null);}
    }
    for &fd in &private{let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{return Err(crate::errno::EBADF);}if unsafe{libc::fcntl(fd,libc::F_SETFD,flags|libc::FD_CLOEXEC)}<0{return Err(crate::errno::last());}}
    // No target is made guest-visible here. A malformed/failed receipt closes
    // only targets installed by this operation; existing child slots are never replaced.
    let mut marked=Vec::new();
    for &fd in &private{if let Err(error)=super::super::fd_visibility::hide(fd){for fd in marked{super::super::fd_visibility::private_closed(fd);}return Err(error);}marked.push(fd);}
    for &fd in &private{fdtab::keep_hidden(fd);}
    for &(source,_)in &transfers{if unsafe{libc::close(source)}<0{return Err(crate::errno::last());}}
    for fd in installed{use std::os::fd::IntoRawFd;let _=fd.into_raw_fd();}Ok(())
}

/// Become the child of the fork that spawned this process: map the
/// parent's memory, take over its state and resume the forking thread.
/// Returns only on failure. The runtime (vfs, heap window, binder) is set
/// up already.
pub fn child_main() -> String {
    match receive() {
        Ok(h) => become_child(h),
        Err(e) => e,
    }
}

fn become_child(h: Handover) -> String {
    let mut r = Reader::new(&h.blob);
    let stack = r.u64();
    let tls = r.opt(|r| r.u64());
    let set_tid = r.u64();
    let clear_tid = r.u64();
    let vfork = r.opt(|r| (r.i32(), r.i32()));
    let traced = r.opt(|r| (r.i32(), r.u64()));
    let thread_of = r.opt(|r| r.i32());
    let ctx = context::init_thread();
    // Traced before the agent serves (`state::restore`), and stopped before
    // the first guest instruction.
    if let Some((tracer, options)) = traced {
        // SAFETY: trivial.
        super::super::ptrace::born_traced(unsafe { libc::getpid() }, tracer, options);
    }
    crate::diag::install_signal_handlers();
    // SAFETY: this thread's fresh context.
    let c = unsafe { &mut *ctx };
    load_context(&mut r, c);
    let tp = r.u64();
    let scs = r.bytes();
    crate::vfs::set_cwd(r.str());
    if let Err(error)=crate::vfs::load_own_mounts(&r.str()){return format!("fork mount namespace: errno {error}");}
    if let Err(e) = map_regions(&mut r, &h.entries) {
        return e;
    }
    if !state::restore(&mut r) {
        return "fork child: damaged state".into();
    }
    let mut consumed=fdtab::take_restored_private_targets();
    match super::super::verity_pager::ForkSnapshot::restore(&mut r){Ok(targets)=>consumed.extend(targets),Err(error)=>return format!("fork verity pager: {error}")}
    consumed.sort_unstable();consumed.dedup();
    for fd in consumed{if let Err(error)=fdtab::close_fork_private(fd){return format!("fork private receipt close: errno {error}");}}
    for (fd, cloexec) in r.seq(|r| (r.i32(), r.bool())) {
        if unsafe{libc::fcntl(fd,libc::F_GETFD)}<0{return "fork inherited descriptor missing".into();}
        if unsafe{libc::fcntl(fd,libc::F_SETFD,if cloexec{libc::FD_CLOEXEC}else{0})}<0{return format!("fork descriptor flags: errno {}",crate::errno::last());}
    }
    if !r.ok() {
        return "fork child: damaged state".into();
    }
    if let Err(error)=super::super::verity_pager::restore_memberships(){return format!("fork restored mapping publication: {error}");}
    if let Err(error)=confirm_ready(h.parent,crate::verity_client().map(|client|client.member).unwrap_or(0)){return error;}
    drop(h);
    if let Some((rd, _)) = vfork {
        // SAFETY: the parent's end of its wait pipe, inherited.
        if let Err(error)=fdtab::close_fork_private(rd){return format!("vfork reader close: errno {error}");}
    }

    // The forking thread, now this process's main thread.
    let base = context::guest_scs();
    // SAFETY: our fresh shadow stack is larger than any parent's in use.
    unsafe { std::ptr::copy_nonoverlapping(scs.as_ptr(), base as *mut u8, scs.len()) };
    context::set_guest_scs(base + scs.len() as u64);
    context::set_guest_tp(tls.unwrap_or(tp));
    super::child_started(c, stack, set_tid, clear_tid);
    if let Some(p) = thread_of {
        super::super::procrec::set_tgid(p);
    }
    // SAFETY: this thread's context, holding the forking thread's registers.
    unsafe { context::resume(ctx) }
}

fn confirm_ready(parent:Port,member:u64)->Result<(),String>{
    let ready_reply=new_port().map_err(|error|format!("fork child ready reply: {error}"))?;
    let mut ready=Ready{header:Header{bits:MACH_MSG_TYPE_COPY_SEND|MACH_MSG_TYPE_MAKE_SEND_ONCE<<8,size:std::mem::size_of::<Ready>()as u32,remote:parent,local:ready_reply,id:READY,..Header::default()},member};
    let sent=unsafe{mach_msg(&mut ready.header,MACH_SEND_MSG,std::mem::size_of::<Ready>()as u32,0,0,0,0)};
    let mut ack=Received{msg:ReadyAck::default(),trailer:[0;128]};
    let received=if sent==0{unsafe{mach_msg(&mut ack.msg.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,std::mem::size_of::<Received<ReadyAck>>()as u32,ready_reply,HANDOVER_TIMEOUT_MS,0)}}else{sent};
    unsafe{mach_port_deallocate(task(),ready_reply);mach_port_mod_refs(task(),ready_reply,MACH_PORT_RIGHT_RECEIVE,-1);}
    if received!=0||ack.msg.header.id!=READY_ACK{return Err(format!("fork child ready acknowledgement: kr {received:#x}"));}
    let error=ack.msg.error;if error!=0{return Err(format!("fork child verity handoff rejected: errno {error}"));}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_native_ready_ack_cannot_reach_resume(){
        let port=new_port().unwrap();
        let receiver=std::thread::spawn(move||{
            let mut message=Received{msg:Ready::default(),trailer:[0;128]};
            let result=unsafe{mach_msg(&mut message.msg.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,std::mem::size_of::<Received<Ready>>()as u32,port,HANDOVER_TIMEOUT_MS,0)};
            assert_eq!(result,0);assert_eq!(message.msg.header.id,READY);
            let mut ack=ReadyAck{header:Header{bits:MACH_MSG_TYPE_MOVE_SEND_ONCE,size:std::mem::size_of::<ReadyAck>()as u32,remote:message.msg.header.remote,id:READY_ACK,..Header::default()},error:crate::errno::EPERM};
            assert_eq!(unsafe{mach_msg(&mut ack.header,MACH_SEND_MSG,std::mem::size_of::<ReadyAck>()as u32,0,0,0,0)},0);
        });
        let result=confirm_ready(port,77);receiver.join().unwrap();
        assert!(result.unwrap_err().contains("errno 1"));
        unsafe{mach_port_deallocate(task(),port);mach_port_mod_refs(task(),port,MACH_PORT_RIGHT_RECEIVE,-1);}
    }

    fn faults() -> i64 {
        // SAFETY: getrusage into a local.
        unsafe {
            let mut r: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_SELF, &mut r);
            r.ru_minflt
        }
    }

    fn touch(at: u64, len: u64) {
        for o in (0..len).step_by(16384) {
            // SAFETY: reading a mapped page.
            unsafe { std::ptr::read_volatile((at + o) as *const u8) };
        }
    }

    /// A fork child's mapping of shared code keeps its pages mapped, and
    /// the parent's, from one touch to the next (#444).
    #[test]
    fn shared_code_stays_mapped() {
        let len = 8 * 16384;
        let path = std::env::temp_dir().join(format!("aim-fork-code-{}", std::process::id()));
        std::fs::write(&path, vec![0xc0u8; len as usize]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        // SAFETY: fresh mappings of our own file; each is unmapped below.
        unsafe {
            use std::os::fd::AsRawFd;
            let code = libc::mmap(
                std::ptr::null_mut(),
                len as usize,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            ) as u64;
            assert_eq!(
                libc::mprotect(
                    code as *mut _,
                    len as usize,
                    libc::PROT_READ | libc::PROT_EXEC
                ),
                0
            );
            let rx = VM_PROT_READ | VM_PROT_EXECUTE;
            let (mut size, mut e) = (len, 0);
            assert_eq!(
                mach_make_memory_entry_64(
                    task(),
                    &mut size,
                    code,
                    rx | MAP_MEM_VM_SHARE,
                    &mut e,
                    0
                ),
                0
            );
            let mut view = 0;
            assert_eq!(
                map_entry(
                    &mut view,
                    len,
                    VM_FLAGS_ANYWHERE,
                    e,
                    rx,
                    rx,
                    VM_INHERIT_SHARE
                ),
                0
            );
            mach_port_deallocate(task(), e);
            touch(code, len);
            touch(view, len);
            let before = faults();
            let rounds = 4;
            for _ in 0..rounds {
                touch(code, len);
                touch(view, len);
            }
            let refaults = faults() - before;
            mach_vm_deallocate(task(), view, len);
            libc::munmap(code as *mut _, len as usize);
            // A few of the test's own, not one per page and round.
            assert!(refaults < rounds * 2, "{refaults} refaults");
        }
    }
}

#[cfg(test)]
mod fd_owner_tests{
 use super::*;
 use std::{fs::File,io::{Read,Write},os::{fd::{AsFd,AsRawFd,FromRawFd},unix::ffi::OsStrExt}};
 const CHILD:&str="sys::fork::spawn::fd_owner_tests::receipt_child";
 #[test]
 #[ignore="actual subprocess of canonical_guest_and_explicit_private_receipts_survive_actual_spawn"]
 fn receipt_child(){
  let argument=std::env::args().find(|arg|arg.starts_with("FORK_FD_OWNER=")).unwrap();let fields=argument.strip_prefix("FORK_FD_OWNER=").unwrap().split('|').collect::<Vec<_>>();assert_eq!(fields.len(),10);
  let guest=fields[0].parse::<i32>().unwrap();let high=fields[1].parse::<i32>().unwrap();let unrelated=fields[2].parse::<i32>().unwrap();let event=fields[3].parse::<i32>().unwrap();let timer=fields[4].parse::<i32>().unwrap();let ino=fields[7].parse::<i32>().unwrap();let fence=fields[8].parse::<i32>().unwrap();let retired=fields[9].parse::<i32>().unwrap();
  reserve_fds(fields[5]).unwrap();assert!(!fdtab::visible(high));assert!(fdtab::is_hidden(high));assert_ne!(unsafe{libc::fcntl(high,libc::F_GETFD)}&libc::FD_CLOEXEC,0);
  assert_eq!(unsafe{libc::fcntl(unrelated,libc::F_GETFD)},-1,"unrelated native private fd inherited");assert!(!fdtab::is_hidden(unrelated));
  let mut blob=Vec::new();File::open(fields[6]).unwrap().read_to_end(&mut blob).unwrap();let mut reader=Reader::new(&blob);fdtab::fork_restore(&mut reader);crate::sys::sync_file::fork_restore(&mut reader);assert!(reader.intact());assert_eq!(unsafe{libc::fcntl(retired,libc::F_GETFD)},-1);assert!(!fdtab::is_hidden(retired));let replaced=unsafe{libc::dup2(1,retired)};assert_eq!(replaced,retired);fdtab::publish_guest(retired).unwrap();assert_eq!(crate::sys::fs::close([retired as u64,0,0,0,0,0]),0);fdtab::after_fork_child();
  for(fd,cloexec)in reader.seq(|r|(r.i32(),r.bool())){assert_eq!(unsafe{libc::fcntl(fd,libc::F_SETFD,if cloexec{libc::FD_CLOEXEC}else{0})},0);}assert!(reader.ok());
  assert!(fdtab::visible(guest));assert!(!fdtab::visible(high));assert!(fdtab::is_hidden(high));
  let mut bytes=[0u8;32];assert_eq!(crate::sys::fs::read([guest as u64,bytes.as_mut_ptr()as u64,32,0,0,0]),12);assert_eq!(&bytes[..12],b"actual bytes");assert_eq!(crate::sys::fs::close([guest as u64,0,0,0,0,0]),0);assert!(!fdtab::visible(guest));
  let mut private_bytes=[0u8;12];assert_eq!(unsafe{libc::pread(high,private_bytes.as_mut_ptr().cast(),12,0)},12);assert_eq!(&private_bytes,b"actual bytes");
  assert_eq!(crate::sys::fs::close([high as u64,0,0,0,0,0]),-9);
  assert_eq!(crate::sys::event::read(event,bytes.as_mut_ptr()as u64,8),Some(8));assert_eq!(u64::from_ne_bytes(bytes[..8].try_into().unwrap()),3);
  let mut spec=[0u8;32];spec[16..24].copy_from_slice(&1i64.to_le_bytes());assert_eq!(crate::sys::event::timerfd_settime([timer as u64,0,spec.as_ptr()as u64,0,0,0]),0);
  assert_eq!(crate::sys::fs::close([event as u64,0,0,0,0,0]),0);assert_eq!(crate::sys::fs::close([timer as u64,0,0,0,0,0]),0);
  let watch_file=std::path::Path::new(fields[6]).parent().unwrap().join("watch-location");let host=std::fs::read_to_string(watch_file).unwrap();File::options().append(true).open(host).unwrap().write_all(b"changed").unwrap();
  let mut event_bytes=[0u8;128];assert!(crate::sys::inotify::read(ino,event_bytes.as_mut_ptr()as u64,128).unwrap()>0);
  assert_eq!(crate::sys::fs::close([ino as u64,0,0,0,0,0]),0);
  assert!(matches!(aim_sync_file::state(unsafe{std::os::fd::BorrowedFd::borrow_raw(fence)}),aim_sync_file::State::Active));assert_eq!(crate::sys::fs::close([fence as u64,0,0,0,0,0]),0);
  fdtab::close_fork_private(high).unwrap();
  let recycled=unsafe{libc::fcntl(1,libc::F_DUPFD_CLOEXEC,high)};assert_eq!(recycled,high);fdtab::publish_guest(recycled).unwrap();assert_eq!(crate::sys::fs::close([recycled as u64,0,0,0,0,0]),0);
  std::fs::write(std::path::Path::new(fields[6]).parent().unwrap().join("child-verified"),b"FORK_FD_OWNER_CHILD_EXECUTED").unwrap();println!("FORK_FD_OWNER_CHILD_EXECUTED");
 }
 #[test]
 fn canonical_guest_and_explicit_private_receipts_survive_actual_spawn(){
  if fdtab::isolated_kernel_test("sys::fork::spawn::fd_owner_tests::canonical_guest_and_explicit_private_receipts_survive_actual_spawn"){return;}
  fdtab::install_storage_registrar().unwrap();
  let directory=std::env::temp_dir().join(format!("aim-fork-owner-{}",std::process::id()));std::fs::create_dir(&directory).unwrap();
  let data=directory.join("data");std::fs::write(&data,b"actual bytes").unwrap();let guest=File::open(&data).unwrap();fdtab::publish_guest(guest.as_raw_fd()).unwrap();
  let unrelated=aim_storage::private_fd::PrivateFile::allocate(||File::open("/dev/null")).unwrap();let unrelated_fd=unrelated.as_raw_fd();
  let raw=unsafe{libc::fcntl(guest.as_raw_fd(),libc::F_DUPFD_CLOEXEC,11000)};assert!(raw>=11000);let high=unsafe{std::os::fd::OwnedFd::from_raw_fd(raw)};crate::sys::fd_visibility::hide(raw).unwrap();fdtab::keep_hidden(raw);
  let event=crate::sys::event::eventfd2([3,0,0,0,0,0])as i32;assert!(event>=0);let timer=crate::sys::event::timerfd_create([1,0,0,0,0,0])as i32;assert!(timer>=0);
  let(_vfs,view)=crate::vfs::test_view();let watch_host=view.join("data/watch");std::fs::write(&watch_host,b"watched").unwrap();std::fs::write(directory.join("watch-location"),watch_host.to_str().unwrap()).unwrap();
  let ino=crate::sys::inotify::inotify_init1([0,0,0,0,0,0])as i32;assert!(ino>=0);let watch=CString::new("/data/watch").unwrap();assert!(crate::sys::inotify::inotify_add_watch([ino as u64,watch.as_ptr()as u64,2,0,0,0])>=0);
  crate::sys::sync_file::init();let(fence,producer)=aim_sync_file::pair().unwrap();let fence=aim_sync_file::give_to_guest(fence).unwrap();let retired=aim_sync_file::inherited()[0];
  let mut writer=Writer::default();let guard=fdtab::lifecycle();let(fds,guest_owners)=fdtab::fork_guest_snapshot().unwrap();fdtab::fork_save(&mut writer);crate::sys::sync_file::fork_save(&mut writer);writer.seq(fds.iter(),|w,(fd,cloexec)|{w.i32(*fd);w.bool(*cloexec)});let private=writer.take_private().unwrap();let explicit=fdtab::hold_fork_private(high.as_fd()).unwrap();let stdout=fdtab::hold_fork_private(unsafe{std::os::fd::BorrowedFd::borrow_raw(1)}).unwrap();let stderr=fdtab::hold_fork_private(unsafe{std::os::fd::BorrowedFd::borrow_raw(2)}).unwrap();
  let mut transfers=Vec::new();for owner in guest_owners.iter().chain(private.iter()).chain([&explicit,&stdout,&stderr]){transfers.push((owner.source().as_raw_fd(),owner.target()));}
  let reservation=reservation_text(&fdtab::kqueue_fds(),&fds,&transfers).unwrap();drop(guard);
  let blob=directory.join("state");File::create(&blob).unwrap().write_all(&writer.into_bytes()).unwrap();
  let marker=format!("FORK_FD_OWNER={}|{}|{}|{}|{}|{}|{}|{}|{}|{}",guest.as_raw_fd(),raw,unrelated_fd,event,timer,reservation.to_str().unwrap(),blob.display(),ino,fence,retired);
  let executable=CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();let arguments=[executable.clone(),CString::new("--exact").unwrap(),CString::new(CHILD).unwrap(),CString::new("--ignored").unwrap(),CString::new("--nocapture").unwrap(),CString::new("--skip").unwrap(),CString::new(marker).unwrap()];let mut argv=arguments.iter().map(|arg|arg.as_ptr()).collect::<Vec<_>>();argv.push(std::ptr::null());
  let port=new_port().unwrap();let pid=spawn_program(&executable,&argv,&transfers,port).unwrap();
  struct Child(Option<i32>);impl Drop for Child{fn drop(&mut self){if let Some(pid)=self.0{unsafe{libc::kill(pid,libc::SIGKILL);libc::waitpid(pid,std::ptr::null_mut(),0);}}}}
  let mut child=Child(Some(pid));let mut status=0;assert_eq!(unsafe{libc::waitpid(pid,&mut status,0)},pid);child.0=None;assert!(libc::WIFEXITED(status));assert_eq!(libc::WEXITSTATUS(status),0);assert_eq!(std::fs::read(directory.join("child-verified")).unwrap(),b"FORK_FD_OWNER_CHILD_EXECUTED");
  unsafe{mach_port_deallocate(task(),port);mach_port_mod_refs(task(),port,MACH_PORT_RIGHT_RECEIVE,-1);}
  assert_eq!(crate::sys::fs::close([guest.as_raw_fd()as u64,0,0,0,0,0]),0);std::mem::forget(guest);
  assert_eq!(crate::sys::fs::close([event as u64,0,0,0,0,0]),0);assert_eq!(crate::sys::fs::close([timer as u64,0,0,0,0,0]),0);assert_eq!(crate::sys::fs::close([ino as u64,0,0,0,0,0]),0);assert_eq!(crate::sys::fs::close([fence as u64,0,0,0,0,0]),0);drop(producer);
  drop(high);fdtab::unhide(raw);crate::sys::fd_visibility::private_closed(raw);std::fs::remove_dir_all(directory).unwrap();
 }
 #[test]
 #[ignore="actual subprocess of private_vfork_writer_keeps_parent_blocked_until_exec_or_exit"]
 fn vfork_receipt_child(){
  let marker=std::env::args().find(|arg|arg.starts_with("VFORK_OWNER=")).unwrap();let fields=marker.strip_prefix("VFORK_OWNER=").unwrap().split('|').collect::<Vec<_>>();assert_eq!(fields.len(),7);
  let rd=fields[0].parse::<i32>().unwrap();let wr=fields[1].parse::<i32>().unwrap();let ready=fields[2].parse::<i32>().unwrap();let command=fields[3].parse::<i32>().unwrap();reserve_fds(fields[4]).unwrap();fdtab::close_fork_private(rd).unwrap();assert_eq!(unsafe{libc::fcntl(wr,libc::F_GETFD)}&libc::FD_CLOEXEC,libc::FD_CLOEXEC);
  assert_eq!(unsafe{libc::write(ready,b"r".as_ptr().cast(),1)},1);let mut byte=0u8;assert_eq!(unsafe{libc::read(command,(&mut byte as*mut u8).cast(),1)},1);
  std::fs::write(fields[6],b"VFORK_OWNER_CHILD_EXECUTED").unwrap();
  if fields[5]=="exec"{use std::os::unix::process::CommandExt;let error=std::process::Command::new("/usr/bin/true").exec();panic!("actual vfork writer exec failed: {error}");}
  assert_eq!(fields[5],"exit");
 }
 #[test]
 fn private_vfork_writer_keeps_parent_blocked_until_exec_or_exit(){
  if fdtab::isolated_kernel_test("sys::fork::spawn::fd_owner_tests::private_vfork_writer_keeps_parent_blocked_until_exec_or_exit"){return;}
  fdtab::install_storage_registrar().unwrap();
  fn pipe()->[std::os::fd::OwnedFd;2]{let mut fds=[0;2];assert_eq!(unsafe{libc::pipe(fds.as_mut_ptr())},0);unsafe{[std::os::fd::OwnedFd::from_raw_fd(fds[0]),std::os::fd::OwnedFd::from_raw_fd(fds[1])]}}
  for mode in ["exec","exit"]{
   let done=pipe();let ready=pipe();let command=pipe();let mut writer=Writer::default();save_vfork(&mut writer,Some((done[0].as_raw_fd(),done[1].as_raw_fd())));writer.retain_private(ready[1].as_raw_fd());writer.retain_private(command[0].as_raw_fd());writer.retain_private(2);let owners=writer.take_private().unwrap();let transfers=owners.iter().map(|owner|(owner.source().as_raw_fd(),owner.target())).collect::<Vec<_>>();let spec=reservation_text(&[],&[],&transfers).unwrap();
   let completion=std::env::temp_dir().join(format!("aim-vfork-owner-{}-{mode}",std::process::id()));
   let marker=format!("VFORK_OWNER={}|{}|{}|{}|{}|{mode}|{}",done[0].as_raw_fd(),done[1].as_raw_fd(),ready[1].as_raw_fd(),command[0].as_raw_fd(),spec.to_str().unwrap(),completion.display());let exe=CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();let args=[exe.clone(),CString::new("--exact").unwrap(),CString::new("sys::fork::spawn::fd_owner_tests::vfork_receipt_child").unwrap(),CString::new("--ignored").unwrap(),CString::new("--nocapture").unwrap(),CString::new("--skip").unwrap(),CString::new(marker).unwrap()];let mut argv=args.iter().map(|arg|arg.as_ptr()).collect::<Vec<_>>();argv.push(std::ptr::null());let port=new_port().unwrap();let pid=spawn_program(&exe,&argv,&transfers,port).unwrap();
   struct Child(Option<i32>);impl Drop for Child{fn drop(&mut self){if let Some(pid)=self.0{unsafe{libc::kill(pid,libc::SIGKILL);libc::waitpid(pid,std::ptr::null_mut(),0);}}}}let mut child=Child(Some(pid));drop(owners);let[done_reader,done_writer]=done;drop(done_writer);let mut event=libc::pollfd{fd:ready[0].as_raw_fd(),events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut event,1,2000)},1);let mut byte=0u8;assert_eq!(unsafe{libc::read(ready[0].as_raw_fd(),(&mut byte as*mut u8).cast(),1)},1);
   event.fd=done_reader.as_raw_fd();event.revents=0;assert_eq!(unsafe{libc::poll(&mut event,1,0)},0,"parent resumed before child exec/exit");assert_eq!(unsafe{libc::write(command[1].as_raw_fd(),b"go".as_ptr().cast(),1)},1);assert_eq!(unsafe{libc::poll(&mut event,1,2000)},1);assert_eq!(unsafe{libc::read(done_reader.as_raw_fd(),(&mut byte as*mut u8).cast(),1)},0);
   let mut status=0;assert_eq!(unsafe{libc::waitpid(pid,&mut status,0)},pid);child.0=None;assert!(libc::WIFEXITED(status));assert_eq!(libc::WEXITSTATUS(status),0);assert_eq!(std::fs::read(&completion).unwrap(),b"VFORK_OWNER_CHILD_EXECUTED");std::fs::remove_file(completion).unwrap();unsafe{mach_port_deallocate(task(),port);mach_port_mod_refs(task(),port,MACH_PORT_RIGHT_RECEIVE,-1);}
  }
 }

}
