//! Load-time rewriting of guest code that must reach the host.
//!
//! Three instruction forms are rewritten before guest code runs:
//!
//! | Guest instruction      | Why                                         | Replacement                       |
//! |------------------------|---------------------------------------------|-----------------------------------|
//! | `svc #0`               | Linux syscall; on Darwin it would trap into XNU with x16 as a Darwin syscall number | `b` to a per-site syscall stub |
//! | `mrs xN, tpidr_el0`    | XNU overwrites TPIDR_EL0 on context switch  | `b` to a 3-instruction TSD load   |
//! | `msr tpidr_el0, xN`    | same                                        | `b` to a TSD store stub           |
//!
//! Each stub lives in a trampoline island within the ±128 MiB reach of a `b`
//! from its site, and ends with `b <site + 4>`. A `b` (not `bl`) keeps x30,
//! which a Linux `svc` preserves and bionic's syscall stubs rely on.
//!
//! When no island can be placed in range, the site becomes
//! `brk #(0xA000 | kind << 5 | reg)` and a SIGTRAP handler emulates it
//! (slow fallback).

use std::sync::Mutex;

use crate::context::{GuestContext, guest_tp_tsd_offset, linux_abi_syscall_entry};

pub const SVC0: u32 = 0xd400_0001;
const MRS_TPIDR_EL0: u32 = 0xd53b_d040;
const MSR_TPIDR_EL0: u32 = 0xd51b_d040;
const MRS_TPIDRRO_EL0: u32 = 0xd53b_d060;
const BRK_TAG: u32 = 0xA000;
const BRANCH_RANGE: i64 = 128 << 20;
const ISLAND_SIZE: usize = 256 << 10;
const PAGE: u64 = 16384;

const KIND_SVC: u32 = 0;
const KIND_MRS: u32 = 1;
const KIND_MSR: u32 = 2;

unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
}

// ---- encoders -------------------------------------------------------------

pub fn encode_b(from: u64, to: u64) -> Option<u32> {
    let off = to.wrapping_sub(from) as i64;
    if !(-BRANCH_RANGE..BRANCH_RANGE).contains(&off) || off & 3 != 0 {
        return None;
    }
    Some(0x1400_0000 | ((off >> 2) as u32 & 0x03ff_ffff))
}

/// Byte offset of an unconditional `b`.
pub fn decode_b(insn: u32) -> i64 {
    (((insn & 0x03ff_ffff) << 6) as i32 >> 4) as i64
}

