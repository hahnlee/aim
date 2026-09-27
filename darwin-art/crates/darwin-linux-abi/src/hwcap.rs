//! Linux AT_HWCAP / AT_HWCAP2 derived from the host CPU (Darwin sysctl).
//!
//! HWCAP_CPUID is never set: Linux emulates EL0 reads of the ID registers,
//! XNU does not.
//!
//! SME and SME2 are never set: bionic's SME helpers would hold state in
//! x18, which Darwin clobbers (ADR 0012, P0 findings).

use std::ffi::CString;

fn feature(name: &str) -> bool {
    let c = CString::new(name).unwrap();
    let mut v: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    // SAFETY: sysctlbyname with a correctly sized i32 buffer.
    let r = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            (&mut v as *mut i32).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    r == 0 && v != 0
}

fn arm(feat: &str) -> bool {
    feature(&format!("hw.optional.arm.{feat}"))
}

/// (AT_HWCAP, AT_HWCAP2)
pub fn host_hwcaps() -> (u64, u64) {
    let mut h = 0u64;
    let mut h2 = 0u64;
    let mut set = |on: bool, bit: u32, second: bool| {
        if on {
            if second {
                h2 |= 1 << bit
            } else {
                h |= 1 << bit
            }
        }
    };
    // AT_HWCAP
    set(feature("hw.optional.floatingpoint"), 0, false); // FP
    set(arm("AdvSIMD") || feature("hw.optional.neon"), 1, false); // ASIMD
    set(true, 2, false); // EVTSTRM: generic timer event stream is always on
    set(arm("FEAT_AES"), 3, false);
    set(arm("FEAT_PMULL"), 4, false);
    set(arm("FEAT_SHA1"), 5, false);
    set(arm("FEAT_SHA256"), 6, false);
    set(feature("hw.optional.armv8_crc32"), 7, false);
    set(arm("FEAT_LSE"), 8, false); // ATOMICS
    set(arm("FEAT_FP16"), 9, false); // FPHP
    set(arm("FEAT_FP16"), 10, false); // ASIMDHP
    set(arm("FEAT_RDM"), 12, false);
    set(arm("FEAT_JSCVT"), 13, false);
    set(arm("FEAT_FCMA"), 14, false);
    set(arm("FEAT_LRCPC"), 15, false);
    set(arm("FEAT_DPB"), 16, false);
    set(arm("FEAT_SHA3"), 17, false);
    set(arm("FEAT_DotProd"), 20, false);
    set(arm("FEAT_SHA512"), 21, false);
    set(arm("FEAT_FHM"), 23, false);
    set(arm("FEAT_DIT"), 24, false);
    set(arm("FEAT_LSE2"), 25, false); // USCAT
    set(arm("FEAT_LRCPC2"), 26, false); // ILRCPC
    set(arm("FEAT_FlagM"), 27, false);
    set(arm("FEAT_SSBS"), 28, false);
    set(arm("FEAT_SB"), 29, false);
    set(arm("FEAT_PAuth"), 30, false); // PACA
    set(arm("FEAT_PAuth"), 31, false); // PACG
    // AT_HWCAP2
    set(arm("FEAT_DPB2"), 0, true);
    set(arm("FEAT_FlagM2"), 7, true);
    set(arm("FEAT_FRINTTS"), 8, true);
    set(arm("FEAT_I8MM"), 13, true);
    set(arm("FEAT_BF16"), 14, true);
    set(arm("FEAT_RNG") || arm("FEAT_RNDR"), 16, true);
    set(arm("FEAT_BTI"), 17, true);
    set(arm("FEAT_ECV"), 19, true);
    set(arm("FEAT_AFP"), 20, true);
    set(arm("FEAT_RPRES"), 21, true);
    set(arm("FEAT_WFxT"), 31, true);
    set(arm("FEAT_EBF16"), 32, true);
    set(arm("FEAT_CSSC"), 34, true);
    (h, h2)
}
