use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

/// A nonblocking counter with one level-triggered socket readiness token.
/// The borrowed FD is pollable by macOS; callers must not read or close it.
/// All signal/drain operations serialize the token and counter together.
pub struct Wake {
    reader: UnixStream,
    writer: UnixStream,
    counter: Mutex<u64>,
}

impl Wake {
    pub fn new() -> io::Result<Self> {
        // std creates both descriptors CLOEXEC and owns partial-failure cleanup.
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        Ok(Self {
            reader,
            writer,
            counter: Mutex::new(0),
        })
    }

    pub fn poll_fd(&self) -> BorrowedFd<'_> {
        self.reader.as_fd()
    }

    pub fn signal(&self, increment: u64) -> io::Result<()> {
        if increment == u64::MAX {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let mut count = self.counter.lock().unwrap();
        let next = count
            .checked_add(increment)
            .filter(|value| *value < u64::MAX)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EAGAIN))?;
        if *count == 0 && next != 0 {
            // No writes occur while a token is pending, so the socket cannot fill.
            let mut writer = &self.writer;
            writer.write_all(&[1])?;
        }
        *count = next;
        Ok(())
    }

    pub fn drain(&self) -> io::Result<u64> {
        let mut count = self.counter.lock().unwrap();
        if *count == 0 {
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }
        let mut reader = &self.reader;
        reader.read_exact(&mut [0])?;
        Ok(std::mem::replace(&mut *count, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::sync::Arc;

    fn ready(wake: &Wake, timeout: i32) -> bool {
        let mut event = libc::pollfd {
            fd: wake.poll_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized entry remains valid throughout the call.
        let result = unsafe { libc::poll(&mut event, 1, timeout) };
        assert!(result >= 0);
        result == 1 && event.revents & libc::POLLIN != 0
    }

    #[test]
    fn coalesces_and_drains_without_losing_counter() {
        let wake = Wake::new().unwrap();
        assert!(!ready(&wake, 0));
        assert_eq!(wake.drain().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        wake.signal(0).unwrap();
        assert!(!ready(&wake, 0));
        for _ in 0..10_000 {
            wake.signal(1).unwrap();
        }
        assert!(ready(&wake, 0));
        assert!(ready(&wake, 0));
        assert_eq!(wake.drain().unwrap(), 10_000);
        assert!(!ready(&wake, 0));
        wake.signal(2).unwrap();
        assert_eq!(wake.drain().unwrap(), 2);
    }

    #[test]
    fn bounds_do_not_change_readiness_or_counter() {
        let wake = Wake::new().unwrap();
        assert_eq!(
            wake.signal(u64::MAX).unwrap_err().raw_os_error(),
            Some(libc::EINVAL)
        );
        assert!(!ready(&wake, 0));
        wake.signal(u64::MAX - 1).unwrap();
        assert_eq!(
            wake.signal(1).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(ready(&wake, 0));
        assert_eq!(wake.drain().unwrap(), u64::MAX - 1);
    }

    #[test]
    fn concurrent_signals_wake_host_poll_and_preserve_total() {
        let wake = Arc::new(Wake::new().unwrap());
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let wake = wake.clone();
                std::thread::spawn(move || {
                    for _ in 0..5_000 {
                        wake.signal(1).unwrap();
                    }
                })
            })
            .collect();
        let mut total = 0;
        while total < 20_000 {
            assert!(ready(&wake, 2_000), "lost wake");
            total += wake.drain().unwrap();
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(total, 20_000);
        assert!(!ready(&wake, 0));
    }

    #[test]
    fn descriptors_are_nonblocking_cloexec_and_owned() {
        let wake = Wake::new().unwrap();
        for fd in [wake.reader.as_raw_fd(), wake.writer.as_raw_fd()] {
            // SAFETY: both FDs are owned and live, fcntl only reads their flags.
            assert_ne!(
                unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
            assert_ne!(
                unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
                0
            );
        }
        // Duplicate the reader to observe writer closure without stale-FD races.
        let mut observer = wake.reader.try_clone().unwrap();
        drop(wake);
        assert_eq!(observer.read(&mut [0]).unwrap(), 0);
    }
}
