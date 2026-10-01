//! Memory syscalls: mmap, munmap, mremap, mprotect, madvise, brk.
//!
//! - Executable private file mappings of translated files (and of
//!   originals with nothing to rewrite) are mapped shared and read-only,
//!   then made executable, so every process shares the same pages (Darwin
//!   refuses PROT_EXEC in the mmap itself; ADR 0012, "Platform probes").
//!   Executable private mappings of other files are materialized as
//!   anonymous memory filled with `pread` and rewritten (see `patch`).
//! - Other private mappings of regular files map the file copy-on-write,
//!   as Linux does: pages are shared with the file until written, and a
//!   write stays private. Whether a later change to the file shows is
//!   unspecified (mmap(2)): Linux shows it in pages not written yet,
//!   Darwin's copy-on-write keeps the file as it was at mmap (as the
//!   anonymous copies did). An access to a page wholly past the end of
//!   the file faults (SIGBUS), and MADV_DONTNEED reads the file again. A
//!   mapping of a file that needs rewriting is rewritten when it becomes
//!   executable ([`mprotect`]), which copies only the pages written.
//! - Shared file mappings go straight to Darwin `mmap`.
//! - A BoringSSL FIPS module rewritten at load time gets its integrity hash
//!   recomputed in memory, as the translator does for cached files.

use std::cell::RefCell;
use std::sync::Mutex;

use crate::context::GuestContext;
use crate::errno::{self, EEXIST, EINVAL, ENOMEM};
use crate::patch;
use crate::xlate::fips;
use crate::xrt::{self, ExecSource};

use std::os::unix::ffi::OsStrExt;

use super::{arena, copies, vmmap, window};

pub(super) const PAGE: u64 = 16384;

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;
const PROT_BTI: u64 = 0x10;
const PROT_MTE: u64 = 0x20;

const MAP_SHARED: u64 = 0x01;
const MAP_PRIVATE: u64 = 0x02;
const MAP_SHARED_VALIDATE: u64 = 0x03;
const MAP_TYPE: u64 = 0x0f;
const MAP_FIXED: u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;
const MAP_NORESERVE: u64 = 0x4000;
const MAP_FIXED_NOREPLACE: u64 = 0x10_0000;

fn page_up(v: u64) -> u64 {
    (v + PAGE - 1) & !(PAGE - 1)
}

fn host_prot(p: u64) -> i32 {
    (p & (PROT_READ | PROT_WRITE | PROT_EXEC)) as i32
}

fn host_mmap(addr: u64, len: u64, prot: i32, flags: i32, fd: i32, off: i64) -> Result<u64, i64> {
    // SAFETY: forwarding a validated guest request to Darwin.
    let p = unsafe { libc::mmap(addr as *mut _, len as usize, prot, flags, fd, off) };
    if p == libc::MAP_FAILED {
        Err(-(errno::last() as i64))
    } else {
        Ok(p as u64)
    }
}

fn host_mprotect(addr: u64, len: u64, prot: i32) -> i64 {
    // SAFETY: guest-requested protection change on guest memory.
    errno::check(unsafe { libc::mprotect(addr as *mut _, len as usize, prot) } as i64)
}

fn trace_stats(what: &str, addr: u64, len: u64, stats: &patch::PatchStats) {
    if crate::sys::tracing() && stats.total() > 0 {
        crate::diag!(
            "[linux-abi] {what} {:#x}..{:#x}: {} svc, {} mrs/{} msr tpidr_el0, {} scs, {} ctr_el0, {} brk fallbacks",
            addr,
            addr + len,
            stats.svc,
            stats.mrs_tp,
            stats.msr_tp,
            stats.scs,
            stats.ctr,
            stats.brk_fallback
        );
    }
}

/// Fill `[b, b+len)` from `fd` at `off`.
fn populate(b: u64, len: u64, fd: i32, off: u64) -> Result<(), i64> {
    let mut done = 0u64;
    while done < len {
        // SAFETY: b..b+len is freshly mapped RW.
        let n = unsafe {
            libc::pread(
                fd,
                (b + done) as *mut _,
                (len - done) as usize,
                (off + done) as i64,
            )
        };
        if n < 0 {
            return Err(-(errno::last() as i64));
        }
        if n == 0 {
            break;
        }
        done += n as u64;
    }
    Ok(())
}

/// Rewrite a freshly populated private file mapping (not executable yet)
/// using the file's sites, or a scan when there is no metadata.
fn rewrite_file_copy(b: u64, len: u64, fd: i32, off: u64, source: &ExecSource) {
    let stats = match source {
        ExecSource::LoadTime(Some(a)) => {
            let abs: Vec<_> = a
                .sites
                .iter()
                .filter(|&&(o, _, _)| o >= off && o + 4 <= off + len)
                .map(|&(o, kind, rt)| (b + (o - off), kind, rt))
                .collect();
            let stats = patch::rewrite_sites(&abs, b, b + len, true);
            if let Some(m) = &a.fips {
                rehash_fips(m, b, len, fd, off);
            }
            stats
        }
        // Already translated (or nothing to rewrite): a scan would only hit
        // words the translator identified as data.
        ExecSource::Shared => return,
        ExecSource::LoadTime(None) => patch::rewrite_region(b, len),
    };
    // Every file that takes this path is named, sites or not: no cache
    // had it (the image cache test counts these).
    if crate::sys::tracing() {
        let path = xrt::fd_path(fd).unwrap_or_default();
        crate::diag!(
            "[linux-abi] load-time rewrite of {} at offset {off:#x}: {} sites",
            path.display(),
            stats.total()
        );
    }
    trace_stats("rewrote", b, len, &stats);
}

