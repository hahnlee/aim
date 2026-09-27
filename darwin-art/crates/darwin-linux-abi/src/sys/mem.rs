//! Memory syscalls: mmap, munmap, mprotect, madvise, brk.
//!
//! - Private file mappings of translated files (and of originals with
//!   nothing to rewrite) stay file-backed. Executable ones are mapped shared
//!   and read-only, then made executable, so every process shares the same
//!   pages (Darwin refuses PROT_EXEC in the mmap itself; experiments/p0/07).
//! - Private file mappings of other files are materialized as anonymous
//!   memory filled with `pread`, and rewritten (see `patch`) before they
//!   become executable.
//! - Shared file mappings go straight to Darwin `mmap`.

use std::cell::RefCell;
use std::sync::Mutex;

use crate::context::GuestContext;
use crate::errno::{self, EEXIST, EINVAL, ENOMEM};
use crate::patch;
use crate::xrt::{self, ExecSource};

const PAGE: u64 = 16384;

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
        eprintln!(
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
fn rewrite_file_copy(b: u64, len: u64, off: u64, source: &ExecSource) {
    let stats = match source {
        ExecSource::LoadTime(Some(sites)) => {
            let abs: Vec<_> = sites
                .iter()
                .filter(|s| s.offset >= off && s.offset + 4 <= off + len)
                .map(|s| (b + (s.offset - off), s.kind, s.rt))
                .collect();
            patch::rewrite_sites(&abs, b, b + len, true)
        }
        // Already translated (or nothing to rewrite): a scan would only hit
        // words the translator identified as data.
        ExecSource::Shared => return,
        ExecSource::LoadTime(None) => patch::rewrite_region(b, len),
    };
    trace_stats("rewrote", b, len, &stats);
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
    let len = page_up(len);
    let fixed = flags & MAP_FIXED != 0;
    let noreplace = flags & MAP_FIXED_NOREPLACE != 0;
    if (fixed || noreplace) && addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    let kind = flags & MAP_TYPE;
    let anon = flags & MAP_ANONYMOUS != 0;
    let mut hflags = if fixed { libc::MAP_FIXED } else { 0 };
    if flags & MAP_NORESERVE != 0 {
        hflags |= libc::MAP_NORESERVE;
    }
    let exec = prot & PROT_EXEC != 0;
    let writable = prot & PROT_WRITE != 0;
    // (base, what remains to do before returning)
    enum Finish {
        Nothing,
        Protect,
    }
    let (base, finish) = if kind == MAP_SHARED || kind == MAP_SHARED_VALIDATE {
        let f = hflags | libc::MAP_SHARED | if anon { libc::MAP_ANON } else { 0 };
        match host_mmap(
            addr,
            len,
            host_prot(prot),
            f,
            if anon { -1 } else { fd },
            off as i64,
        ) {
            Ok(b) => (b, Finish::Nothing),
            Err(e) => return e,
        }
    } else if kind == MAP_PRIVATE {
        let source = if !anon && exec && !writable {
            Some(xrt::exec_source(fd))
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
        } else if !anon && !exec && xrt::is_shared_source(fd) {
            // Data and read-only segments of such files: file-backed COW.
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
            let initial = if fill || exec {
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
                if let Err(e) = populate(b, len, fd, off) {
                    // SAFETY: unmapping what we just mapped.
                    unsafe { libc::munmap(b as *mut _, len as usize) };
                    return e;
                }
                if exec {
                    crate::diag::register_fd_module(b, len, fd, off);
                    let source = source.unwrap_or_else(|| xrt::exec_source(fd));
                    rewrite_file_copy(b, len, off, &source);
                }
            } else if exec {
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
    match finish {
        Finish::Nothing => {}
        Finish::Protect => {
            let r = host_mprotect(base, len, host_prot(prot));
            if r < 0 {
                return r;
            }
        }
    }
    base as i64
}

thread_local! {
    /// munmaps of this thread's own stack, run when the thread exits.
    static DEFERRED_UNMAPS: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

/// The syscall stub keeps x16, x17 and x30 in the 32 bytes below the guest
/// sp until the syscall returns. bionic's `_exit_with_stack_teardown` unmaps
/// the calling thread's own stack and then calls `exit`, so an munmap that
/// covers that frame is deferred to thread exit (experiments/p0/02).
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
    // SAFETY: guest-requested unmap of guest memory.
    errno::check(unsafe { libc::munmap(addr as *mut _, len as usize) } as i64)
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
/// - File-backed pieces are translated files (or need nothing).
/// - Other pieces are not executable, so nothing runs them: they are made
///   readable if needed, scanned and rewritten, then protected.
pub fn mprotect(a: [u64; 6]) -> i64 {
    let (addr, len, prot) = (a[0], page_up(a[1]), a[2] & !(PROT_BTI | PROT_MTE));
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    if prot & PROT_EXEC != 0 {
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
            if cur_prot & libc::PROT_EXEC == 0 && !file_backed(lo) {
                if cur_prot & libc::PROT_READ == 0 {
                    host_mprotect(lo, hi - lo, cur_prot | libc::PROT_READ);
                }
                trace_stats("rewrote", lo, hi - lo, &patch::rewrite_region(lo, hi - lo));
            }
            cur = hi;
        }
    }
    host_mprotect(addr, len, host_prot(prot))
}

const MADV_DONTNEED: u64 = 4;
const MADV_FREE: u64 = 8;
const MADV_REMOVE: u64 = 9;

unsafe extern "C" {
    static mach_task_self_: libc::mach_port_t;
    fn mach_vm_region(
        task: libc::mach_port_t,
        address: *mut u64,
        size: *mut u64,
        flavor: i32,
        info: *mut i32,
        count: *mut u32,
        object_name: *mut libc::mach_port_t,
    ) -> i32;
}

/// Linux MADV_DONTNEED on private memory must read back zeros; Darwin's
/// does not guarantee that. Replace each private, non-executable piece of
/// the range with fresh anonymous memory of the same protection.
fn zero_private(addr: u64, len: u64) -> i64 {
    let end = addr + len;
    let mut cur = addr;
    while cur < end {
        let mut raddr = cur;
        let mut rsize = 0u64;
        let mut info = [0i32; 9];
        let mut count = 9u32;
        let mut obj = 0;
        // SAFETY: VM_REGION_BASIC_INFO_64 (9) query into a 9-int buffer.
        let kr = unsafe {
            mach_vm_region(
                mach_task_self_,
                &mut raddr,
                &mut rsize,
                9,
                info.as_mut_ptr(),
                &mut count,
                &mut obj,
            )
        };
        if kr != 0 || raddr >= end {
            break;
        }
        let lo = raddr.max(cur);
        let hi = (raddr + rsize).min(end);
        let prot = info[0];
        let shared = info[3] != 0;
        let replace = !shared && prot & libc::PROT_EXEC == 0 && prot != 0;
        if replace
            && let Err(e) = host_mmap(
                lo,
                hi - lo,
                prot,
                libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        {
            return e;
        }
        cur = hi;
    }
    0
}

pub fn madvise(a: [u64; 6]) -> i64 {
    let (addr, len, advice) = (a[0], page_up(a[1]), a[2]);
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    match advice {
        MADV_DONTNEED | MADV_REMOVE => zero_private(addr, len),
        MADV_FREE => {
            // SAFETY: advisory only.
            unsafe { libc::madvise(addr as *mut _, len as usize, libc::MADV_FREE) };
            0
        }
        _ => 0,
    }
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
