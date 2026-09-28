//! Per-image initialization state, separate from mutable mapping construction.
//! No lock or mutable image borrow crosses an initializer callback. This is
//! an execution-once guard, not the linker's cross-thread load scheduler.
use super::{LoadError, LoadedElf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Default)]
pub(crate) struct Initialization {
    // 0 fresh, 1 executing, 2 complete, 3 failed (never retry a partial run).
    state: AtomicU8,
    armed: AtomicBool,
}
impl Initialization {
    pub(crate) fn complete(&self) -> bool {
        self.state.load(Ordering::Acquire) == 2
    }
    pub(crate) fn armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }
    fn run(
        &self,
        execute: impl FnOnce() -> Result<(), LoadError>,
        arm: bool,
    ) -> Result<(), LoadError> {
        self.state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| LoadError::InitializersAlreadyRun)?;
        let result = execute();
        if result.is_ok() {
            self.armed.store(arm, Ordering::Release);
            self.state.store(2, Ordering::Release);
        } else {
            self.state.store(3, Ordering::Release);
        }
        result
    }
}
impl LoadedElf {
    pub fn run_initializers(&self) -> Result<(), LoadError> {
        self.run_initializers_internal(true)
    }
    pub(crate) fn run_initializers_for_graph(&self) -> Result<(), LoadError> {
        self.run_initializers_internal(false)
    }
    fn run_initializers_internal(&self, arm: bool) -> Result<(), LoadError> {
        self.validate_initializer_entries()?;
        self.initialization.run(
            || {
                let address = self.dynamic.init_array.unwrap_or(0);
                let size = self.dynamic.init_array_size.unwrap_or(0);
                for index in 0..size / 8 {
                    let pointer = self.read_loaded_u64(address + index * 8)?;
                    if pointer == 0 || pointer == u64::MAX {
                        continue;
                    }
                    self.require_host_executable(pointer, "DT_INIT_ARRAY function")?;
                    // SAFETY: relocated executable address, retained mapping, supported
                    // no-argument AArch64 initializer ABI. No mutable host borrow.
                    let initializer: unsafe extern "C" fn() =
                        unsafe { std::mem::transmute(pointer as usize) };
                    unsafe { initializer() };
                }
                Ok(())
            },
            arm,
        )
    }
    pub(crate) fn arm_finalizers(&self) {
        debug_assert!(self.initialization.complete());
        self.initialization.armed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executing_and_failed_states_cannot_run_twice() {
        let state = Initialization::default();
        state
            .run(
                || {
                    assert!(!state.complete());
                    assert!(matches!(
                        state.run(|| panic!("reentered"), true),
                        Err(LoadError::InitializersAlreadyRun)
                    ));
                    Ok(())
                },
                true,
            )
            .unwrap();
        assert!(state.complete() && state.armed());
        assert!(state.run(|| panic!("repeated"), true).is_err());
        let failed = Initialization::default();
        assert!(
            failed
                .run(|| Err(LoadError::Format("fixture failure")), true)
                .is_err()
        );
        assert!(!failed.complete() && !failed.armed());
        assert!(
            failed
                .run(|| panic!("retried partial execution"), true)
                .is_err()
        );
    }
}