/// After the copy at `b` (file offset `off`) holding the module text was
/// rewritten, store the module's new hash. The read-only data is never
/// rewritten, so it comes from the file. The hash is in `.rodata` (or, in
/// static builds, after the text), which linker64 has mapped by now: it maps
/// segments in program header order.
fn rehash_fips(m: &fips::Module, b: u64, len: u64, fd: i32, off: u64) {
    let text_len = m.text.1 - m.text.0;
    if m.text_offset < off || m.text_offset + text_len > off + len {
        return;
    }
    let text_addr = b + (m.text_offset - off);
    let rodata = match m.rodata {
        Some((lo, hi)) => {
            let mut v = vec![0u8; (hi - lo) as usize];
            // SAFETY: reading into our buffer.
            let n =
                unsafe { libc::pread(fd, v.as_mut_ptr().cast(), v.len(), m.rodata_offset as i64) };
            if n != v.len() as isize {
                return;
            }
            Some(v)
        }
        None => None,
    };
    // SAFETY: the module text lies inside the populated copy.
    let text = unsafe { std::slice::from_raw_parts(text_addr as *const u8, text_len as usize) };
    let new = fips::digest(text, rodata.as_deref());
    let hash_addr = (text_addr - m.text.0).wrapping_add(m.hash_vaddr);
    if !store_hash(hash_addr, &m.original, &new) {
        crate::diag!("[linux-abi] FIPS module hash at {hash_addr:#x} is not mapped from this file");
    }
}

/// Replace `original` with `new` at `addr` in an anonymous copy of the
/// file. False when no such copy is mapped there.
fn store_hash(addr: u64, original: &fips::Hash, new: &fips::Hash) -> bool {
    let Some((lo, hi, prot, _)) = patch::vm::region(addr) else {
        return false;
    };
    if lo > addr || hi < addr + 32 || prot & libc::PROT_READ == 0 || file_backed(addr) {
        return false;
    }
    // SAFETY: readable, inside one region.
    if unsafe { std::slice::from_raw_parts(addr as *const u8, 32) } != original {
        return false;
    }
    let page = addr & !(PAGE - 1);
    let span = page_up(addr + 32) - page;
    let opened = prot & libc::PROT_WRITE == 0;
    if opened && host_mprotect(page, span, prot | libc::PROT_WRITE) < 0 {
        return false;
    }
    // SAFETY: writable now; the 32 bytes checked above.
    unsafe { std::ptr::copy_nonoverlapping(new.as_ptr(), addr as *mut u8, 32) };
    if opened {
        host_mprotect(page, span, prot);
    }
    true
}

