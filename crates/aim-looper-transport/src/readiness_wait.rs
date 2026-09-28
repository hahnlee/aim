//! Waiting and terminal-edge suppression; Android still owns message deadlines.
use crate::{Interest, Readiness, ReadyEvent, Terminal};
use std::io;
use std::time::{Duration, Instant};

impl Readiness {
    pub fn wait(&self, capacity: usize, timeout_ms: i32) -> io::Result<Vec<ReadyEvent>> {
        if capacity == 0 || capacity > i32::MAX as usize || timeout_ms < -1 {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let start = Instant::now();
        loop {
            let pending = {
                let mut terminal = self.terminal.lock().unwrap();
                self.validate_terminals(&mut terminal)?;
                terminal.pending().next().is_some()
            };
            let remaining = if pending {
                0
            } else if timeout_ms < 0 {
                -1
            } else {
                let left = Duration::from_millis(timeout_ms as u64).saturating_sub(start.elapsed());
                left.as_millis()
                    .saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0))
                    .min(i32::MAX as u128) as i32
            };
            let raw = self.wait_once(capacity, remaining)?;
            let mut terminal = self.terminal.lock().unwrap();
            let mut ready = Vec::with_capacity(raw.len());
            for event in raw {
                if event.interest == Interest::ErrorOnly {
                    // Mode comes from the returned kernel event, not mutable
                    // registration state. A removed/replaced error-only watch
                    // must never turn an old data edge into ordinary READ.
                    terminal.observe_sequence(
                        event.token,
                        Terminal {
                            eof: event.eof,
                            error: event.error,
                        },
                    );
                } else {
                    ready.push(event);
                }
            }
            self.validate_terminals(&mut terminal)?;
            ready.extend(terminal.pending().map(|(_, token, state)| ReadyEvent {
                token,
                interest: Interest::ErrorOnly,
                eof: state.eof,
                error: state.error,
            }));
            drop(terminal);
            if !ready.is_empty() {
                // Mix persistent terminal readiness with newly polled descriptors.
                // No terminal-only fast path that starves ordinary readable FDs.
                let mut cursor = self.cursor.lock().unwrap();
                let len = ready.len();
                ready.rotate_left(*cursor % len);
                ready.truncate(capacity);
                *cursor = (*cursor + ready.len()) % len;
                return Ok(ready);
            }
            if timeout_ms >= 0 && start.elapsed() >= Duration::from_millis(timeout_ms as u64) {
                return Ok(vec![]);
            }
            // Ordinary data edge was consumed. Continue with the original
            // deadline, not a new timeout. EV_CLEAR prevents idle-data spin.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn actual_error_only_socket_suppresses_data_and_latches_eof() {
        let queue = Readiness::new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        queue
            .register(reader.as_fd(), Interest::ErrorOnly, 7)
            .unwrap();
        writer.write_all(b"unread").unwrap();
        let start = Instant::now();
        assert!(queue.wait(16, 10).unwrap().is_empty());
        assert!(start.elapsed() >= Duration::from_millis(10));
        drop(writer);
        for _ in 0..3 {
            let events = queue.wait(16, 1000).unwrap();
            assert_eq!(
                events,
                vec![ReadyEvent {
                    token: 7,
                    interest: Interest::ErrorOnly,
                    eof: true,
                    error: false
                }]
            );
        }
        queue.remove(reader.as_fd(), Interest::ErrorOnly).unwrap();
        assert!(queue.wait(16, 0).unwrap().is_empty());
    }

    #[test]
    fn pipe_terminal_and_normal_readiness_both_progress() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut fds = [-1; 2];
        // SAFETY: writable two-descriptor output; successful pipe transfers ownership.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both successful pipe outputs are exclusively owned.
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        let queue = Readiness::new().unwrap();
        queue
            .register(read.as_fd(), Interest::ErrorOnly, 5)
            .unwrap();
        drop(write);
        assert_eq!(queue.wait(1, 1000).unwrap()[0].token, 5);
        let (normal, mut peer) = UnixStream::pair().unwrap();
        queue.register(normal.as_fd(), Interest::Read, 6).unwrap();
        peer.write_all(b"x").unwrap();
        let mut seen = vec![];
        for _ in 0..4 {
            seen.push(queue.wait(1, 0).unwrap()[0].token);
        }
        assert!(seen.contains(&5) && seen.contains(&6));
        queue.remove(read.as_fd(), Interest::ErrorOnly).unwrap();
        assert_eq!(queue.wait(1, 0).unwrap()[0].token, 6);
    }

    #[test]
    fn explicit_filter_replacement_restores_level_triggering() {
        let queue = Readiness::new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        queue
            .register(reader.as_fd(), Interest::ErrorOnly, 5)
            .unwrap();
        writer.write_all(b"x").unwrap();
        assert!(queue.wait(16, 0).unwrap().is_empty());
        assert_eq!(
            queue
                .register(reader.as_fd(), Interest::Read, 6)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        assert!(queue.wait(16, 0).unwrap().is_empty());
        queue.remove(reader.as_fd(), Interest::ErrorOnly).unwrap();
        queue.register(reader.as_fd(), Interest::Read, 6).unwrap();
        for _ in 0..2 {
            let events = queue.wait(16, 0).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].interest, Interest::Read);
            assert_eq!(events[0].token, 6);
        }
    }
}
