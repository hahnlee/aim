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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcessObservation {
    Live(ProcessIncarnation),
    Zombie(ProcessIncarnation),
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessObservationDetails {
    pub(crate) observation: ProcessObservation,
    pub(crate) parent_pid: u32,
}

impl ProcessIncarnation {
    pub(crate) fn parts(self) -> [u64; 2] {
        [self.seconds, self.microseconds]
    }

    pub(crate) fn read(pid: u32) -> Result<Self, ProfileError> {
        match Self::observe(pid)? {
            ProcessObservation::Live(incarnation) | ProcessObservation::Zombie(incarnation) => {
                Ok(incarnation)
            }
            ProcessObservation::Absent => Err(io::Error::from_raw_os_error(libc::ESRCH).into()),
        }
    }

    // Authentication/readiness require a running process; resource owners also
    // need `read` to capture short-lived children awaiting their actual reap.
    pub(crate) fn read_live(pid: u32) -> Result<Self, ProfileError> {
        match Self::observe(pid)? {
            ProcessObservation::Live(incarnation) => Ok(incarnation),
            ProcessObservation::Zombie(_) | ProcessObservation::Absent => {
                Err(io::Error::from_raw_os_error(libc::ESRCH).into())
            }
        }
    }

    pub(crate) fn observe(pid: u32) -> Result<ProcessObservation, ProfileError> {
        Ok(Self::observe_with_parent(pid)?.observation)
    }

    pub(crate) fn observe_with_parent(pid: u32) -> Result<ProcessObservationDetails, ProfileError> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(ProfileError::Daemon("invalid process PID".into()));
        }
        let mut info = BsdInfo::default();
        let length = size_of::<BsdInfo>() as i32;
        // PROC_PIDTBSDINFO=3, arg=1 also searches the kernel zombie list.
        // arg=0 reports ESRCH for an unreaped zombie, which is NOT absence.
        // XNU bsd/kern/proc_info.c proc_pidinfo's findzomb admission controls
        // this argument; birth/state/PPID still come from one snapshot.
        let actual =
            unsafe { proc_pidinfo(pid as i32, 3, 1, (&mut info as *mut BsdInfo).cast(), length) };
        if actual <= 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(ProcessObservationDetails {
                    observation: ProcessObservation::Absent,
                    parent_pid: 0,
                });
            }
            return Err(error.into());
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
        let incarnation = Self {
            seconds: info.start_seconds,
            microseconds: info.start_microseconds,
        };
        // proc_bsdinfo.pbi_status uses SZOMB=5 on the pinned macOS SDK.
        let observation = if info.header[1] == 5 {
            ProcessObservation::Zombie(incarnation)
        } else {
            ProcessObservation::Live(incarnation)
        };
        Ok(ProcessObservationDetails {
            observation,
            parent_pid: info.header[4],
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
    fn unreaped_zombie_retains_birth_but_has_no_live_authority() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let ProcessObservation::Zombie(identity) =
                ProcessIncarnation::observe(child.id()).unwrap()
            {
                assert_eq!(ProcessIncarnation::read(child.id()).unwrap(), identity);
                assert!(ProcessIncarnation::read_live(child.id()).is_err());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait().unwrap();
    }
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
