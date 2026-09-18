//! Readiness boundary for the profile daemon's nonblocking listener.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::Duration;

pub(crate) fn wait_readable(fd: BorrowedFd<'_>, timeout: Duration) -> io::Result<bool> {
    let deadline = std::time::Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "poll timeout is not representable",
            )
        })?;
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let timeout_ms = remaining
            .as_nanos()
            .div_ceil(1_000_000)
            .min(i32::MAX as u128) as i32;
        // SAFETY: the borrowed descriptor remains live for this bounded call.
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result >= 0 {
            return Ok(result != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wait_readable;
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn queued_peer_wakes_before_the_idle_poll_deadline() {
        let (reader, mut writer) = UnixStream::pair().expect("socket pair");
        let producer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            writer.write_all(&[1]).expect("publish readiness");
        });
        let started = Instant::now();
        assert!(wait_readable(reader.as_fd(), Duration::from_millis(200)).expect("poll"));
        assert!(started.elapsed() < Duration::from_millis(100));
        producer.join().expect("producer");
    }

    #[test]
    fn idle_descriptor_observes_the_requested_timeout() {
        let (reader, _writer) = UnixStream::pair().expect("socket pair");
        let started = Instant::now();
        assert!(!wait_readable(reader.as_fd(), Duration::from_millis(10)).expect("poll"));
        assert!(started.elapsed() >= Duration::from_millis(5));
    }
}