pub fn mmap(a: [u64; 6]) -> i64 {
    let (addr, len, prot, flags, fd, off) = (
        a[0],
        a[1],
        a[2] & !(PROT_BTI | PROT_MTE),
        a[3],
        a[4] as i32,
        a[5],
    );
    if len == 0 || off & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    if flags & MAP_ANONYMOUS == 0
        && fd >= 0
        && let Some(r) = super::binder::mmap(addr, len, prot, flags, fd)
    {
        return r;
    }
    if flags & MAP_ANONYMOUS == 0 && fd >= 0 {
        super::procfs::on_mmap(fd);
    }
    if flags & MAP_ANONYMOUS == 0
        && fd >= 0
        && let Some(e) = super::ashmem::before_mmap(fd, len, prot)
    {
        return e;
    }
    let len = page_up(len);
    let mut addr = addr;
    let mut fixed = flags & MAP_FIXED != 0;
    let noreplace = flags & MAP_FIXED_NOREPLACE != 0;
    if (fixed || noreplace) && addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    // A hint in the heap reference window takes the reserved pages there
    // exactly; a hint that cannot be honoured there is dropped, as Linux
    // places the mapping elsewhere.
    if !fixed && (window::BASE..window::BASE + window::SIZE).contains(&addr) {
        if addr & (PAGE - 1) == 0 && window::is_free(addr, len) {
            fixed = true;
        } else if noreplace {
            return -(EEXIST as i64);
        } else {
            addr = 0;
        }
    }
    // Anywhere means anywhere in the guest range; MAP_FIXED_NOREPLACE
    // wants exactly its address.
    let placed = !fixed && !noreplace;
    if placed {
        addr = arena::hint(addr, len);
    }
    let placing = placed.then(arena::placing);
    let kind = flags & MAP_TYPE;
    let anon = flags & MAP_ANONYMOUS != 0;
    let mut hflags = if fixed { libc::MAP_FIXED } else { 0 };
    if flags & MAP_NORESERVE != 0 {
        hflags |= libc::MAP_NORESERVE;
    }
    let exec = prot & PROT_EXEC != 0;
    let writable = prot & PROT_WRITE != 0;
    // Fresh RWX memory: `jit` makes it so below.
    let fresh_jit = anon && exec && writable;
    // (base, what remains to do before returning)
    enum Finish {
        Nothing,
        Protect,
    }
    let mut noted = false;
    let shared = kind == MAP_SHARED || kind == MAP_SHARED_VALIDATE;
    let (base, finish) = if shared
        && !anon
        && !noreplace
        && let Some(r) = super::sharedfile::map(fd, addr, len, prot as i32, fixed, off)
    {
        match r {
            Ok(b) => (b, Finish::Nothing),
            Err(e) => return e,
        }
    } else if shared {
        let memfd = if anon {
            None
        } else {
            match super::memfd::map_shared(fd, addr, len, host_prot(prot), fixed, off) {
                super::memfd::Shared::Done(r) => return r,
                super::memfd::Shared::File(k) => k,
            }
        };
        let f = hflags | libc::MAP_SHARED | if anon { libc::MAP_ANON } else { 0 };
        match host_mmap(
            addr,
            len,
            host_prot(prot),
            f,
            if anon { -1 } else { fd },
            off as i64,
        ) {
            Ok(b) => {
                if let Some(k) = memfd {
                    super::memfd::note_view(k, b, len, off);
                }
                (b, Finish::Nothing)
            }
            Err(e) => return e,
        }
    } else if kind == MAP_PRIVATE {
        let source = if !anon && exec && !writable {
            Some(xrt::exec_source(fd, off))
        } else {
            None
        };
        if let Some(ExecSource::Shared) = source {
            // Translated (or nothing to rewrite): the file itself, shared.
            let b = match host_mmap(
                addr,
                len,
                libc::PROT_READ,
                hflags | libc::MAP_SHARED,
                fd,
                off as i64,
            ) {
                Ok(b) => b,
                Err(e) => return e,
            };
            crate::diag::register_fd_module(b, len, fd, off);
            (b, Finish::Protect)
        } else if !anon && !exec && maps_file(fd) {
            // The file itself, copy-on-write.
            noted = true;
            match host_mmap(
                addr,
                len,
                host_prot(prot),
                hflags | libc::MAP_PRIVATE,
                fd,
                off as i64,
            ) {
                Ok(b) => (b, Finish::Nothing),
                Err(e) => return e,
            }
        } else {
            let fill = !anon;
            let initial = if fresh_jit {
                libc::PROT_NONE
            } else if fill || exec {
                libc::PROT_READ | libc::PROT_WRITE
            } else {
                host_prot(prot)
            };
            let b = match host_mmap(
                addr,
                len,
                initial,
                hflags | libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            ) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if fill {
                noted = true;
                if let Err(e) = populate(b, len, fd, off) {
                    // SAFETY: unmapping what we just mapped.
                    unsafe { libc::munmap(b as *mut _, len as usize) };
                    return e;
                }
                if exec {
                    crate::diag::register_fd_module(b, len, fd, off);
                    let source = source.unwrap_or_else(|| xrt::exec_source(fd, off));
                    rewrite_file_copy(b, len, fd, off, &source);
                }
            } else if exec && !fresh_jit {
                // Fresh anonymous code: nothing written yet, but scan anyway so
                // the path is the same as mprotect's.
                trace_stats("rewrote", b, len, &patch::rewrite_region(b, len));
            }
            (
                b,
                if fill || exec {
                    Finish::Protect
                } else {
                    Finish::Nothing
                },
            )
        }
    } else {
        return -(EINVAL as i64);
    };
    if noreplace && base != addr {
        // SAFETY: unmapping the mapping we just created at the wrong place.
        unsafe { libc::munmap(base as *mut _, len as usize) };
        return -(EEXIST as i64);
    }
    if placed && !arena::contains(base, len) {
        // SAFETY: unmapping the mapping Darwin placed outside the range.
        unsafe { libc::munmap(base as *mut _, len as usize) };
        return -(ENOMEM as i64);
    }
    drop(placing);
    match finish {
        Finish::Nothing => {}
        Finish::Protect => {
            let r = if writable && exec {
                super::jit::protect_rwx(base, len)
            } else {
                host_mprotect(base, len, host_prot(prot))
            };
            if r < 0 {
                if fresh_jit {
                    // SAFETY: unmapping the mapping we just created.
                    unsafe { libc::munmap(base as *mut _, len as usize) };
                }
                return r;
            }
        }
    }
    copies::forget(base, base + len);
    if noted {
        note_file_copy(base, len, fd, off);
    }
    base as i64
}

