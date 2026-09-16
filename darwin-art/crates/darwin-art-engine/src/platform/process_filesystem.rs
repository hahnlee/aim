//! Safe engine-side owner for the process-scoped Android filesystem facade.
//!
//! The caller lends an authority descriptor only for the synchronous native
//! install call. The native owner duplicates it before returning, so this
//! module never retains the caller's descriptor or borrowed byte slices.

use darwin_art_engine_sys::{
    PROCESS_FILESYSTEM_ALREADY_INSTALLED, PROCESS_FILESYSTEM_OK, ProcessFilesystemInstallFn,
};
use std::fmt;
use std::os::fd::{AsRawFd, BorrowedFd};

pub(crate) fn install_with(
    root_fd: BorrowedFd<'_>,
    guest_mount: &[u8],
    cwd: &[u8],
    install: ProcessFilesystemInstallFn,
) -> Result<(), ProcessFilesystemError> {
    if guest_mount.is_empty() || cwd.is_empty() {
        return Err(ProcessFilesystemError::InvalidArgument);
    }

    // SAFETY: `root_fd` is borrowed for this call and the native owner
    // duplicates it synchronously. The slices remain alive for the entire
    // call and are copied by the native implementation before return.
    let status = unsafe {
        install(
            root_fd.as_raw_fd(),
            guest_mount.as_ptr(),
            guest_mount.len(),
            cwd.as_ptr(),
            cwd.len(),
        )
    };
    match status {
        PROCESS_FILESYSTEM_OK => Ok(()),
        PROCESS_FILESYSTEM_ALREADY_INSTALLED => Err(ProcessFilesystemError::AlreadyInstalled),
        1 => Err(ProcessFilesystemError::InvalidArgument),
        other => Err(ProcessFilesystemError::NativeFailure(other)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessFilesystemError {
    EngineClosed,
    AlreadyInstalled,
    InvalidArgument,
    NativeFailure(i32),
}

impl fmt::Display for ProcessFilesystemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EngineClosed => formatter.write_str("process engine is closed"),
            Self::AlreadyInstalled => formatter.write_str("process filesystem already installed"),
            Self::InvalidArgument => formatter.write_str("invalid process filesystem argument"),
            Self::NativeFailure(status) => {
                write!(formatter, "process filesystem install failed ({status})")
            }
        }
    }
}

impl std::error::Error for ProcessFilesystemError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::sync::Mutex;

    type CapturedFilesystemCall = (i32, Vec<u8>, Vec<u8>);
    static CALL: Mutex<Option<CapturedFilesystemCall>> = Mutex::new(None);

    unsafe extern "C" fn install_callback(
        root_fd: i32,
        guest_mount: *const u8,
        guest_mount_length: usize,
        cwd: *const u8,
        cwd_length: usize,
    ) -> i32 {
        // SAFETY: the production contract requires both spans to be readable
        // for their supplied lengths during the synchronous call.
        let guest_mount = unsafe { std::slice::from_raw_parts(guest_mount, guest_mount_length) };
        // SAFETY: same contract as guest_mount.
        let cwd = unsafe { std::slice::from_raw_parts(cwd, cwd_length) };
        *CALL.lock().unwrap() = Some((root_fd, guest_mount.to_vec(), cwd.to_vec()));
        PROCESS_FILESYSTEM_OK
    }

    #[test]
    fn install_lends_fd_and_copies_input_spans() {
        let file = File::open("/").unwrap();
        let result = install_with(
            // SAFETY: file remains open through the synchronous call.
            unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) },
            b"/system",
            b"/system/bin",
            install_callback,
        );
        assert_eq!(result, Ok(()));
        let call = CALL.lock().unwrap().take().unwrap();
        assert_eq!(call.0, file.as_raw_fd());
        assert_eq!(call.1, b"/system");
        assert_eq!(call.2, b"/system/bin");
    }

    #[test]
    fn empty_paths_are_rejected_before_native_call() {
        let file = File::open("/").unwrap();
        assert_eq!(
            install_with(
                // SAFETY: file remains open through the synchronous call.
                unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) },
                &[],
                b"/",
                install_callback,
            ),
            Err(ProcessFilesystemError::InvalidArgument)
        );
    }

    #[test]
    fn native_duplicate_status_is_preserved_as_typed_error() {
        unsafe extern "C" fn duplicate(
            _: i32,
            _: *const u8,
            _: usize,
            _: *const u8,
            _: usize,
        ) -> i32 {
            PROCESS_FILESYSTEM_ALREADY_INSTALLED
        }
        let file = File::open("/").unwrap();
        assert_eq!(
            install_with(
                // SAFETY: file remains open through the synchronous call.
                unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) },
                b"/",
                b"/",
                duplicate,
            ),
            Err(ProcessFilesystemError::AlreadyInstalled)
        );
    }
}
