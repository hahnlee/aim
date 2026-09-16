use crate::TerminalState;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interest {
    Read,
    Write,
    ErrorOnly,
}

impl Interest {
    fn filter(self) -> i16 {
        match self {
            Self::Read | Self::ErrorOnly => libc::EVFILT_READ,
            Self::Write => libc::EVFILT_WRITE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadyEvent {
    pub token: u64,
    pub interest: Interest,
    pub eof: bool,
    pub error: bool,
}

/// Native kernel readiness owner. Does not own or close registered descriptors.
/// Registration is per filter, level-triggered, and replaced by `register`.
/// This is not an epoll ABI: its caller must retain Android registration policy.
pub struct Readiness {
    pub(crate) queue: OwnedFd,
    pub(crate) terminal: Mutex<TerminalState>,
    pub(crate) cursor: Mutex<usize>,
}

impl Readiness {
    pub fn new() -> io::Result<Self> {
        // SAFETY: kqueue has no pointer arguments and returns a newly owned FD.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: kqueue transferred exclusive descriptor ownership.
        let queue = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: live owned FD, integer flag argument.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            queue,
            terminal: Mutex::new(TerminalState::default()),
            cursor: Mutex::new(0),
        })
    }

    fn change(&self, fd: i32, interest: Interest, flags: u16, token: u64) -> io::Result<()> {
        if fd < 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        let mut terminal = self.terminal.lock().unwrap();
        // EV_CLEAR is sticky. The owner must remove/add with rollback when
        // converting to level-triggered read, not silently reuse the knote.
        if interest == Interest::Read && flags & libc::EV_DELETE == 0 && terminal.contains_fd(fd) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let flags = if interest == Interest::ErrorOnly && flags & libc::EV_DELETE == 0 {
            flags | libc::EV_CLEAR
        } else {
            flags
        };
        let event = libc::kevent64_s {
            ident: fd as u64,
            filter: interest.filter(),
            flags,
            fflags: 0,
            data: 0,
            udata: token,
            ext: [0; 2],
        };
        // No output list: apply changes without consuming pending readiness.
        // SAFETY: one initialized input, no output buffer or timeout dereference.
        let result = unsafe {
            libc::kevent64(
                self.queue.as_raw_fd(),
                &event,
                1,
                std::ptr::null_mut(),
                0,
                0,
                std::ptr::null(),
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::EBADF)) {
                terminal.remove_fd(fd);
            }
            // kqueue can report a missing knote before validating the target FD.
            // Distinguish closed FD from a live FD whose old watch disappeared.
            // SAFETY: fcntl only queries an integer descriptor; no Rust borrow
            // or ownership is manufactured for a potentially closed handle.
            if error.raw_os_error() == Some(libc::ENOENT)
                && unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::EBADF)
            {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            Err(error)
        } else {
            if interest == Interest::ErrorOnly && flags & libc::EV_DELETE == 0 {
                if flags & libc::EV_ADD != 0 {
                    terminal.register(fd, token);
                } else {
                    terminal.update(fd, token);
                }
            } else if interest != Interest::Write {
                terminal.remove_fd(fd);
            }
            Ok(())
        }
    }

    pub fn register(&self, fd: BorrowedFd<'_>, interest: Interest, token: u64) -> io::Result<()> {
        self.register_raw(fd.as_raw_fd(), interest, token)
    }

    // Native callers may unregister a closed descriptor. Pass integer identity
    // to the kernel without fabricating a Rust BorrowedFd lifetime guarantee.
    pub(crate) fn register_raw(&self, fd: i32, interest: Interest, token: u64) -> io::Result<()> {
        self.change(fd, interest, libc::EV_ADD | libc::EV_ENABLE, token)
    }

    pub fn remove(&self, fd: BorrowedFd<'_>, interest: Interest) -> io::Result<()> {
        self.remove_raw(fd.as_raw_fd(), interest)
    }

    pub(crate) fn remove_raw(&self, fd: i32, interest: Interest) -> io::Result<()> {
        self.change(fd, interest, libc::EV_DELETE, 0)
    }

    /// Update only an existing kernel registration. Unlike EV_ADD, this must
    /// report ENOENT after a descriptor was closed/recycled, allowing Android's
    /// original Looper to take its add-and-rebuild path.
    pub fn update(&self, fd: BorrowedFd<'_>, interest: Interest, token: u64) -> io::Result<()> {
        self.update_raw(fd.as_raw_fd(), interest, token)
    }

    pub(crate) fn update_raw(&self, fd: i32, interest: Interest, token: u64) -> io::Result<()> {
        self.change(fd, interest, libc::EV_ENABLE, token)
    }

