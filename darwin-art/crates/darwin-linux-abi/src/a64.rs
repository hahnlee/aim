//! A64 encodings shared by the ahead-of-time translator (`xlate`) and the
//! load-time patcher (`patch`): the guest instruction patterns that must be
//! rewritten, and the per-site stubs that replace them.
//!
//! Stubs are position independent apart from their final `b <site + 4>`,
//! and reach host state only through fixed Darwin TSD slots
//! (`[TPIDRRO_EL0 + slot]`), never through absolute addresses. That is what
//! lets a translated file be mapped shared by every process: the same bytes
//! are valid at any load address and in any process whose runtime claimed
//! the same slots (see `context`).
//!
//! No stub uses x18, and every stub returns with `b`, so x30 survives.

/// `svc #0`: a Linux syscall.
pub const SVC0: u32 = 0xd400_0001;
/// `mrs xN, tpidr_el0` (N in bits 4:0).
pub const MRS_TPIDR_EL0: u32 = 0xd53b_d040;
/// `msr tpidr_el0, xN`.
pub const MSR_TPIDR_EL0: u32 = 0xd51b_d040;
/// `mrs xN, tpidrro_el0`: Darwin's TSD base, readable at EL0.
pub const MRS_TPIDRRO_EL0: u32 = 0xd53b_d060;
/// `mrs xN, ctr_el0`: traps on Darwin (experiments/p0/05).
pub const MRS_CTR_EL0: u32 = 0xd53b_0020;
/// `str x30, [x18], #8`: shadow-call-stack push.
pub const SCS_PUSH: u32 = 0xf800_865e;
/// `ldr x30, [x18, #-8]!`: shadow-call-stack pop.
pub const SCS_POP: u32 = 0xf85f_8e5e;

/// Tag of the `brk` fallback used when no stub is reachable:
/// `brk #(BRK_TAG | kind << 5 | rt)`.
pub const BRK_TAG: u32 = 0xA000;
pub const BRANCH_RANGE: i64 = 128 << 20;

/// Darwin pthread keys the runtime claims at start-up. Translated code
/// addresses their slots as `[TPIDRRO_EL0 + key * 8]`, so the numbers are
/// part of the translation ABI (a change needs a translator version bump).
/// They are the last four dynamic keys (Darwin hands out 258..=767), which
/// host code practically never reaches.
pub mod slot {
    /// Pointer to the thread's `GuestContext`.
    pub const CTX_KEY: usize = 764;
    /// The guest `TPIDR_EL0` value (bionic's thread pointer).
    pub const TP_KEY: usize = 765;
    /// Address of the syscall entry trampoline.
    pub const ENTRY_KEY: usize = 766;
    /// The guest shadow-call-stack pointer (what x18 holds on Linux).
    pub const SCS_KEY: usize = 767;

    pub const CTX: u32 = (CTX_KEY * 8) as u32;
    pub const TP: u32 = (TP_KEY * 8) as u32;
    pub const ENTRY: u32 = (ENTRY_KEY * 8) as u32;
    pub const SCS: u32 = (SCS_KEY * 8) as u32;
}

