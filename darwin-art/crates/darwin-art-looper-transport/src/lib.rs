//! Host resource boundary for the original Android Looper, not a scheduler.
//! Callback descriptors and readiness descriptors are host FDs, never guest IDs.
#[cfg(target_os = "macos")]
mod readiness;
#[cfg(all(test, target_os = "macos"))]
mod readiness_concurrency_tests;
#[cfg(target_os = "macos")]
mod readiness_ffi;
#[cfg(target_os = "macos")]
mod readiness_wait;
#[cfg(target_os = "macos")]
mod registration;
#[cfg(target_os = "macos")]
mod registration_ffi;
#[cfg(target_os = "macos")]
mod terminal_state;
#[cfg(target_os = "macos")]
mod terminal_validation;
mod wake;
mod wake_ffi;

#[cfg(target_os = "macos")]
pub use readiness::{Interest, Readiness, ReadyEvent};
#[cfg(target_os = "macos")]
pub use registration::{Registration, TransitionError};
#[cfg(target_os = "macos")]
pub use terminal_state::{Terminal, TerminalState};
pub use wake::Wake;
