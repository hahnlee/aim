//! Kernel-watch lifetime validation for cached terminal readiness.
use crate::{Readiness, TerminalState};
use std::io;
use std::os::fd::AsRawFd;

impl Readiness {
    /// Caller holds the terminal mutex, serializing this check with all changes.
    /// Query the existing knote rather than fcntl(fd): a recycled FD can be live
    /// without belonging to this queue. No EV_ADD, so validation cannot recreate
    /// a watch after close. No event output, so readiness is not consumed.
    pub(crate) fn validate_terminals(&self, state: &mut TerminalState) -> io::Result<()> {
        let pending: Vec<_> = state.pending().map(|(fd, token, _)| (fd, token)).collect();
        for (fd, token) in pending {
            let event = libc::kevent64_s {
                ident: fd as u64,
                filter: libc::EVFILT_READ,
                flags: libc::EV_ENABLE,
                fflags: 0,
                data: 0,
                udata: token,
                ext: [0; 2],
            };
            // SAFETY: initialized single change, live queue, no output pointers.
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
                    state.remove(fd, token);
                } else {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Interest;
    use std::io::Write;
    use std::os::fd::{AsFd, FromRawFd, IntoRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    #[test]
    fn closed_watch_does_not_keep_cached_eof_alive() {
        let queue = Readiness::new().unwrap();
        let (reader, writer) = UnixStream::pair().unwrap();
        queue
            .register(reader.as_fd(), Interest::ErrorOnly, 1)
            .unwrap();
        drop(writer);
        assert!(queue.wait(4, 1000).unwrap()[0].eof);
        drop(reader);
        assert!(queue.wait(4, 0).unwrap().is_empty());
        assert_eq!(queue.terminal.lock().unwrap().pending().count(), 0);
    }

    #[test]
    fn live_recycled_fd_does_not_inherit_cached_terminal() {
        let queue = Readiness::new().unwrap();
        let (reader, writer) = UnixStream::pair().unwrap();
        queue
            .register(reader.as_fd(), Interest::ErrorOnly, 1)
            .unwrap();
        drop(writer);
        assert!(queue.wait(4, 1000).unwrap()[0].eof);
        let (replacement, mut peer) = UnixStream::pair().unwrap();
        let fd = reader.into_raw_fd();
        // SAFETY: exclusively owned source/target; atomically closes the old
        // socket and reuses exactly its number without an intervening FD race.
        assert_eq!(unsafe { libc::dup2(replacement.as_raw_fd(), fd) }, fd);
        // SAFETY: into_raw_fd transferred this target's exclusive ownership.
        let recycled = unsafe { OwnedFd::from_raw_fd(fd) };
        peer.write_all(b"new").unwrap();
        assert!(queue.wait(4, 0).unwrap().is_empty());
        queue.register(recycled.as_fd(), Interest::Read, 2).unwrap();
        let events = queue.wait(4, 1000).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token, 2);
        assert_eq!(events[0].interest, Interest::Read);
        assert!(!events[0].eof);
    }
}