/// CTR_EL0 as the guest sees it: 64-byte I and D lines (never larger than the
/// host's, so cache maintenance loops cover every line), PIPT I-cache,
/// IDC = DIC = 0 so guests always clean and invalidate. Apple silicon has
/// 128-byte lines. `mrs ctr_el0` traps on Darwin, so this value is what
/// rewritten `mrs xN, ctr_el0` sites load.
pub fn host_ctr_el0() -> u32 {
    let mut line: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: sysctl reading a u64 into a local.
    let ok = unsafe {
        libc::sysctlbyname(
            c"hw.cachelinesize".as_ptr(),
            (&mut line as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    let line = if ok && line >= 16 { line.min(64) } else { 64 };
    let log2_words = (line / 4).trailing_zeros();
    // RES1 | CWG | ERG | DminLine | L1Ip=PIPT | IminLine.
    0x8000_0000 | (4 << 24) | (4 << 20) | (log2_words << 16) | (3 << 14) | log2_words
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Svc = 0,
    MrsTp = 1,
    MsrTp = 2,
    ScsPush = 3,
    ScsPop = 4,
    MrsCtr = 5,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Svc,
        Kind::MrsTp,
        Kind::MsrTp,
        Kind::ScsPush,
        Kind::ScsPop,
        Kind::MrsCtr,
    ];

    pub fn from_index(i: u32) -> Option<Kind> {
        Kind::ALL.get(i as usize).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Svc => "svc",
            Kind::MrsTp => "mrs_tpidr_el0",
            Kind::MsrTp => "msr_tpidr_el0",
            Kind::ScsPush => "scs_push",
            Kind::ScsPop => "scs_pop",
            Kind::MrsCtr => "mrs_ctr_el0",
        }
    }
}

/// Classify a word as one of the rewritten instruction forms.
/// Returns the kind and the Rt field.
pub fn classify(w: u32) -> Option<(Kind, u32)> {
    let rt = w & 0x1f;
    match w {
        SVC0 => Some((Kind::Svc, 0)),
        SCS_PUSH => Some((Kind::ScsPush, 30)),
        SCS_POP => Some((Kind::ScsPop, 30)),
        // `mrs xzr, ...` has no effect and does not trap for TPIDR_EL0.
        _ if w & !0x1f == MRS_TPIDR_EL0 && rt != 31 => Some((Kind::MrsTp, rt)),
        _ if w & !0x1f == MSR_TPIDR_EL0 => Some((Kind::MsrTp, rt)),
        // `mrs xzr, ctr_el0` still traps, so it is rewritten too.
        _ if w & !0x1f == MRS_CTR_EL0 => Some((Kind::MrsCtr, rt)),
        _ => None,
    }
}

// ---- encoders -------------------------------------------------------------

pub const SP: u32 = 31;

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

pub fn is_b(insn: u32) -> bool {
    insn & 0xfc00_0000 == 0x1400_0000
}

pub fn stp_pre(rt: u32, rt2: u32, rn: u32, imm: i32) -> u32 {
    0xa980_0000 | (((imm / 8) as u32 & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
pub fn ldp_post(rt: u32, rt2: u32, rn: u32, imm: i32) -> u32 {
    0xa8c0_0000 | (((imm / 8) as u32 & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
pub fn str_uimm(rt: u32, rn: u32, imm: u32) -> u32 {
    0xf900_0000 | ((imm / 8) << 10) | (rn << 5) | rt
}
pub fn ldr_uimm(rt: u32, rn: u32, imm: u32) -> u32 {
    0xf940_0000 | ((imm / 8) << 10) | (rn << 5) | rt
}
/// `str xt, [xn], #imm` (post-index).
pub fn str_post(rt: u32, rn: u32, imm: i32) -> u32 {
    0xf800_0400 | (((imm as u32) & 0x1ff) << 12) | (rn << 5) | rt
}
/// `ldr xt, [xn, #imm]!` (pre-index).
pub fn ldr_pre(rt: u32, rn: u32, imm: i32) -> u32 {
    0xf840_0c00 | (((imm as u32) & 0x1ff) << 12) | (rn << 5) | rt
}
pub fn blr(rn: u32) -> u32 {
    0xd63f_0000 | (rn << 5)
}
pub fn brk(imm: u32) -> u32 {
    0xd420_0000 | ((imm & 0xffff) << 5)
}
/// `movz xd, #imm16, lsl #shift`.
pub fn movz(rd: u32, imm16: u32, shift: u32) -> u32 {
    0xd280_0000 | ((shift / 16) << 21) | ((imm16 & 0xffff) << 5) | rd
}
/// `movk xd, #imm16, lsl #shift`.
pub fn movk(rd: u32, imm16: u32, shift: u32) -> u32 {
    0xf280_0000 | ((shift / 16) << 21) | ((imm16 & 0xffff) << 5) | rd
}
pub const NOP: u32 = 0xd503_201f;

pub fn brk_fallback(kind: Kind, rt: u32) -> u32 {
    brk(BRK_TAG | ((kind as u32) << 5) | rt)
}

// ---- stubs ----------------------------------------------------------------

/// Number of words of a stub, including the final `b <site + 4>`.
pub fn stub_len(kind: Kind) -> usize {
    stub_body(kind, 0, 0).len() + 1
}

/// Word index, inside an svc stub, of the instruction after `blr`: the
/// syscall entry sees it as x30 (`stub_ret`), and the `b <site + 4>` is two
/// words later (see `GuestContext::resume_pc`).
pub const SVC_STUB_RET_WORD: usize = 5;

/// Stub body for one rewritten site, without the final `b <site + 4>`.
pub fn stub_body(kind: Kind, rt: u32, ctr_el0: u32) -> Vec<u32> {
    match kind {
        // Saves x16/x17/x30 below the guest sp, as a Linux-safe scratch area
        // (AArch64 Linux has no red zone). The entry trampoline finds them
        // there. x30 is restored, so it survives the syscall as on Linux.
        Kind::Svc => vec![
            stp_pre(16, 17, SP, -32),
            str_uimm(30, SP, 16),
            MRS_TPIDRRO_EL0 | 16,
            ldr_uimm(16, 16, slot::ENTRY),
            blr(16),
            ldr_uimm(30, SP, 16),
            ldp_post(16, 17, SP, 32),
        ],
        // The destination register is its own scratch.
        Kind::MrsTp => vec![MRS_TPIDRRO_EL0 | rt, ldr_uimm(rt, rt, slot::TP)],
        Kind::MsrTp => {
            let scratch = if rt == 16 { 17 } else { 16 };
            vec![
                stp_pre(16, 17, SP, -16),
                MRS_TPIDRRO_EL0 | scratch,
                str_uimm(rt, scratch, slot::TP),
                ldp_post(16, 17, SP, 16),
            ]
        }
        // push: *ptr = x30; ptr += 8. The shadow stack stays a separate,
        // guarded allocation, so the return address is still reloaded from
        // memory the regular stack cannot overwrite.
        Kind::ScsPush => vec![
            stp_pre(16, 17, SP, -16),
            MRS_TPIDRRO_EL0 | 16,
            ldr_uimm(17, 16, slot::SCS),
            str_post(30, 17, 8),
            str_uimm(17, 16, slot::SCS),
            ldp_post(16, 17, SP, 16),
        ],
        // pop: ptr -= 8; x30 = *ptr.
        Kind::ScsPop => vec![
            stp_pre(16, 17, SP, -16),
            MRS_TPIDRRO_EL0 | 16,
            ldr_uimm(17, 16, slot::SCS),
            ldr_pre(30, 17, -8),
            str_uimm(17, 16, slot::SCS),
            ldp_post(16, 17, SP, 16),
        ],
        Kind::MrsCtr if rt == 31 => vec![NOP],
        Kind::MrsCtr => vec![movz(rt, ctr_el0 & 0xffff, 0), movk(rt, ctr_el0 >> 16, 16)],
    }
}

/// The full stub for a site at `site`, placed at `stub`. None when either
/// branch is out of range.
pub fn stub_for(kind: Kind, rt: u32, ctr_el0: u32, stub: u64, site: u64) -> Option<Vec<u32>> {
    let mut words = stub_body(kind, rt, ctr_el0);
    let back = stub + words.len() as u64 * 4;
    words.push(encode_b(back, site + 4)?);
    encode_b(site, stub)?;
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    std::arch::global_asm!(
        ".globl _a64_reference",
        ".p2align 2",
        "_a64_reference:",
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
        "str x30, [x18], #8",
        "ldr x30, [x18, #-8]!",
        "str x30, [x17], #8",
        "ldr x30, [x17, #-8]!",
        "mrs x4, ctr_el0",
        "movz x4, #0xc004",
        "movk x4, #0x8444, lsl #16",
        "ldr x16, [x16, #6128]",
    );
    unsafe extern "C" {
        static a64_reference: [u32; 21];
    }

    #[test]
    fn encoders_match_assembler() {
        // SAFETY: symbol defined above.
        let r = unsafe { &a64_reference };
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
        assert_eq!(brk_fallback(Kind::MrsTp, 1), r[10]);
        assert_eq!(MRS_TPIDR_EL0 | 7, r[11]);
        assert_eq!(MSR_TPIDR_EL0 | 9, r[12]);
        assert_eq!(SCS_PUSH, r[13]);
        assert_eq!(SCS_POP, r[14]);
        assert_eq!(str_post(30, 17, 8), r[15]);
        assert_eq!(ldr_pre(30, 17, -8), r[16]);
        assert_eq!(MRS_CTR_EL0 | 4, r[17]);
        assert_eq!(movz(4, 0xc004, 0), r[18]);
        assert_eq!(movk(4, 0x8444, 16), r[19]);
        assert_eq!(ldr_uimm(16, 16, slot::ENTRY), r[20]);
    }

    #[test]
    fn branch_roundtrip() {
        for off in [4i64, -4, 1 << 20, -(1 << 26), (128 << 20) - 4] {
            let b = encode_b(0x1000_0000, (0x1000_0000i64 + off) as u64).unwrap();
            assert_eq!(decode_b(b), off);
        }
        assert!(encode_b(0, 128 << 20).is_none());
    }

    #[test]
    fn stubs_contain_no_rewritten_patterns_and_no_x18() {
        for kind in Kind::ALL {
            for rt in 0..31 {
                for w in stub_body(kind, rt, 0x8444_c004) {
                    assert!(classify(w).is_none(), "{kind:?} x{rt}: {w:#x}");
                }
            }
        }
    }

    #[test]
    fn svc_stub_return_word_is_after_blr() {
        let body = stub_body(Kind::Svc, 0, 0);
        assert_eq!(body[SVC_STUB_RET_WORD - 1], blr(16));
        // b <site+4> is two words after stub_ret.
        assert_eq!(body.len(), SVC_STUB_RET_WORD + 2);
    }

    #[test]
    fn host_ctr_is_conservative() {
        let v = host_ctr_el0();
        assert_eq!(v >> 31, 1);
        assert!(4 << ((v >> 16) & 0xf) <= 64);
        assert_eq!(v & (3 << 28), 0, "IDC/DIC must be clear");
    }
}
