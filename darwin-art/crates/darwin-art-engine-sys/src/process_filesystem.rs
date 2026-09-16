//! Raw ABI for the process-scoped Android filesystem facade.
//!
//! The native owner duplicates the borrowed authority descriptor and copies
//! both byte spans synchronously. No pointer or Rust allocation is retained
//! after the install call returns.

pub type ProcessFilesystemInstallFn = unsafe extern "C" fn(
    root_fd: i32,
    guest_mount: *const u8,
    guest_mount_length: usize,
    cwd: *const u8,
    cwd_length: usize,
) -> i32;
pub type ProcessFilesystemUninstallFn = unsafe extern "C" fn() -> i32;

pub const PROCESS_FILESYSTEM_OK: i32 = 0;
pub const PROCESS_FILESYSTEM_INVALID_ARGUMENT: i32 = 1;
pub const PROCESS_FILESYSTEM_ALREADY_INSTALLED: i32 = 2;
pub const PROCESS_FILESYSTEM_CREATE_FAILED: i32 = 3;
pub const PROCESS_FILESYSTEM_NOT_INSTALLED: i32 = 4;
pub const PROCESS_FILESYSTEM_BUSY: i32 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::mem::size_of;

    #[test]
    fn filesystem_callbacks_are_pointer_sized() {
        assert_eq!(
            size_of::<ProcessFilesystemInstallFn>(),
            size_of::<*mut c_void>()
        );
        assert_eq!(
            size_of::<ProcessFilesystemUninstallFn>(),
            size_of::<*mut c_void>()
        );
    }
}
