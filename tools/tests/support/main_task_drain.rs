//! Test-only bounded servicing of the shared macOS main run loop.
//!
//! This is not a production task executor or proof of native task retirement.

use std::time::Duration;

const MAIN_TASK_SLICE: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MainTaskDrainError {
    NotMainThread,
    MainRunLoopUnavailable,
    MainRunLoopStopped,
    UnknownRunLoopResult(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MainTaskDrainResult {
    NoSource,
    SourceHandled,
    TimedOut,
}

#[cfg(target_os = "macos")]
mod darwin {
    use super::{MAIN_TASK_SLICE, MainTaskDrainError, MainTaskDrainResult};
    use std::ffi::c_void;

    type CFRunLoopRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFTimeInterval = f64;
    type CFRunLoopRunResult = i32;

    const K_CF_RUN_LOOP_RUN_FINISHED: CFRunLoopRunResult = 1;
    const K_CF_RUN_LOOP_RUN_STOPPED: CFRunLoopRunResult = 2;
    const K_CF_RUN_LOOP_RUN_TIMED_OUT: CFRunLoopRunResult = 3;
    const K_CF_RUN_LOOP_RUN_HANDLED_SOURCE: CFRunLoopRunResult = 4;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: CFStringRef;
        fn CFRunLoopGetMain() -> CFRunLoopRef;
        fn CFRunLoopRunInMode(
            mode: CFStringRef,
            seconds: CFTimeInterval,
            return_after_source_handled: u8,
        ) -> CFRunLoopRunResult;
    }

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(context: *mut c_void);
    }

    #[link(name = "System")]
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }

    struct AutoreleasePool(*mut c_void);

    impl AutoreleasePool {
        fn push() -> Self {
            // SAFETY: called on the current thread; the returned token is
            // consumed exactly once by Drop below.
            Self(unsafe { objc_autoreleasePoolPush() })
        }
    }

    impl Drop for AutoreleasePool {
        fn drop(&mut self) {
            // SAFETY: this is the matching pop for the push in this scope.
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }

    pub(crate) fn drain_main_tasks() -> Result<MainTaskDrainResult, MainTaskDrainError> {
        // SAFETY: pthread_main_np is a process-local query with no borrowed
        // pointers or ownership effects.
        if unsafe { pthread_main_np() } == 0 {
            return Err(MainTaskDrainError::NotMainThread);
        }
        let _pool = AutoreleasePool::push();
        // SAFETY: CoreFoundation owns the process main loop and default mode
        // singleton for the duration of this process.
        let run_loop = unsafe { CFRunLoopGetMain() };
        if run_loop.is_null() {
            return Err(MainTaskDrainError::MainRunLoopUnavailable);
        }
        // SAFETY: the mode pointer is the CoreFoundation default-mode
        // singleton; this is a bounded, source-returning run-loop slice.
        let result =
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, MAIN_TASK_SLICE.as_secs_f64(), 1) };
        match result {
            K_CF_RUN_LOOP_RUN_HANDLED_SOURCE => Ok(MainTaskDrainResult::SourceHandled),
            K_CF_RUN_LOOP_RUN_TIMED_OUT => Ok(MainTaskDrainResult::TimedOut),
            K_CF_RUN_LOOP_RUN_FINISHED => Ok(MainTaskDrainResult::NoSource),
            K_CF_RUN_LOOP_RUN_STOPPED => Err(MainTaskDrainError::MainRunLoopStopped),
            other => Err(MainTaskDrainError::UnknownRunLoopResult(other)),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use darwin::drain_main_tasks;

#[cfg(not(target_os = "macos"))]
pub(crate) fn drain_main_tasks() -> Result<MainTaskDrainResult, MainTaskDrainError> {
    Err(MainTaskDrainError::MainRunLoopUnavailable)
}
