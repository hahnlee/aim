//! Guest `/proc` and `/sys` files, read through the bionic filesystem facade.
//!
//! The facade serves the device view's single capability contract (see
//! tools/bionic-fs-facade); natives that parse procfs on Android read these
//! files instead of the host's, so every caller sees the same device.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::{c_char, c_int};
use std::sync::OnceLock;

type Open = unsafe extern "C" fn(*const c_char, c_int, u32) -> c_int;
type Read = unsafe extern "C" fn(c_int, *mut c_void, usize) -> isize;
type Close = unsafe extern "C" fn(c_int) -> c_int;

/// Android's O_CLOEXEC, as the facade's open expects Android flags.
const ANDROID_O_CLOEXEC: c_int = 0o2000000;

struct Facade {
    open: Open,
    read: Read,
    close: Close,
}

fn facade() -> Option<&'static Facade> {
    static FACADE: OnceLock<Option<Facade>> = OnceLock::new();
    FACADE
        .get_or_init(|| {
            let symbol = |name: &CStr| unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
            let (open, read, close) = (
                symbol(c"aim_bionic_open"),
                symbol(c"aim_bionic_read"),
                symbol(c"aim_bionic_close"),
            );
            if open.is_null() || read.is_null() || close.is_null() {
                return None;
            }
            // SAFETY: the facade exports these with the declared signatures.
            unsafe {
                Some(Facade {
                    open: std::mem::transmute::<*mut c_void, Open>(open),
                    read: std::mem::transmute::<*mut c_void, Read>(read),
                    close: std::mem::transmute::<*mut c_void, Close>(close),
                })
            }
        })
        .as_ref()
}

/// The whole guest file, or None when this process has no facade or the
/// device view does not provide it.
pub fn read(path: &str) -> Option<String> {
    let facade = facade()?;
    let path = CString::new(path).ok()?;
    let fd = unsafe { (facade.open)(path.as_ptr(), ANDROID_O_CLOEXEC, 0) };
    if fd < 0 {
        return None;
    }
    let mut contents = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = unsafe { (facade.read)(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count <= 0 {
            break;
        }
        contents.extend_from_slice(&buffer[..count as usize]);
    }
    unsafe { (facade.close)(fd) };
    Some(String::from_utf8_lossy(&contents).into_owned())
}

/// The kB value of a `Tag:   value kB` line at the start of a line.
pub fn kb_field(contents: &str, tag: &str) -> Option<u64> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(tag))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

/// One field of the guest /proc/meminfo.
pub fn meminfo_kb(tag: &str) -> Option<u64> {
    kb_field(&read("/proc/meminfo")?, tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_kb_fields_at_line_starts() {
        let contents =
            "MemTotal:        8388608 kB\nSwapTotal:             0 kB\nVmRSS:\t   1234 kB\n";
        assert_eq!(kb_field(contents, "MemTotal:"), Some(8388608));
        assert_eq!(kb_field(contents, "SwapTotal:"), Some(0));
        assert_eq!(kb_field(contents, "VmRSS:"), Some(1234));
        assert_eq!(kb_field(contents, "Total:"), None);
    }
}
