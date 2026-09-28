//! Application JITs: memory that is writable and executable at once
//! (ADR 0012, "Application JITs"). V8 reserves its code range PROT_NONE and
//! makes it RWX with `mprotect`; other JITs `mmap` RWX memory directly.
//!
//! Darwin gives RWX only to `MAP_JIT` memory, where each thread sees the
//! pages either writable or executable (`pthread_jit_write_protect_np`).
//! A `MAP_JIT` mapping cannot be made at a fixed address, and once its
//! pages are RWX their protection cannot change again (EACCES). So:
//!
//! - RWX pages are made by replacing their mapping with a `MAP_JIT` one at
//!   the same address: unmap, then map with the address as a hint (Darwin
//!   places a non-fixed mapping first-fit from its hint) while no other
//!   guest mapping is being placed ([`arena::placing`]). Contents are kept.
//! - An `mprotect` of RWX pages to anything else replaces them with plain
//!   memory holding the same contents, made elsewhere and remapped over
//!   them in one step. `munmap`, `mmap` over them and `madvise` work as
//!   usual.
//! - A thread that writes the pages while they are executable for it faults
//!   and is switched to writable. One that runs code there while they are
//!   writable faults and is switched back to executable. Darwin restores the
//!   writable state when a signal handler returns, so that switch happens
//!   outside the handler: the handler saves the registers, sends the thread
//!   to [`exec_and_resume`] on its host stack, and the resume trap loads the
//!   registers again (as `rt_sigreturn` does).
//!
//! Only `MAP_JIT` memory can be RWX on Darwin, so RWX in the VM map is how
//! these pages are recognized.

use crate::context::{self, GuestContext};
use crate::errno::{self, ENOMEM};
use crate::patch::{self, vm};

use super::sigframe::{Cpu, DarwinMcontext};
use super::{arena, vmmap};

const RWX: i32 = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;

unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
}

/// Whether the page at `addr` is JIT memory (RWX).
fn is_jit(addr: u64) -> bool {
    matches!(patch::vm::region(addr), Some((lo, _, prot, _)) if lo <= addr && prot & RWX == RWX)
}

/// Make `[addr, addr+len)` RWX, keeping its contents. Every page must be
/// mapped (ENOMEM otherwise, as Linux's mprotect).
pub fn protect_rwx(addr: u64, len: u64) -> i64 {
    let end = addr + len;
    let _placing = arena::placing_exclusive();
    let mut cur = addr;
    while cur < end {
        let Some(i) = vmmap::info_at(cur) else {
            return -(ENOMEM as i64);
        };
        if i.start > cur {
            return -(ENOMEM as i64);
        }
        let hi = i.end.min(end);
        if i.prot as i32 & RWX != RWX
            && let Err(e) = replace(cur, hi - cur, i.prot as i32, !i.empty)
        {
            return e;
        }
        cur = hi;
    }
    0
}

/// Replace `[lo, lo+n)` (protection `prot`) with RWX `MAP_JIT` memory, and
/// copy its contents over when it has any.
fn replace(lo: u64, n: u64, prot: i32, keep: bool) -> Result<(), i64> {
    let mut saved = None;
    if keep {
        // SAFETY: a scratch mapping of our own.
        let tmp = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                n as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if tmp == libc::MAP_FAILED {
            return Err(-(ENOMEM as i64));
        }
        // SAFETY: our own guest pages, into the scratch mapping.
        unsafe {
            if prot & libc::PROT_READ == 0 {
                libc::mprotect(lo as *mut _, n as usize, prot | libc::PROT_READ);
            }
            std::ptr::copy_nonoverlapping(lo as *const u8, tmp as *mut u8, n as usize);
        }
        saved = Some(tmp as *const u8);
    }
    let r = map_at(lo, n, saved);
    if r.is_err() {
        // Put the pages back as they were.
        // SAFETY: our own guest range, which `map_at` left unmapped.
        unsafe {
            let back = libc::mmap(
                lo as *mut _,
                n as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED,
                -1,
                0,
            );
            if back != libc::MAP_FAILED {
                if let Some(t) = saved {
                    std::ptr::copy_nonoverlapping(t, lo as *mut u8, n as usize);
                }
                libc::mprotect(lo as *mut _, n as usize, prot);
            }
        }
    }
    if let Some(t) = saved {
        // SAFETY: the scratch mapping.
        unsafe { libc::munmap(t as *mut _, n as usize) };
    }
    r
}