fn stp_pre(rt: u32, rt2: u32, rn: u32, imm: i32) -> u32 {
    0xa980_0000 | (((imm / 8) as u32 & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_post(rt: u32, rt2: u32, rn: u32, imm: i32) -> u32 {
    0xa8c0_0000 | (((imm / 8) as u32 & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn str_uimm(rt: u32, rn: u32, imm: u32) -> u32 {
    0xf900_0000 | ((imm / 8) << 10) | (rn << 5) | rt
}
fn ldr_uimm(rt: u32, rn: u32, imm: u32) -> u32 {
    0xf940_0000 | ((imm / 8) << 10) | (rn << 5) | rt
}
fn ldr_literal(rt: u32, from: u64, to: u64) -> u32 {
    let off = to.wrapping_sub(from) as i64;
    assert!(off.abs() < (1 << 20) && off & 3 == 0);
    0x5800_0000 | ((((off >> 2) as u32) & 0x7_ffff) << 5) | rt
}
fn blr(rn: u32) -> u32 {
    0xd63f_0000 | (rn << 5)
}
fn brk(imm: u32) -> u32 {
    0xd420_0000 | ((imm & 0xffff) << 5)
}

const SP: u32 = 31;

/// Per-site stub bodies, excluding the final `b <site+4>`.
fn svc_stub(stub: u64, literal: u64) -> Vec<u32> {
    vec![
        stp_pre(16, 17, SP, -32),
        str_uimm(30, SP, 16),
        ldr_literal(16, stub + 8, literal),
        blr(16),
        ldr_uimm(30, SP, 16),
        ldp_post(16, 17, SP, 32),
    ]
}

fn mrs_stub(rt: u32, tp_off: u32) -> Vec<u32> {
    vec![MRS_TPIDRRO_EL0 | rt, ldr_uimm(rt, rt, tp_off)]
}

fn msr_stub(rt: u32, tp_off: u32) -> Vec<u32> {
    let scratch = if rt == 16 { 17 } else { 16 };
    vec![
        stp_pre(16, 17, SP, -16),
        MRS_TPIDRRO_EL0 | scratch,
        str_uimm(rt, scratch, tp_off),
        ldp_post(16, 17, SP, 16),
    ]
}

// ---- islands --------------------------------------------------------------

struct Island {
    base: u64,
    used: u64,
}

static ISLANDS: Mutex<Vec<Island>> = Mutex::new(Vec::new());

fn set_prot(addr: u64, len: u64, prot: i32) {
    let start = addr & !(PAGE - 1);
    let end = (addr + len + PAGE - 1) & !(PAGE - 1);
    // SAFETY: island memory is owned by this module.
    let r = unsafe { libc::mprotect(start as *mut _, (end - start) as usize, prot) };
    assert_eq!(r, 0, "island mprotect failed");
}

fn in_range(a: u64, b: u64) -> bool {
    let d = a.wrapping_sub(b) as i64;
    d.abs() < BRANCH_RANGE - ISLAND_SIZE as i64
}

fn new_island(hint: u64) -> Option<u64> {
    // SAFETY: non-fixed anonymous mapping; hint only steers placement.
    let p = unsafe {
        libc::mmap(
            hint as *mut _,
            ISLAND_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return None;
    }
    let base = p as u64;
    // SAFETY: freshly mapped, writable.
    unsafe { (base as *mut u64).write(linux_abi_syscall_entry as usize as u64) };
    set_prot(base, ISLAND_SIZE as u64, libc::PROT_READ | libc::PROT_EXEC);
    Some(base)
}

/// Reserve `bytes` of stub space reachable from `site`; the region being
/// patched spans `[lo, hi)` and guides where a new island goes.
fn alloc_stub(
    islands: &mut Vec<Island>,
    site: u64,
    lo: u64,
    hi: u64,
    bytes: u64,
) -> Option<(u64, u64)> {
    for isl in islands.iter_mut() {
        let at = isl.base + isl.used;
        if isl.used + bytes <= ISLAND_SIZE as u64 && in_range(at, site) {
            isl.used += bytes;
            return Some((at, isl.base));
        }
    }
    let hi_hint = (hi + PAGE - 1) & !(PAGE - 1);
    let lo_hint = (lo & !(PAGE - 1)).saturating_sub(4 * ISLAND_SIZE as u64);
    for hint in [hi_hint, lo_hint, site] {
        if let Some(base) = new_island(hint) {
            if in_range(base, site) {
                islands.push(Island {
                    base,
                    used: 8 + bytes,
                });
                return Some((base + 8, base));
            }
            // SAFETY: unmapping the island we just created.
            unsafe { libc::munmap(base as *mut _, ISLAND_SIZE) };
        }
    }
    None
}

fn write_stub(stub: u64, body: &[u32], site: u64) {
    set_prot(
        stub,
        (body.len() as u64 + 1) * 4,
        libc::PROT_READ | libc::PROT_WRITE,
    );
    let back = encode_b(stub + body.len() as u64 * 4, site + 4).expect("stub out of range");
    // SAFETY: stub space was reserved in a writable island.
    unsafe {
        let p = stub as *mut u32;
        for (i, w) in body.iter().enumerate() {
            p.add(i).write(*w);
        }
        p.add(body.len()).write(back);
        sys_icache_invalidate(p as *mut _, (body.len() + 1) * 4);
    }
    set_prot(
        stub,
        (body.len() as u64 + 1) * 4,
        libc::PROT_READ | libc::PROT_EXEC,
    );
}

#[derive(Default, Debug, Clone, Copy)]
pub struct PatchStats {
    pub svc: usize,
    pub mrs_tp: usize,
    pub msr_tp: usize,
    pub brk_fallback: usize,
}

/// Rewrite every `svc #0` and `TPIDR_EL0` access in `[addr, addr+len)`.
/// The region must currently be writable; the caller restores its protection.
pub fn rewrite_region(addr: u64, len: u64) -> PatchStats {
    rewrite_region_with(addr, len, true)
}

/// As [`rewrite_region`], but with `use_islands == false` every site takes the
/// `brk` fallback (used to measure that path).
pub fn rewrite_region_with(addr: u64, len: u64, use_islands: bool) -> PatchStats {
    let mut stats = PatchStats::default();
    let tp_off = guest_tp_tsd_offset();
    let mut islands = ISLANDS.lock().unwrap_or_else(|e| e.into_inner());
    let words = (len / 4) as usize;
    let base = (addr + 3) & !3;
    for i in 0..words {
        let site = base + i as u64 * 4;
        if site + 4 > addr + len {
            break;
        }
        // SAFETY: the caller guarantees the region is mapped and readable.
        let insn = unsafe { (site as *const u32).read() };
        let rt = insn & 0x1f;
        let (kind, body_len) = if insn == SVC0 {
            (KIND_SVC, 6)
        } else if insn & !0x1f == MRS_TPIDR_EL0 {
            (KIND_MRS, 2)
        } else if insn & !0x1f == MSR_TPIDR_EL0 {
            (KIND_MSR, 4)
        } else {
            continue;
        };
        if kind == KIND_MRS && rt == 31 {
            continue; // mrs xzr: no effect.
        }
        let stub = if use_islands {
            alloc_stub(&mut islands, site, addr, addr + len, (body_len + 1) * 4)
        } else {
            None
        };
        let replacement = match stub {
            Some((stub, island)) => {
                let body = match kind {
                    KIND_SVC => svc_stub(stub, island),
                    KIND_MRS => mrs_stub(rt, tp_off),
                    _ => msr_stub(rt, tp_off),
                };
                write_stub(stub, &body, site);
                encode_b(site, stub).expect("island out of range")
            }
            None => {
                stats.brk_fallback += 1;
                brk(BRK_TAG | (kind << 5) | rt)
            }
        };
        match kind {
            KIND_SVC => stats.svc += 1,
            KIND_MRS => stats.mrs_tp += 1,
            _ => stats.msr_tp += 1,
        }
        // SAFETY: region is writable per the caller's contract.
        unsafe { (site as *mut u32).write(replacement) };
    }
    // SAFETY: flushing the range we just wrote.
    unsafe { sys_icache_invalidate(addr as *mut _, len as usize) };
    stats
}

// ---- brk fallback ---------------------------------------------------------

#[repr(C)]
struct ThreadState64 {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    cpsr: u32,
    pad: u32,
}

#[repr(C)]
struct NeonState64 {
    v: [u128; 32],
    fpsr: u32,
    fpcr: u32,
}

#[repr(C)]
struct MContext64 {
    es: [u64; 2],
    ss: ThreadState64,
    ns: NeonState64,
}

/// Emulate a fallback `brk` site. Returns false if the brk is not ours.
///
/// # Safety
/// `uc` must be the ucontext passed to a SIGTRAP handler.
pub unsafe fn handle_brk(uc: *mut libc::ucontext_t) -> bool {
    // SAFETY: Darwin arm64 ucontext layout.
    unsafe {
        let mc = (*uc).uc_mcontext as *mut MContext64;
        let ss = &mut (*mc).ss;
        let insn = (ss.pc as *const u32).read();
        if insn & 0xffe0_001f != 0xd420_0000 {
            return false;
        }
        let imm = (insn >> 5) & 0xffff;
        if imm & 0xff00 != BRK_TAG {
            return false;
        }
        let kind = (imm >> 5) & 0x7;
        let rt = (imm & 0x1f) as usize;
        let tp_slot = (crate::context::guest_tp_tsd_offset() / 8) as usize;
        let tsd: u64;
        std::arch::asm!("mrs {}, tpidrro_el0", out(reg) tsd);
        let get = |ss: &ThreadState64, r: usize| match r {
            0..=28 => ss.x[r],
            29 => ss.fp,
            30 => ss.lr,
            _ => 0,
        };
        match kind {
            KIND_MRS => {
                let v = (tsd as *const u64).add(tp_slot).read();
                match rt {
                    0..=28 => ss.x[rt] = v,
                    29 => ss.fp = v,
                    30 => ss.lr = v,
                    _ => {}
                }
            }
            KIND_MSR => (tsd as *mut u64).add(tp_slot).write(get(ss, rt)),
            _ => {
                let mut ctx: GuestContext = std::mem::zeroed();
                ctx.x[..29].copy_from_slice(&ss.x);
                ctx.x[29] = ss.fp;
                ctx.x[30] = ss.lr;
                ctx.sp = ss.sp;
                ctx.pc = ss.pc + 4;
                ctx.v = (*mc).ns.v;
                crate::sys::dispatch(&mut ctx);
                ss.x.copy_from_slice(&ctx.x[..29]);
                ss.fp = ctx.x[29];
                ss.lr = ctx.x[30];
                ss.sp = ctx.sp;
                ss.pc = ctx.pc - 4;
            }
        }
        ss.pc += 4;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    std::arch::global_asm!(
        ".globl _patch_reference",
        ".p2align 2",
        "_patch_reference:",
        "stp x16, x17, [sp, #-32]!",
        "str x30, [sp, #16]",
        "blr x16",
        "ldr x30, [sp, #16]",
        "ldp x16, x17, [sp], #32",
        "mrs x5, tpidrro_el0",
        "ldr x5, [x5, #1024]",
        "stp x16, x17, [sp, #-16]!",
        "str x3, [x16, #1024]",
        "ldp x16, x17, [sp], #16",
        "brk #0xa021",
        "mrs x7, tpidr_el0",
        "msr tpidr_el0, x9",
    );
    unsafe extern "C" {
        static patch_reference: [u32; 13];
    }

    #[test]
    fn encoders_match_assembler() {
        // SAFETY: symbol defined above.
        let r = unsafe { &patch_reference };
        assert_eq!(stp_pre(16, 17, SP, -32), r[0]);
        assert_eq!(str_uimm(30, SP, 16), r[1]);
        assert_eq!(blr(16), r[2]);
        assert_eq!(ldr_uimm(30, SP, 16), r[3]);
        assert_eq!(ldp_post(16, 17, SP, 32), r[4]);
        assert_eq!(MRS_TPIDRRO_EL0 | 5, r[5]);
        assert_eq!(ldr_uimm(5, 5, 1024), r[6]);
        assert_eq!(stp_pre(16, 17, SP, -16), r[7]);
        assert_eq!(str_uimm(3, 16, 1024), r[8]);
        assert_eq!(ldp_post(16, 17, SP, 16), r[9]);
        assert_eq!(brk(BRK_TAG | (KIND_MRS << 5) | 1), r[10]);
        assert_eq!(MRS_TPIDR_EL0 | 7, r[11]);
        assert_eq!(MSR_TPIDR_EL0 | 9, r[12]);
    }

    #[test]
    fn branch_roundtrip() {
        for off in [4i64, -4, 1 << 20, -(1 << 26), (128 << 20) - 4] {
            let b = encode_b(0x1000_0000, (0x1000_0000i64 + off) as u64).unwrap();
            assert_eq!(decode_b(b), off);
        }
        assert!(encode_b(0, 128 << 20).is_none());
    }
}
