//! Kernel process birth identity: a PID alone is not a process lifetime.
use crate::ProfileError;
use std::ffi::c_void;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessIncarnation {
    seconds: u64,
    microseconds: u64,
}

// macOS SDK sys/proc_info.h: proc_bsdinfo (MAXCOMLEN=16).
// Keep the narrow platform layout here, outside registry/protocol policy.
#[repr(C)]
#[derive(Default)]
struct BsdInfo {
    header: [u32; 12],
    comm: [u8; 16],
    name: [u8; 32],
    scheduling: [u32; 6],
    start_seconds: u64,
    start_microseconds: u64,
}
const _: () = assert!(size_of::<BsdInfo>() == 136);

impl ProcessIncarnation {
    pub(crate) fn parts(self) -> [u64; 2] {
        [self.seconds, self.microseconds]
    }

    pub(crate) fn read(pid: u32) -> Result<Self, ProfileError> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(ProfileError::Daemon("invalid process PID".into()));
        }
        let mut info = BsdInfo::default();
        let length = size_of::<BsdInfo>() as i32;
        // PROC_PIDTBSDINFO=3; the kernel supplies PID and birth timestamp.
        let actual =
            unsafe { proc_pidinfo(pid as i32, 3, 0, (&mut info as *mut BsdInfo).cast(), length) };
        if actual <= 0 {
            return Err(io::Error::last_os_error().into());
        }
        if actual != length
            || info.header[3] != pid
            || info.start_seconds == 0
            || info.start_microseconds >= 1_000_000
        {
            return Err(ProfileError::Daemon(
                "invalid kernel process identity".into(),
            ));
        }
        Ok(Self {
            seconds: info.start_seconds,
            microseconds: info.start_microseconds,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture(seconds: u64) -> Self {
        Self {
            seconds,
            microseconds: 0,
        }
    }
}

unsafe extern "C" {
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, size: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_birth_identity_is_stable_and_reaped_child_is_absent() {
        use std::process::{Command, Stdio};
        let current = ProcessIncarnation::read(std::process::id()).unwrap();
        assert_eq!(
            current,
            ProcessIncarnation::read(std::process::id()).unwrap()
        );
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let result = ProcessIncarnation::read(child.id());
        drop(child.stdin.take());
        child.wait().unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(ProcessIncarnation::read(child.id()).is_err());
        assert!(ProcessIncarnation::read(0).is_err());
    }
}
