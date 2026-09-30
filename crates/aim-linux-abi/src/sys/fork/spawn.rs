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
use std::sync::{Condvar, Mutex};
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
const PROX_FDTYPE_KQUEUE: u32 = 5;

/// The id of the child's request message.
const HELLO: i32 = 0x666f_726b;
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

/// The open fds `posix_spawn` can pass (all but kqueues), with their
/// close-on-exec flags.
fn inheritable_fds() -> Vec<(i32, bool)> {
    // SAFETY: sizing call, then a buffer of that size.
    let fds: Vec<libc::proc_fdinfo> = unsafe {
        let pid = libc::getpid();
        let n = libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        let sz = std::mem::size_of::<libc::proc_fdinfo>();
        let mut v: Vec<libc::proc_fdinfo> = Vec::with_capacity(n as usize / sz + 16);
        let n = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            v.as_mut_ptr().cast(),
            (v.capacity() * sz) as i32,
        );
        if n <= 0 {
            return Vec::new();
        }
        v.set_len(n as usize / sz);
        v
    };
    fds.iter()
        .filter(|f| f.proc_fdtype != PROX_FDTYPE_KQUEUE)
        .filter_map(|f| {
            // SAFETY: plain fcntl; a closed fd is skipped.
            let fl = unsafe { libc::fcntl(f.proc_fd, libc::F_GETFD) };
            (fl >= 0).then_some((f.proc_fd, fl & libc::FD_CLOEXEC != 0))
        })
        .collect()
}

// ---- the parent ---------------------------------------------------------------------

