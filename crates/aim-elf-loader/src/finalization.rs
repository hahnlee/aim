//! Image teardown is separate from reservation release. The Android linker
//! decides when a group can unload; this guard only prevents duplicate callbacks.
use super::LoadedElf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(crate) struct Finalization(AtomicBool);
impl Finalization {
    pub(crate) fn run(&self, execute: impl FnOnce()) {
        if !self.0.swap(true, Ordering::AcqRel) {
            execute();
        }
    }
}
impl LoadedElf {
    // Caller owns the unloading operation and retains the complete group while
    // callbacks run. No mutable image reference or data lock crosses guest code.
    pub(crate) fn finalize_once(&self) {
        self.finalization.run(|| {
            if let Some(lifecycle) = &self.dso_lifecycle
                && lifecycle.finalize_image(self.mapping_range()).is_err()
            {
                // Same fail-closed behavior as Drop: unmapping live callbacks
                // after a failed native lifecycle drain would be unsafe.
                std::process::abort();
            }
            if !self.initialization.armed() {
                return;
            }
            for &pointer in &self.finalizers {
                // SAFETY: finish_load validated/captured executable pointers;
                // the reservation is retained throughout this sequence.
                let finalizer: unsafe extern "C" fn() = unsafe { std::mem::transmute(pointer) };
                unsafe { finalizer() };
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reentry_and_later_release_do_not_repeat_callbacks() {
        let state = Finalization::default();
        let mut calls = 0;
        state.run(|| {
            calls += 1;
            state.run(|| panic!("recursive finalizer"));
        });
        state.run(|| panic!("second finalizer"));
        assert_eq!(calls, 1);
    }
}
