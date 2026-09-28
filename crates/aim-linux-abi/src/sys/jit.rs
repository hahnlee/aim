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
//! - Code in JIT memory that writes JIT memory (self-modifying code, such
//!   as Widevine's self-decrypting code) cannot take that path: switched to
//!   writable, the thread could not fetch the store, and switched back, the
//!   store faults again. The handler does such a store itself and moves
//!   the thread past it, so the thread never leaves the executable view.
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
/// into it while it is writable: switch the thread and retry. A store whose
/// own instruction is in JIT memory (self-modifying code) is done here
/// instead: the thread could not fetch it while the pages are writable for
/// it, so switching would fault back and forth forever. Called first for
/// every fault signal; false when the fault is not one of these.
pub fn fault(ctx: *mut GuestContext, m: &mut DarwinMcontext) -> bool {
    let ec = m.esr >> 26;
    let data_write = (ec == 0x24 || ec == 0x25) && m.esr & (1 << 6) != 0;
    let insn = ec == 0x20 || ec == 0x21;
    if !(data_write || insn) {
        return false;
    }
    let Some(region) = jit_region(m.far) else {
        return false;
    };
    if data_write {
        if store_from_jit(m, region) {
            return true;
        }
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

/// The bounds of the JIT mapping holding `addr`.
fn jit_region(addr: u64) -> Option<(u64, u64)> {
    match patch::vm::region(addr) {
        Some((lo, hi, prot, _)) if lo <= addr && prot & RWX == RWX => Some((lo, hi)),
        _ => None,
    }
}

/// Performs the faulting store at `m.pc` when that instruction is in JIT
/// memory and writes only JIT memory (`region` holds the fault address),
/// and moves the thread past it.
fn store_from_jit(m: &mut DarwinMcontext, region: (u64, u64)) -> bool {
    let inside = |(lo, hi): (u64, u64), a: u64, n: u64| lo <= a && a + n <= hi;
    let in_jit =
        |a: u64, n: u64| inside(region, a, n) || jit_region(a).is_some_and(|r| inside(r, a, n));
    if !in_jit(m.pc, 4) {
        return false;
    }
    // SAFETY: JIT memory is readable in either state.
    let insn = unsafe { (m.pc as *const u32).read() };
    let mut x = [0u64; 31];
    x[..29].copy_from_slice(&m.x);
    x[29] = m.fp;
    x[30] = m.lr;
    let Some(st) = decode_store(insn, &x, m.sp, &m.v) else {
        return false;
    };
    let addr = st.addr & ADDRESS_MASK;
    let n = st.len as u64;
    if !in_jit(addr, n) {
        return false;
    }
    // SAFETY: [addr, addr+n) is JIT memory, writable while the thread's
    // view is; the returning handler restores the interrupted view.
    unsafe {
        libc::pthread_jit_write_protect_np(0);
        std::ptr::copy_nonoverlapping(st.bytes.as_ptr(), addr as *mut u8, st.len);
        libc::pthread_jit_write_protect_np(1);
        sys_icache_invalidate(addr as *mut _, st.len);
    }
    match st.writeback {
        Some((31, v)) => m.sp = v,
        Some((29, v)) => m.fp = v,
        Some((30, v)) => m.lr = v,
        Some((r, v)) => m.x[r as usize] = v,
        None => {}
    }
    m.pc += 4;
    true
}

/// Top-byte-ignore: the tag of a tagged pointer is not part of the address.
const ADDRESS_MASK: u64 = (1 << 56) - 1;

/// A decoded store: the bytes and where they go, and the base register's
/// new value for pre- and post-indexed forms (31: sp).
struct Store {
    addr: u64,
    bytes: [u8; 32],
    len: usize,
    writeback: Option<(u32, u64)>,
}

/// Decodes the plain A64 stores (general and SIMD&FP registers, single and
/// pair, immediate, register and unscaled offsets, pre- and post-indexed,
/// and store-release) given the registers (x31 in `x` is unused: sp is
/// `sp`, xzr is 0). Exclusive, atomic and multi-structure stores, and those
/// that name x18 (the host's), are not decoded.
fn decode_store(insn: u32, x: &[u64; 31], sp: u64, v: &[u128; 32]) -> Option<Store> {
    let rt = insn & 31;
    let rn = (insn >> 5) & 31;
    let base = if rn == 31 { sp } else { x[rn as usize] };
    let simd = insn & (1 << 26) != 0;
    let gpr = |r: u32| if r == 31 { 0 } else { x[r as usize] as u128 };
    let value = |r: u32| if simd { v[r as usize] } else { gpr(r) };
    let sext = |v: u32, bits: u32| ((v << (32 - bits)) as i32 >> (32 - bits)) as i64;
    let uses_x18 = |regs: &[u32]| regs.contains(&18);
    let mut st = Store {
        addr: 0,
        bytes: [0; 32],
        len: 0,
        writeback: None,
    };
    // Single register: size (31:30), opc (23:22); a SIMD&FP store of
    // size 0 with opc 2 is the 128-bit one.
    let single_scale = || -> Option<u32> {
        let (size, opc) = (insn >> 30, (insn >> 22) & 3);
        match (simd, opc) {
            (_, 0) => Some(size),
            (true, 2) if size == 0 => Some(4),
            _ => None,
        }
    };
    let (addr, scale, regs, wb): (u64, u32, [u32; 2], Option<u64>) =
        if insn & 0x3b00_0000 == 0x3900_0000 {
            // Unsigned immediate offset.
            let scale = single_scale()?;
            let imm = ((insn >> 10) & 0xfff) as u64;
            (base.wrapping_add(imm << scale), scale, [rt, 32], None)
        } else if insn & 0x3b20_0000 == 0x3800_0000 {
            // Unscaled, post-indexed, unprivileged and pre-indexed.
            let scale = single_scale()?;
            let imm = sext((insn >> 12) & 0x1ff, 9) as u64;
            match (insn >> 10) & 3 {
                0 | 2 => (base.wrapping_add(imm), scale, [rt, 32], None),
                1 => (base, scale, [rt, 32], Some(base.wrapping_add(imm))),
                _ => {
                    let a = base.wrapping_add(imm);
                    (a, scale, [rt, 32], Some(a))
                }
            }
        } else if insn & 0x3b20_0c00 == 0x3820_0800 {
            // Register offset.
            let scale = single_scale()?;
            let rm = (insn >> 16) & 31;
            let m = gpr(rm) as u64;
            let off = match (insn >> 13) & 7 {
                0b010 => m as u32 as u64,
                0b011 | 0b111 => m,
                0b110 => m as u32 as i32 as i64 as u64,
                _ => return None,
            };
            let shift = if insn & (1 << 12) != 0 { scale } else { 0 };
            if uses_x18(&[rm]) {
                return None;
            }
            (base.wrapping_add(off << shift), scale, [rt, 32], None)
        } else if insn & 0x3a40_0000 == 0x2800_0000 {
            // Pair (L = 0).
            let opc = insn >> 30;
            let scale = match (simd, opc) {
                (false, 0) => 2,
                (false, 2) => 3,
                (true, 0) => 2,
                (true, 1) => 3,
                (true, 2) => 4,
                _ => return None,
            };
            let imm = (sext((insn >> 15) & 0x7f, 7) << scale) as u64;
            let rt2 = (insn >> 10) & 31;
            match (insn >> 23) & 3 {
                0 | 2 => (base.wrapping_add(imm), scale, [rt, rt2], None),
                1 => (base, scale, [rt, rt2], Some(base.wrapping_add(imm))),
                _ => {
                    let a = base.wrapping_add(imm);
                    (a, scale, [rt, rt2], Some(a))
                }
            }
        } else if insn & 0x3fff_fc00 == 0x089f_fc00 || insn & 0x3fff_fc00 == 0x089f_7c00 {
            // STLR, STLLR.
            (base, insn >> 30, [rt, 32], None)
        } else if insn & 0x3fe0_0c00 == 0x1900_0000 {
            // STLUR (general registers).
            let imm = sext((insn >> 12) & 0x1ff, 9) as u64;
            (base.wrapping_add(imm), insn >> 30, [rt, 32], None)
        } else {
            return None;
        };
    if uses_x18(&[rn]) || (!simd && uses_x18(&regs)) {
        return None;
    }
    let size = 1usize << scale;
    for r in regs.into_iter().filter(|&r| r < 32) {
        st.bytes[st.len..st.len + size].copy_from_slice(&value(r).to_le_bytes()[..size]);
        st.len += size;
    }
    st.addr = addr;
    st.writeback = wb.map(|w| (rn, w));
    Some(st)
}

/// Make JIT memory executable for this thread, then resume the registers
/// [`fault`] saved.
extern "C" fn exec_and_resume() -> ! {
    // SAFETY: switches this thread's view of JIT memory.
    unsafe { libc::pthread_jit_write_protect_np(1) };
    context::resume_trap()
}

#[cfg(test)]
mod tests {
    use super::decode_store;

    /// Address, bytes and writeback of a decoded store.
    type Decoded = (u64, Vec<u8>, Option<(u32, u64)>);

    fn decode(insn: u32) -> Option<Decoded> {
        let mut x = [0u64; 31];
        x[0] = 0x1000;
        x[1] = 0x0102_0304_0506_0708;
        x[2] = 16;
        x[18] = 0x5000;
        let mut v = [0u128; 32];
        v[0] = 0x1111_2222_3333_4444_5555_6666_7777_8888;
        decode_store(insn, &x, 0x9000, &v).map(|s| (s.addr, s.bytes[..s.len].to_vec(), s.writeback))
    }

    #[test]
    fn stores_decode() {
        let x1 = 0x0102_0304_0506_0708u64.to_le_bytes();
        // str x1, [x0, #8]
        assert_eq!(decode(0xf9000401), Some((0x1008, x1.to_vec(), None)));
        // strb w1, [x0, #40]; sturh w1, [x0, #13]
        assert_eq!(decode(0x3900a001), Some((0x1028, vec![8], None)));
        assert_eq!(decode(0x7800d001), Some((0x100d, vec![8, 7], None)));
        // str x1, [x0, #8]!; str w1, [x0], #-4
        assert_eq!(
            decode(0xf8008c01),
            Some((0x1008, x1.to_vec(), Some((0, 0x1008))))
        );
        assert_eq!(
            decode(0xb81fc401),
            Some((0x1000, x1[..4].to_vec(), Some((0, 0xffc))))
        );
        // str x1, [x0, x2]; stp x1, xzr, [sp, #-16]!
        assert_eq!(decode(0xf8226801), Some((0x1010, x1.to_vec(), None)));
        let pair = [x1, [0; 8]].concat();
        assert_eq!(decode(0xa9bf7fe1), Some((0x8ff0, pair, Some((31, 0x8ff0)))));
        // str q0, [x0, #48]; stlr w1, [x0]
        let q0 = 0x1111_2222_3333_4444_5555_6666_7777_8888u128.to_le_bytes();
        assert_eq!(decode(0x3d800c00), Some((0x1030, q0.to_vec(), None)));
        assert_eq!(decode(0x889ffc01), Some((0x1000, x1[..4].to_vec(), None)));
        // Loads, exclusive and atomic stores, and x18 are left alone:
        // ldr x1, [x0]; stxr w3, x1, [x0]; ldadd x1, x2, [x0];
        // str x1, [x18]; str x18, [x0]
        for insn in [0xf9400001, 0xc8037c01, 0xf8210002, 0xf9000241, 0xf9000012] {
            assert!(decode(insn).is_none(), "{insn:#x}");
        }
    }
}