/// What the forking `clone` asked for, as the child applies it.
pub struct ChildSetup {
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

/// Fork the calling guest thread's process; the child's pid.
pub fn fork(ctx: &GuestContext, setup: &ChildSetup, runtime: &[CString]) -> Result<i32, i64> {
    let exe = super::super::exec::launch_exe().ok_or(-(libc::ENOEXEC as i64))?;
    let mut w = Writer::default();
    w.u64(setup.stack);
    w.opt(setup.tls, |w, v| w.u64(v));
    w.u64(setup.set_tid);
    w.u64(setup.clear_tid);
    w.opt(setup.vfork, |w, (rd, wr)| {
        w.i32(rd);
        w.i32(wr);
    });
    save_context(&mut w, ctx);
    w.u64(context::guest_tp());
    w.bytes(&shadow_stack());
    w.str(&crate::vfs::cwd());
    w.str(&crate::vfs::own_mounts_text());
    let snap = snapshot()?;
    save_regions(&mut w, &snap.regions);
    state::save(&mut w);

    let port = new_port()?;
    // Another thread may close an fd between the listing and the spawn,
    // which then fails with EBADF.
    let mut tries = 0;
    let spawned = loop {
        let fds = inheritable_fds();
        match spawn(exe, runtime, &fds, port) {
            Err(e) if e == -(libc::EBADF as i64) && tries < 3 => tries += 1,
            r => break r.map(|pid| (pid, fds)),
        }
    };
    let pid = match spawned {
        Ok((pid, fds)) => {
            w.seq(fds.iter(), |w, (fd, cloexec)| {
                w.i32(*fd);
                w.bool(*cloexec);
            });
            // Before the handover: the child keeps this entry.
            crate::sys::cred::note_child(pid);
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
        .spawn(move || serve(port, snap, blob));
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
fn spawn(exe: &CString, runtime: &[CString], fds: &[(i32, bool)], port: Port) -> Result<i32, i64> {
    let mut kqueues: Vec<i32> = fdtab::kqueue_fds();
    kqueues.extend(super::super::wait::pidfd_fds());
    let kqueues: Vec<String> = kqueues.iter().map(|fd| fd.to_string()).collect();
    let kqueues = CString::new(kqueues.join(",")).unwrap_or_default();
    let mut argv: Vec<*const libc::c_char> = vec![exe.as_ptr()];
    argv.extend(runtime.iter().map(|a| a.as_ptr()));
    let fork_child = c"--fork-child";
    argv.extend([fork_child.as_ptr(), kqueues.as_ptr(), std::ptr::null()]);
    let mut pid = 0;
    // SAFETY: attribute and file action objects initialized and destroyed
    // here; NULL-terminated argument arrays that outlive the call.
    let e = unsafe {
        let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
        let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
        libc::posix_spawnattr_init(&mut attr);
        libc::posix_spawn_file_actions_init(&mut actions);
        let mut all: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        let none: libc::sigset_t = 0;
        libc::posix_spawnattr_setsigdefault(&mut attr, &all);
        libc::posix_spawnattr_setsigmask(&mut attr, &none);
        libc::posix_spawnattr_setflags(
            &mut attr,
            (libc::POSIX_SPAWN_CLOEXEC_DEFAULT
                | libc::POSIX_SPAWN_SETSIGDEF
                | libc::POSIX_SPAWN_SETSIGMASK) as i16,
        );
        let mut ports = [port];
        posix_spawnattr_set_registered_ports_np(&mut attr, ports.as_mut_ptr(), 1);
        for &(fd, _) in fds {
            if posix_spawn_file_actions_addinherit_np(&mut actions, fd) != 0 {
                crate::diag!("[linux-abi] fork: fd {fd} cannot be passed to the child");
            }
        }
        let e = libc::posix_spawn(
            &mut pid,
            exe.as_ptr(),
            &actions,
            &attr,
            argv.as_ptr() as *const *mut libc::c_char,
            *_NSGetEnviron() as *const *mut libc::c_char,
        );
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attr);
        e
    };
    if e != 0 {
        crate::diag!("[linux-abi] fork: posix_spawn failed (errno {e})");
        return Err(-(errno::from_darwin(e) as i64));
    }
    Ok(pid)
}

/// Hand the child its state when it asks, on a host thread of the parent.
fn serve(port: Port, mut snap: Snapshot, blob: Vec<u8>) {
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
        } else {
            crate::diag!("[linux-abi] fork: cannot send the child its state (kr {kr:#x})");
        }
    } else if kr != 0 {
        crate::diag!("[linux-abi] fork: the child never asked for its state (kr {kr:#x})");
    }
    // SAFETY: dropping our receive right; the child's send right dies.
    unsafe { mach_port_mod_refs(task(), port, MACH_PORT_RIGHT_RECEIVE, -1) };
    handover_done();
}

// ---- the child ----------------------------------------------------------------------

/// The parent's state, received by a `linux-run --fork-child`.
struct Handover {
    blob: Vec<u8>,
    entries: Vec<Port>,
}

impl Drop for Handover {
    fn drop(&mut self) {
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
        mach_port_deallocate(task(), parent);
        if kr != 0 {
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
            return Err(format!("fork child: no state from the parent (kr {kr:#x})"));
        }
        let m = got.msg;
        if m.header.bits & MACH_MSGH_BITS_COMPLEX == 0 || m.descriptors != 2 {
            return Err("fork child: malformed state message".into());
        }
        let (ea, en) = (m.entries.address, m.entries.count as usize);
        let entries = std::slice::from_raw_parts(ea as *const Port, en).to_vec();
        mach_vm_deallocate(task(), ea, en as u64 * 4);
        let (ba, bn) = (m.blob.address, m.blob.size as usize);
        let blob = std::slice::from_raw_parts(ba as *const u8, bn).to_vec();
        mach_vm_deallocate(task(), ba, bn as u64);
        Ok(Handover { blob, entries })
    }
}

/// Hold the fd numbers the guest's kqueues had (a comma-separated list)
/// with /dev/null until they are rebuilt, so nothing the runtime opens
/// meanwhile takes them. First thing in a `linux-run --fork-child`.
pub fn reserve_fds(list: &str) {
    // SAFETY: opening /dev/null and duplicating it onto free fd numbers.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if null < 0 {
            return;
        }
        for fd in list.split(',').filter_map(|n| n.parse::<i32>().ok()) {
            if fd != null {
                libc::dup2(null, fd);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        if !list.split(',').any(|n| n.parse() == Ok(null)) {
            libc::close(null);
        }
    }
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
    let ctx = context::init_thread();
    crate::diag::install_signal_handlers();
    // SAFETY: this thread's fresh context.
    let c = unsafe { &mut *ctx };
    load_context(&mut r, c);
    let tp = r.u64();
    let scs = r.bytes();
    crate::vfs::set_cwd(r.str());
    crate::vfs::load_own_mounts(&r.str());
    if let Err(e) = map_regions(&mut r, &h.entries) {
        return e;
    }
    if !state::restore(&mut r) {
        return "fork child: damaged state".into();
    }
    for (fd, cloexec) in r.seq(|r| (r.i32(), r.bool())) {
        fdtab::set_flags(fd, false, cloexec);
    }
    if !r.ok() {
        return "fork child: damaged state".into();
    }
    drop(h);
    if let Some((rd, _)) = vfork {
        // SAFETY: the parent's end of its wait pipe, inherited.
        unsafe { libc::close(rd) };
    }

    // The forking thread, now this process's main thread.
    let base = context::guest_scs();
    // SAFETY: our fresh shadow stack is larger than any parent's in use.
    unsafe { std::ptr::copy_nonoverlapping(scs.as_ptr(), base as *mut u8, scs.len()) };
    context::set_guest_scs(base + scs.len() as u64);
    context::set_guest_tp(tls.unwrap_or(tp));
    super::child_started(c, stack, set_tid, clear_tid);
    // SAFETY: this thread's context, holding the forking thread's registers.
    unsafe { context::resume(ctx) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