/// execve in place: the old image's memory goes, the whole guest range,
/// with what is recorded about it; the heap window is reserved again.
pub fn exec_reset() {
    // SAFETY: nothing of the old image is used from here on.
    unsafe { mach_vm_deallocate(task(), arena::LO, arena::HI - arena::LO) };
    window::init();
    copies::forget(arena::LO, arena::HI);
    DEFERRED_UNMAPS.with(|d| d.borrow_mut().clear());
}

/// Record that `[base, base+len)` maps (or holds a copy of) `fd` from
/// `off`.
fn note_file_copy(base: u64, len: u64, fd: i32, off: u64) {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes; fstat into a local
    // buffer.
    let st = unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) < 0 || libc::fstat(fd, &mut st) < 0 {
            return;
        }
        st
    };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let host = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n]));
    let guest = super::procfs::fd_guest_path(fd).unwrap_or_default();
    let id = (st.st_dev as u32 as u64, st.st_ino);
    copies::note_file(base, len, &host, &guest, off, id);
}

thread_local! {
    /// munmaps of this thread's own stack, run when the thread exits.
    static DEFERRED_UNMAPS: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

/// The syscall stub keeps x16, x17 and x30 in the 32 bytes below the guest
/// sp until the syscall returns. bionic's `_exit_with_stack_teardown` unmaps
/// the calling thread's own stack and then calls `exit`, so an munmap that
/// covers that frame is deferred to thread exit (ADR 0012, "Platform probes").
pub fn munmap(ctx: &GuestContext, a: [u64; 6]) -> i64 {
    let (addr, len) = (a[0], page_up(a[1]));
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    let frame_lo = ctx.sp.saturating_sub(32);
    if len > 0 && frame_lo < addr.saturating_add(len) && ctx.sp > addr {
        DEFERRED_UNMAPS.with(|d| d.borrow_mut().push((addr, len)));
        return 0;
    }
    copies::forget(addr, addr + len);
    window::unmap(addr, len)
}

/// Run the munmaps deferred by [`munmap`]; called on the host stack when the
/// thread exits.
pub fn run_deferred_unmaps() -> usize {
    DEFERRED_UNMAPS.with(|d| {
        let v = std::mem::take(&mut *d.borrow_mut());
        for &(addr, len) in &v {
            // SAFETY: the guest asked for this unmap; its thread is exiting.
            unsafe { libc::munmap(addr as *mut _, len as usize) };
        }
        v.len()
    })
}

/// Whether private mappings of `fd` can map the file itself: a regular
/// file, but not a memfd whose pages moved to anonymous memory for an
/// executable view (`memfd`). Copying a 16 MiB memfd cost 1.6 ms (#501).
fn maps_file(fd: i32) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    let regular =
        unsafe { libc::fstat(fd, &mut st) } == 0 && st.st_mode & libc::S_IFMT == libc::S_IFREG;
    regular && !super::memfd::converted(&st)
}

/// Whether the region at `addr` maps a file whose pages are never
/// rewritten: shared (writing would change the file), or a translated
/// file or one with nothing to rewrite.
fn unrewritten_file(addr: u64) -> bool {
    vmmap::region_at(addr).is_some_and(|r| {
        r.file
            .as_ref()
            .is_some_and(|(host, _, _)| r.shared || xrt::is_shared_source(host))
    })
}

/// Whether the region at `addr` is backed by a file.
fn file_backed(addr: u64) -> bool {
    let mut buf = [0u8; 16];
    // SAFETY: proc_regionfilename writes at most buf.len() bytes.
    unsafe {
        libc::proc_regionfilename(
            libc::getpid(),
            addr,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        ) > 0
    }
}

/// mprotect with PROT_EXEC: code becoming executable is rewritten first.
///
/// - Pieces that are already executable are not touched: they were rewritten
///   when they became executable, and other threads may be running them.
/// - Shared file mappings and mappings of translated files need nothing.
/// - Other pieces are not executable, so nothing runs them: they are made
///   readable if needed, scanned and rewritten (a private file mapping
///   gets its own copy of each page written), then protected.
pub fn mprotect(a: [u64; 6]) -> i64 {
    let (addr, len, prot) = (a[0], page_up(a[1]), a[2] & !(PROT_BTI | PROT_MTE));
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    if window::touches_reserved(addr, addr + len) {
        return -(ENOMEM as i64);
    }
    let rwx = prot & (PROT_WRITE | PROT_EXEC) == PROT_WRITE | PROT_EXEC;
    if prot & PROT_EXEC != 0 {
        super::memfd::before_exec_protect(addr, addr + len);
        let end = addr + len;
        let mut cur = addr;
        while cur < end {
            let Some((lo, hi, cur_prot, _)) = patch::vm::region(cur) else {
                break;
            };
            if lo >= end {
                break;
            }
            let (lo, hi) = (lo.max(cur), hi.min(end));
            // A memfd's views are ART's dual-mapped JIT cache: never scanned.
            if cur_prot & libc::PROT_EXEC == 0
                && !(rwx && vmmap::info_at(lo).is_some_and(|i| i.empty))
                && !unrewritten_file(lo)
                && super::memfd::anon_name(lo).is_none()
            {
                if cur_prot & libc::PROT_READ == 0 {
                    host_mprotect(lo, hi - lo, cur_prot | libc::PROT_READ);
                }
                trace_stats("rewrote", lo, hi - lo, &patch::rewrite_region(lo, hi - lo));
            }
            cur = hi;
        }
    }
    if rwx {
        return super::jit::protect_rwx(addr, len);
    }
    let r = host_mprotect(addr, len, host_prot(prot));
    if r == -(errno::EACCES as i64) {
        // JIT pages keep their protection.
        return super::jit::protect_around(addr, len, host_prot(prot));
    }
    r
}

