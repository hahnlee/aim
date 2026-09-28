//! One process-local boundary for non-atomic Darwin FD inheritance windows.
//! Never acquire this lock in a post-fork child or hold it while waiting for
//! socket input, process registration, or process completion.

use std::{
    io,
    process::{Child, Command, Output, Stdio},
    sync::{Mutex, MutexGuard},
};

static INHERITANCE: Mutex<()> = Mutex::new(());

/// Synchronous native operation, borrowing its context only for this call.
/// It must not unwind, block for input, call spawn, or reenter this boundary.
#[cfg(target_os = "macos")]
pub type NativeFdOperation = unsafe extern "C" fn(*mut libc::c_void) -> isize;

/// Provider callback uses this Rust instance's lock, not a second C++ mutex.
/// Native callers must use nonblocking intake and finish ownership/CLOEXEC
/// before returning. No lock token crosses a language or thread boundary.
///
/// # Safety
/// `context` must satisfy the supplied operation's contract; that operation
/// must obey NativeFdOperation's restrictions. Never call after fork/pre_exec.
#[cfg(target_os = "macos")]
pub unsafe extern "C" fn with_native_operation(
    operation: Option<NativeFdOperation>,
    context: *mut libc::c_void,
) -> isize {
    let Some(operation) = operation else {
        unsafe { *libc::__error() = libc::EINVAL };
        return -1;
    };
    let Ok(protected) = guard() else {
        unsafe { *libc::__error() = libc::EIO };
        return -1;
    };
    let result = unsafe { operation(context) };
    let native_errno = unsafe { *libc::__error() };
    drop(protected);
    unsafe { *libc::__error() = native_errno };
    result
}

pub fn guard() -> io::Result<MutexGuard<'static, ()>> {
    INHERITANCE
        .lock()
        .map_err(|_| io::Error::other("FD inheritance boundary poisoned"))
}

pub fn spawn_owned(command: &mut Command) -> io::Result<Child> {
    let _guard = guard()?;
    command.spawn()
}

/// Capture both output streams with null stdin. Completion is outside the
/// protected spawn window; callers must not configure intentional output FDs.
pub fn output_owned(command: &mut Command) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn_owned(command)?.wait_with_output()
}