/// Map RWX `MAP_JIT` memory at `[lo, lo+n)` in place of whatever is there,
/// filled from `src` if given (a fork child copies its parent's this way).
/// The caller keeps other placements out (a fork child has no others yet).
pub fn map_at(lo: u64, n: u64, src: Option<*const u8>) -> Result<(), i64> {
    // SAFETY (all blocks): our own guest range.
    unsafe { libc::munmap(lo as *mut _, n as usize) };
    let p = unsafe {
        libc::mmap(
            lo as *mut _,
            n as usize,
            RWX,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_JIT,
            -1,
            0,
        )
    };
    if p as u64 != lo {
        crate::diag!("[linux-abi] jit: cannot map {lo:#x}..{:#x} RWX", lo + n);
        if p == libc::MAP_FAILED {
            return Err(-(errno::last() as i64));
        }
        unsafe { libc::munmap(p, n as usize) };
        return Err(-(ENOMEM as i64));
    }
    if let Some(src) = src {
        unsafe {
            libc::pthread_jit_write_protect_np(0);
            std::ptr::copy_nonoverlapping(src, lo as *mut u8, n as usize);
            libc::pthread_jit_write_protect_np(1);
            sys_icache_invalidate(lo as *mut _, n as usize);
        }
    }
    Ok(())
}

/// mprotect to `prot` (not RWX) over a range with JIT pages, whose
/// protection Darwin will not change: they become plain memory again.
pub fn protect_around(addr: u64, len: u64, prot: i32) -> i64 {
    let end = addr + len;
    let mut cur = addr;
    while cur < end {
        let Some(i) = vmmap::info_at(cur) else {
            return -(ENOMEM as i64);
        };
        if i.start > cur {
            return -(ENOMEM as i64);
        }
        let hi = i.end.min(end);
        let r = if i.prot as i32 & RWX == RWX {
            unjit(cur, hi - cur, prot, !i.empty)
        } else {
            // SAFETY: guest-requested protection change on guest memory.
            errno::check(unsafe { libc::mprotect(cur as *mut _, (hi - cur) as usize, prot) } as i64)
        };
        if r < 0 {
            return r;
        }
        cur = hi;
    }
    0
}

/// Replace JIT pages `[lo, lo+n)` with plain memory of protection `prot`
/// and the same contents, in one step (other threads may be running the
/// code): the copy is made elsewhere and remapped over them.
fn unjit(lo: u64, n: u64, prot: i32, keep: bool) -> i64 {
    // SAFETY (all blocks): a scratch mapping of our own, then our own
    // guest pages.
    let tmp = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            n as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if tmp == libc::MAP_FAILED {
        return -(ENOMEM as i64);
    }
    unsafe {
        if keep {
            std::ptr::copy_nonoverlapping(lo as *const u8, tmp as *mut u8, n as usize);
        }
        libc::mprotect(tmp, n as usize, prot);
    }
    let (mut at, mut cur, mut max) = (lo, 0, 0);
    let kr = unsafe {
        vm::mach_vm_remap(
            vm::task(),
            &mut at,
            n,
            0,
            vm::VM_FLAGS_FIXED | vm::VM_FLAGS_OVERWRITE,
            vm::task(),
            tmp as u64,
            0,
            &mut cur,
            &mut max,
            vm::VM_INHERIT_COPY,
        )
    };
    unsafe { libc::munmap(tmp, n as usize) };
    if kr != 0 {
        return -(ENOMEM as i64);
    }
    if keep && prot & libc::PROT_EXEC != 0 {
        unsafe { sys_icache_invalidate(lo as *mut _, n as usize) };
    }
    0
}

/// A write to JIT memory while it is executable for this thread, or a jump
/// into it while it is writable: switch the thread and retry. Called first
/// for every fault signal; false when the fault is not one of these.
pub fn fault(ctx: *mut GuestContext, m: &mut DarwinMcontext) -> bool {
    let ec = m.esr >> 26;
    let data_write = (ec == 0x24 || ec == 0x25) && m.esr & (1 << 6) != 0;
    let insn = ec == 0x20 || ec == 0x21;
    if !(data_write || insn) || !is_jit(m.far) {
        return false;
    }
    if data_write {
        // The interrupted state was executable: a change here lasts.
        // SAFETY: switches this thread's view of JIT memory.
        unsafe { libc::pthread_jit_write_protect_np(0) };
        return true;
    }
    // Only guest code runs in JIT memory.
    // SAFETY: this thread's context, if any.
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return false;
    };
    if ctx.in_host != 0 {
        return false;
    }
    Cpu::from_mc(m).to_ctx(ctx);
    ctx.in_host = 1;
    m.pc = exec_and_resume as usize as u64;
    m.sp = ctx.host_sp;
    m.fp = 0;
    m.lr = 0;
    true
}

/// Make JIT memory executable for this thread, then resume the registers
/// [`fault`] saved.
extern "C" fn exec_and_resume() -> ! {
    // SAFETY: switches this thread's view of JIT memory.
    unsafe { libc::pthread_jit_write_protect_np(1) };
    context::resume_trap()
}