const MADV_DONTNEED: u64 = 4;
const MADV_FREE: u64 = 8;
const MADV_REMOVE: u64 = 9;

use patch::vm::{
    VM_FLAGS_FIXED, VM_FLAGS_OVERWRITE, VM_INHERIT_COPY, mach_vm_deallocate, mach_vm_protect,
    mach_vm_remap, task,
};

unsafe extern "C" {
    fn mach_vm_allocate(task: libc::mach_port_t, address: *mut u64, size: u64, flags: i32) -> i32;
    fn mach_vm_inherit(task: libc::mach_port_t, address: u64, size: u64, inheritance: u32) -> i32;
    fn mach_vm_read_overwrite(
        task: libc::mach_port_t,
        address: u64,
        size: u64,
        data: u64,
        out_size: *mut u64,
    ) -> i32;
}

const VM_INHERIT_SHARE: u32 = 0;

/// Map the pages of `[src, src+len)` at `dst` too (replacing what is
/// there), sharing them; protections and inheritance follow the source.
pub(super) fn remap_shared(dst: u64, src: u64, len: u64) -> Result<(), i64> {
    let (mut addr, mut cur, mut max) = (dst, 0, 0);
    // SAFETY: remapping guest memory within our own task.
    let kr = unsafe {
        mach_vm_remap(
            task(),
            &mut addr,
            len,
            0,
            VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
            task(),
            src,
            0,
            &mut cur,
            &mut max,
            VM_INHERIT_COPY,
        )
    };
    if kr != 0 {
        return Err(-(ENOMEM as i64));
    }
    // mach_vm_remap sets the inheritance given; keep the source's sharing.
    for r in vmmap::regions(src, src + len) {
        if r.shared {
            inherit_shared(dst + (r.start - src), r.end - r.start);
        }
    }
    Ok(())
}

/// Make fork children share `[addr, addr+len)` instead of copying it.
pub(super) fn inherit_shared(addr: u64, len: u64) {
    // SAFETY: changing the inheritance of our own mapping.
    unsafe { mach_vm_inherit(task(), addr, len, VM_INHERIT_SHARE) };
}

/// Fresh anonymous memory anywhere in the guest range.
pub(super) fn allocate(len: u64) -> Result<u64, i64> {
    arena::map_anon(len, libc::PROT_READ | libc::PROT_WRITE)
}

/// Linux MADV_DONTNEED on private memory must read back zeros (anonymous)
/// or the file's contents (file-backed); Darwin's does not guarantee it.
/// Replace each private, non-executable piece with fresh memory of the
/// same protection. Shared mappings keep their contents, as on Linux.
fn discard_private(addr: u64, len: u64) -> i64 {
    for r in vmmap::regions(addr, addr + len) {
        let prot = r.prot as i32 & (libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC);
        if r.shared || prot & libc::PROT_EXEC != 0 || prot == 0 {
            continue;
        }
        let (lo, n) = (r.start, r.end - r.start);
        let fresh = match &r.file {
            Some((path, _, _)) => {
                let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
                    continue;
                };
                // SAFETY: reopening the region's own backing file.
                let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
                if fd < 0 {
                    continue;
                }
                // A fork child's copy reports offset 0 (`copies`).
                let offset = copies::find(lo).map_or(r.offset, |(_, off)| off);
                let m = host_mmap(
                    lo,
                    n,
                    prot,
                    libc::MAP_FIXED | libc::MAP_PRIVATE,
                    fd,
                    offset as i64,
                );
                // SAFETY: our temporary fd.
                unsafe { libc::close(fd) };
                m
            }
            None => host_mmap(
                lo,
                n,
                prot,
                libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            ),
        };
        if let Err(e) = fresh {
            return e;
        }
        if r.file.is_none() {
            refill_copy(lo, n, prot);
        }
    }
    0
}