    /// One bounded kernel wait. EINTR is returned so Android can recompute its
    /// message deadline. -1 waits indefinitely; nonnegative values are milliseconds.
    pub(crate) fn wait_once(
        &self,
        capacity: usize,
        timeout_ms: i32,
    ) -> io::Result<Vec<ReadyEvent>> {
        if capacity == 0 || capacity > i32::MAX as usize || timeout_ms < -1 {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let zero = libc::kevent64_s {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: 0,
            ext: [0; 2],
        };
        let mut events = vec![zero; capacity];
        let timeout = libc::timespec {
            tv_sec: (timeout_ms.max(0) / 1000) as _,
            tv_nsec: (timeout_ms.max(0) % 1000) as libc::c_long * 1_000_000,
        };
        let timeout_ptr = if timeout_ms < 0 {
            std::ptr::null()
        } else {
            &timeout
        };
        // SAFETY: output vector has capacity initialized writable elements;
        // timeout remains live and there are no input changes.
        let count = unsafe {
            libc::kevent64(
                self.queue.as_raw_fd(),
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                capacity as i32,
                0,
                timeout_ptr,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        events.truncate(count as usize);
        events
            .into_iter()
            .map(|event| {
                let interest = match event.filter {
                    libc::EVFILT_READ if event.flags & libc::EV_CLEAR != 0 => Interest::ErrorOnly,
                    libc::EVFILT_READ => Interest::Read,
                    libc::EVFILT_WRITE => Interest::Write,
                    _ => return Err(io::Error::from_raw_os_error(libc::EIO)),
                };
                Ok(ReadyEvent {
                    token: event.udata,
                    interest,
                    eof: event.flags & libc::EV_EOF != 0,
                    error: event.flags & libc::EV_ERROR != 0 || event.fflags != 0,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Wake;
    use std::io::{Read, Write};
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn wake_level_trigger_update_remove_and_timeout() {
        let queue = Readiness::new().unwrap();
        let wake = Wake::new().unwrap();
        queue
            .register(wake.poll_fd(), Interest::Read, u64::MAX)
            .unwrap();
        assert!(queue.wait(16, 1).unwrap().is_empty());
        wake.signal(1).unwrap();
        for _ in 0..2 {
            assert_eq!(
                queue.wait(16, 0).unwrap(),
                vec![ReadyEvent {
                    token: u64::MAX,
                    interest: Interest::Read,
                    eof: false,
                    error: false,
                }]
            );
        }
        queue.register(wake.poll_fd(), Interest::Read, 2).unwrap();
        assert_eq!(queue.wait(16, 0).unwrap()[0].token, 2);
        assert_eq!(wake.drain().unwrap(), 1);
        assert!(queue.wait(16, 0).unwrap().is_empty());
        queue.remove(wake.poll_fd(), Interest::Read).unwrap();
        wake.signal(1).unwrap();
        assert!(queue.wait(16, 0).unwrap().is_empty());
        assert_eq!(
            queue
                .remove(wake.poll_fd(), Interest::Read)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );
    }

    #[test]
    fn socket_read_write_and_eof_preserve_tokens() {
        let queue = Readiness::new().unwrap();
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        queue.register(reader.as_fd(), Interest::Read, 11).unwrap();
        queue.register(reader.as_fd(), Interest::Write, 12).unwrap();
        writer.write_all(b"x").unwrap();
        let events = queue.wait(16, 1000).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.token == 11 && e.interest == Interest::Read)
        );
        assert!(
            events
                .iter()
                .any(|e| e.token == 12 && e.interest == Interest::Write)
        );
        reader.read_exact(&mut [0]).unwrap();
        queue.remove(reader.as_fd(), Interest::Write).unwrap();
        drop(writer);
        let events = queue.wait(16, 1000).unwrap();
        assert!(events.iter().any(|e| e.token == 11 && e.eof));
        drop(queue);
        // Queue ownership never consumes the caller's registered descriptor.
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn cross_thread_wake_and_invalid_wait() {
        let queue = Readiness::new().unwrap();
        assert_eq!(
            queue.wait(0, 0).unwrap_err().raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(
            queue.wait(1, -2).unwrap_err().raw_os_error(),
            Some(libc::EINVAL)
        );
        let wake = std::sync::Arc::new(Wake::new().unwrap());
        queue.register(wake.poll_fd(), Interest::Read, 99).unwrap();
        let sender = wake.clone();
        let thread = std::thread::spawn(move || sender.signal(1).unwrap());
        assert_eq!(queue.wait(16, 2000).unwrap()[0].token, 99);
        thread.join().unwrap();
        assert_eq!(wake.drain().unwrap(), 1);
    }

    #[test]
    fn update_missing_and_recycled_descriptor_preserves_enoent() {
        use std::os::fd::IntoRawFd;
        let queue = Readiness::new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        assert_eq!(
            queue
                .update(reader.as_fd(), Interest::Read, 7)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );
        queue.register(reader.as_fd(), Interest::Read, 7).unwrap();
        queue.update(reader.as_fd(), Interest::Read, 8).unwrap();
        writer.write_all(b"x").unwrap();
        assert_eq!(queue.wait(16, 0).unwrap()[0].token, 8);

        // Prepare a distinct socket before replacing the old FD atomically.
        let (replacement, mut peer) = UnixStream::pair().unwrap();
        let fd = reader.into_raw_fd();
        // SAFETY: this test exclusively owns both descriptors. dup2 closes the
        // old kernel object and replaces exactly the owned target, no reuse race.
        assert_eq!(unsafe { libc::dup2(replacement.as_raw_fd(), fd) }, fd);
        // SAFETY: the raw target was transferred by into_raw_fd and is live.
        let recycled = unsafe { OwnedFd::from_raw_fd(fd) };
        assert_eq!(
            queue
                .update(recycled.as_fd(), Interest::Read, 9)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );
        queue.register(recycled.as_fd(), Interest::Read, 9).unwrap();
        peer.write_all(b"y").unwrap();
        let events = queue.wait(16, 1000).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token, 9);
    }
}
