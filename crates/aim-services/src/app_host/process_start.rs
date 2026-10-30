//! Process-start death notification, before an ApplicationThread exists.
//!
//! Until attachApplication there is no ApplicationThread whose Binder death
//! retires a start. Our processes are Darwin processes, so NOTE_EXIT tells the
//! host of a death in that window instead of the start timeout.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

pub(super) struct ExitWatch(OwnedFd);

impl ExitWatch {
    pub(super) fn new(pid: i32) -> io::Result<Self> {
        if pid <= 0 {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let identity = aim_storage::process_namespace::ProcessIdentity::running_strict(pid)?;
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let watch = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        let event = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // Register before returning: a death before the worker runs must
        // remain queued. The kernel registration follows this process,
        // including exec, rather than a later reuse of its pid.
        if unsafe { libc::kevent(fd, &event, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if aim_storage::process_namespace::ProcessIdentity::running_strict(pid)? != identity {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(watch)
    }

    pub(super) fn wait(&self, timeout: Duration) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let timeout = libc::timespec {
                tv_sec: left.as_secs() as _,
                tv_nsec: left.subsec_nanos() as _,
            };
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            let n = unsafe {
                libc::kevent(
                    self.0.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    &timeout,
                )
            };
            if n == 0 {
                return Ok(false);
            }
            if n < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if event.flags & libc::EV_ERROR != 0 {
                return Err(io::Error::from_raw_os_error(event.data as i32));
            }
            if event.filter != libc::EVFILT_PROC || event.fflags & libc::NOTE_EXIT == 0 {
                return Err(io::Error::from_raw_os_error(libc::EPROTO));
            }
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            self.0.wait().unwrap();
        }
    }

    #[test]
    fn exit_before_wait_is_retained_and_live_process_does_not_signal() {
        let mut child = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let watch = ExitWatch::new(child.0.id() as i32).unwrap();
        assert!(!watch.wait(Duration::from_millis(10)).unwrap());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(watch.wait(Duration::from_secs(2)).unwrap());
    }

    #[test]
    fn death_before_application_thread_retires_start() {
        let mut child = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id() as i32;
        let host = super::super::tests::host_with_start(7, std::sync::Arc::new(|| {}));
        host.apps.lock().unwrap().get_mut(&7).unwrap().pid = pid;
        host.watch_start(7).unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.apps.lock().unwrap().contains_key(&7) {
            assert!(
                Instant::now() < deadline,
                "unattached start survived process death"
            );
            std::thread::yield_now();
        }
        // A late timeout or Binder notification is harmless to a newer start.
        host.process_died(7, pid);
        host.start_timed_out(7);
        assert!(host.apps.lock().unwrap().is_empty());
    }

    #[test]
    fn an_already_reaped_process_is_not_a_live_start() {
        let mut child = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let pid = child.0.id() as i32;
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert_eq!(
            ExitWatch::new(pid).err().unwrap().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn a_start_retired_before_registration_needs_no_watcher() {
        let host = super::super::tests::host_with_start(7, std::sync::Arc::new(|| {}));
        host.apps.lock().unwrap().remove(&7);
        host.watch_start(7).unwrap();
    }

    #[test]
    fn death_notifications_match_both_start_and_pid() {
        let host = super::super::tests::host_with_start(7, std::sync::Arc::new(|| {}));
        host.process_died(7, 4322);
        host.process_died(8, 4321);
        assert!(host.apps.lock().unwrap().contains_key(&7));
        host.process_died(7, 4321);
        assert!(host.apps.lock().unwrap().is_empty());
    }
}
