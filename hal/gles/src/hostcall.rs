//! The one host call every forwarded entry point makes.

/// Call forwarded entry point `id` with its register image of `words`
/// values.
///
/// # Safety
/// `regs` must be that entry point's register image.
#[inline(always)]
pub unsafe fn call(id: u32, regs: *mut u64, words: usize) -> i64 {
    // SAFETY: caller contract.
    unsafe { aim_hostcall::guest::gpu_forward(id, regs, words) }
}