/// Reload the file contents of copied pages in fresh `[lo, lo+n)`.
fn refill_copy(lo: u64, n: u64, prot: i32) {
    let mut cur = lo;
    while cur < lo + n {
        let Some((c, off)) = copies::find(cur) else {
            cur += PAGE;
            continue;
        };
        let end = c.end.min(lo + n);
        let Ok(p) = std::ffi::CString::new(c.host.as_os_str().as_bytes()) else {
            cur = end;
            continue;
        };
        // SAFETY: refilling our fresh private pages from the file they copy.
        unsafe {
            let fd = libc::open(p.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if fd >= 0 {
                if prot & libc::PROT_WRITE == 0 {
                    libc::mprotect(cur as *mut _, (end - cur) as usize, prot | libc::PROT_WRITE);
                }
                let _ = populate(cur, end - cur, fd, off);
                if prot & libc::PROT_WRITE == 0 {
                    libc::mprotect(cur as *mut _, (end - cur) as usize, prot);
                }
                libc::close(fd);
            }
        }
        cur = end;
    }
}

pub fn madvise(a: [u64; 6]) -> i64 {
    let (addr, len, advice) = (a[0], page_up(a[1]), a[2]);
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    match advice {
        MADV_DONTNEED | MADV_REMOVE => discard_private(addr, len),
        MADV_FREE => {
            // SAFETY: advisory only.
            unsafe { libc::madvise(addr as *mut _, len as usize, libc::MADV_FREE) };
            0
        }
        _ => 0,
    }
}

const MREMAP_MAYMOVE: u64 = 1;
const MREMAP_FIXED: u64 = 2;
const MREMAP_DONTUNMAP: u64 = 4;

/// Map `[addr, addr+len)` as the continuation of the mapping whose last
/// page is `like`, as mremap(2) grows it: the file's following pages with
/// the same sharing and protection for a file mapping, fresh anonymous
/// memory otherwise. `addr` must be free unless `replace`.
fn extend_at(addr: u64, len: u64, like: u64, replace: bool) -> Result<(), i64> {
    let replace = replace || window::is_free(addr, len);
    let mut at = addr;
    let flags = VM_FLAGS_FIXED | if replace { VM_FLAGS_OVERWRITE } else { 0 };
    // SAFETY: allocating in our own task; FIXED without OVERWRITE fails
    // rather than replacing anything.
    if unsafe { mach_vm_allocate(task(), &mut at, len, flags) } != 0 {
        return Err(-(ENOMEM as i64));
    }
    let Some(r) = vmmap::region_at(like).filter(|r| r.start <= like) else {
        return Ok(());
    };
    let prot = r.prot as u64 & (PROT_READ | PROT_WRITE | PROT_EXEC);
    // A private file mapping (or a copy standing in for one) is recorded
    // with its file and offset, which a fork child's copy does not report.
    // Executable file views not recorded are translated code the guest
    // mapped private, from a cache file ([`mmap`]).
    let copy = copies::find(like);
    let file = match (&copy, &r.file) {
        (Some((c, off)), _) => Some((c.host.clone(), off + PAGE, false)),
        (None, Some((host, _, _))) if prot & PROT_EXEC == 0 => {
            Some((host.clone(), r.offset + (like - r.start) + PAGE, r.shared))
        }
        _ => None,
    };
    let Some((host, off, shared)) = file else {
        // SAFETY: our fresh allocation.
        unsafe { mach_vm_protect(task(), addr, len, 0, prot as i32) };
        return Ok(());
    };
    let mapped = map_file_at(addr, len, prot, &host, off, shared);
    if let Err(e) = mapped {
        window::unmap(addr, len);
        return Err(e);
    }
    if let Some((c, _)) = copy {
        copies::note_file(addr, len, &c.host, &c.guest, off, (c.dev, c.ino));
    }
    Ok(())
}

/// Map `host` from `off` at `addr` (replacing what is there) as the guest's
/// mmap would.
fn map_file_at(
    addr: u64,
    len: u64,
    prot: u64,
    host: &std::path::Path,
    off: u64,
    shared: bool,
) -> Result<(), i64> {
    let c = std::ffi::CString::new(host.as_os_str().as_bytes()).map_err(|_| -(ENOMEM as i64))?;
    let mode = if shared && prot & PROT_WRITE != 0 {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    // SAFETY: reopening the mapping's own backing file.
    let fd = unsafe { libc::open(c.as_ptr(), mode | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(-(errno::last() as i64));
    }
    let kind = if shared { MAP_SHARED } else { MAP_PRIVATE };
    let r = mmap([addr, len, prot, kind | MAP_FIXED, fd as u64, off]);
    // SAFETY: our temporary fd.
    unsafe { libc::close(fd) };
    if r < 0 { Err(r) } else { Ok(()) }
}

/// Move `[old, old+old_len)` to `new` (resized to `new_len`), replacing
/// what is at the destination.
fn move_to(old: u64, old_len: u64, new: u64, new_len: u64, keep_old: bool) -> Result<(), i64> {
    let moved = old_len.min(new_len);
    remap_shared(new, old, moved)?;
    super::memfd::note_alias(old, new, moved);
    copies::moved(old, new, moved);
    if new_len > old_len {
        extend_at(new + old_len, new_len - old_len, new + old_len - PAGE, true)?;
    }
    if keep_old {
        // MREMAP_DONTUNMAP: the old range stays mapped, empty.
        discard_private(old, old_len);
    } else {
        // The old pages now live at `new`.
        window::unmap(old, old_len);
    }
    Ok(())
}

pub fn mremap(a: [u64; 6]) -> i64 {
    let (old, old_len, new_len, flags, new_addr) = (a[0], page_up(a[1]), page_up(a[2]), a[3], a[4]);
    if old & (PAGE - 1) != 0
        || flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0
        || (flags & (MREMAP_FIXED | MREMAP_DONTUNMAP) != 0 && flags & MREMAP_MAYMOVE == 0)
        || new_len == 0
    {
        return -(EINVAL as i64);
    }
    if flags & MREMAP_DONTUNMAP != 0 && old_len != new_len {
        return -(EINVAL as i64);
    }
    if vmmap::region_at(old).is_none_or(|r| r.start > old) {
        return -(libc::EFAULT as i64);
    }
    let keep_old = flags & MREMAP_DONTUNMAP != 0;
    if flags & MREMAP_FIXED != 0 {
        if new_addr & (PAGE - 1) != 0 || (new_addr < old + old_len && old < new_addr + new_len) {
            return -(EINVAL as i64);
        }
        return match move_to(old, old_len, new_addr, new_len, keep_old) {
            Ok(()) => new_addr as i64,
            Err(e) => e,
        };
    }
    if old_len == 0 {
        // Linux: a second mapping of the same (shared) pages.
        let at = match allocate(new_len) {
            Ok(a) => a,
            Err(e) => return e,
        };
        return match remap_shared(at, old, new_len) {
            Ok(()) => {
                super::memfd::note_alias(old, at, new_len);
                at as i64
            }
            Err(e) => e,
        };
    }
    if !keep_old && new_len <= old_len {
        if new_len < old_len {
            copies::forget(old + new_len, old + old_len);
            window::unmap(old + new_len, old_len - new_len);
        }
        return old as i64;
    }
    if !keep_old
        && extend_at(
            old + old_len,
            new_len - old_len,
            old + old_len - PAGE,
            false,
        )
        .is_ok()
    {
        return old as i64;
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return -(ENOMEM as i64);
    }
    let at = match allocate(new_len) {
        Ok(a) => a,
        Err(e) => return e,
    };
    match move_to(old, old_len, at, new_len, keep_old) {
        Ok(()) => at as i64,
        Err(e) => {
            // SAFETY: releasing our reservation.
            unsafe { mach_vm_deallocate(task(), at, new_len) };
            e
        }
    }
}

pub fn mincore(a: [u64; 6]) -> i64 {
    let (addr, len, vec) = (a[0], page_up(a[1]), a[2]);
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    if vmmap::regions(addr, addr + len)
        .map(|r| r.end - r.start)
        .sum::<u64>()
        != len
    {
        return -(ENOMEM as i64);
    }
    // SAFETY: guest vector of len/PAGE bytes.
    let r = unsafe { libc::mincore(addr as *const _, len as usize, vec as *mut libc::c_char) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    for i in 0..(len / PAGE) as usize {
        // SAFETY: inside the guest vector; Linux reports only residency.
        unsafe { *(vec as *mut u8).add(i) &= 1 };
    }
    0
}

pub fn msync(a: [u64; 6]) -> i64 {
    const MS_ASYNC: u64 = 1;
    const MS_INVALIDATE: u64 = 2;
    const MS_SYNC: u64 = 4;
    let (addr, len, flags) = (a[0], a[1], a[2]);
    if addr & (PAGE - 1) != 0
        || flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0
        || flags & (MS_ASYNC | MS_SYNC) == (MS_ASYNC | MS_SYNC)
    {
        return -(EINVAL as i64);
    }
    if window::touches_reserved(addr, addr + page_up(len)) {
        return -(ENOMEM as i64);
    }
    let mut h = 0;
    if flags & MS_ASYNC != 0 {
        h |= libc::MS_ASYNC;
    }
    if flags & MS_SYNC != 0 {
        h |= libc::MS_SYNC;
    }
    if flags & MS_INVALIDATE != 0 {
        h |= libc::MS_INVALIDATE;
    }
    // SAFETY: guest range.
    errno::check(unsafe { libc::msync(addr as *mut _, page_up(len) as usize, h) } as i64)
}

pub fn mlock(nr: u64, a: [u64; 6]) -> i64 {
    // SAFETY: guest range.
    let r = unsafe {
        if nr == 229 {
            libc::munlock(a[0] as *const _, a[1] as usize)
        } else {
            libc::mlock(a[0] as *const _, a[1] as usize)
        }
    };
    errno::check(r as i64)
}

const MEMBARRIER_CMD_QUERY: u64 = 0;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED: u64 = 1 << 3;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: u64 = 1 << 4;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE: u64 = 1 << 5;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE: u64 = 1 << 6;

unsafe extern "C" {
    fn thread_get_register_pointer_values(
        thread: u32,
        sp: *mut u64,
        length: *mut usize,
        values: *mut u64,
    ) -> i32;
}

/// Serialize the process's other running threads, as Linux interrupts
/// only the cores that run them: reading a thread's registers makes the
/// kernel interrupt it, which is a full barrier and a context
/// synchronization on its core (as .NET does on macOS arm64). A thread
/// that is not running takes the scheduler's thread lock, which reading
/// its state also takes, and an exception return before it runs again;
/// the fence puts the caller's accesses before that read, as Linux's
/// smp_mb before it looks at the run queues.
fn barrier_all_threads() {
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    super::thread::each_running_other(|t| {
        let (mut sp, mut n, mut regs) = (0u64, 128usize, [0u64; 128]);
        // SAFETY: a live thread of this task; results into locals.
        unsafe { thread_get_register_pointer_values(t, &mut sp, &mut n, regs.as_mut_ptr()) };
    });
}

pub fn membarrier(a: [u64; 6]) -> i64 {
    let (cmd, flags) = (a[0], a[1]);
    if flags != 0 {
        return -(EINVAL as i64);
    }
    match cmd {
        MEMBARRIER_CMD_QUERY => {
            (MEMBARRIER_CMD_PRIVATE_EXPEDITED
                | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
                | MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE
                | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE) as i64
        }
        MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
        | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE => 0,
        MEMBARRIER_CMD_PRIVATE_EXPEDITED | MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE => {
            barrier_all_threads();
            0
        }
        _ => -(EINVAL as i64),
    }
}

/// process_vm_readv/writev: on this process here, on another through its
/// agent (`ptrace`). `pid` may name any thread of the process.
pub fn process_vm_rw(write: bool, a: [u64; 6]) -> i64 {
    let (pid, local, lcnt, remote, rcnt, flags) = (a[0] as i32, a[1], a[2], a[3], a[4], a[5]);
    if flags != 0 || lcnt > 1024 || rcnt > 1024 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest iovec arrays of the given counts.
    let iov = |p: u64, n: u64| unsafe {
        std::slice::from_raw_parts(p as *const libc::iovec, n as usize)
            .iter()
            .map(|v| (v.iov_base as u64, v.iov_len as u64))
            .filter(|v| v.1 > 0)
            .collect::<Vec<_>>()
    };
    let (l, r) = (iov(local, lcnt), iov(remote, rcnt));
    let owner = super::thread::owner(pid);
    // SAFETY: trivial.
    if pid <= 0 || owner != unsafe { libc::getpid() } {
        return super::ptrace::remote_vm(write, owner, &l, &r);
    }
    let (mut li, mut lo, mut done) = (0usize, 0u64, 0u64);
    for (rbase, rlen) in r {
        let mut ro = 0u64;
        while ro < rlen && li < l.len() {
            let n = (rlen - ro).min(l[li].1 - lo);
            let (src, dst) = if write {
                (l[li].0 + lo, rbase + ro)
            } else {
                (rbase + ro, l[li].0 + lo)
            };
            let mut got = 0u64;
            // SAFETY: a fault-safe copy within our own task.
            if unsafe { mach_vm_read_overwrite(task(), src, n, dst, &mut got) } != 0 {
                return if done > 0 {
                    done as i64
                } else {
                    -(libc::EFAULT as i64)
                };
            }
            done += n;
            ro += n;
            lo += n;
            if lo == l[li].1 {
                li += 1;
                lo = 0;
            }
        }
    }
    done as i64
}

struct Brk {
    base: u64,
    cur: u64,
    mapped: u64,
    limit: u64,
}

static BRK: Mutex<Brk> = Mutex::new(Brk {
    base: 0,
    cur: 0,
    mapped: 0,
    limit: 0,
});
const BRK_RESERVE: u64 = 64 << 20;

/// Reserve the program break area near the end of the main executable.
pub fn init_brk(hint: u64) {
    let base = host_mmap(
        page_up(hint),
        BRK_RESERVE,
        libc::PROT_NONE,
        libc::MAP_PRIVATE | libc::MAP_ANON,
        -1,
        0,
    )
    .unwrap_or(0);
    let mut b = BRK.lock().unwrap();
    *b = Brk {
        base,
        cur: base,
        mapped: base,
        limit: base + BRK_RESERVE,
    };
}

pub fn brk(a: [u64; 6]) -> i64 {
    let want = a[0];
    let mut b = BRK.lock().unwrap();
    if want == 0 || want < b.base || want > b.limit {
        return b.cur as i64;
    }
    let need = page_up(want);
    if need > b.mapped {
        if host_mprotect(
            b.mapped,
            need - b.mapped,
            libc::PROT_READ | libc::PROT_WRITE,
        ) != 0
        {
            return -(ENOMEM as i64);
        }
        b.mapped = need;
    }
    b.cur = want;
    b.cur as i64
}

/// Fork: the program break (its pages come with the guest's memory).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let b = BRK.lock().unwrap();
    for v in [b.base, b.cur, b.mapped, b.limit] {
        w.u64(v);
    }
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let v = [(); 4].map(|_| r.u64());
    *BRK.lock().unwrap() = Brk {
        base: v[0],
        cur: v[1],
        mapped: v[2],
        limit: v[3],
    };
}
