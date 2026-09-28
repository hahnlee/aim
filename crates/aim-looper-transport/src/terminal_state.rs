//! Sequence-scoped terminal readiness, separate from Android callback policy.
//! The native queue must serialize successful kernel changes with this state.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Terminal {
    pub eof: bool,
    pub error: bool,
}

impl Terminal {
    fn ready(self) -> bool {
        self.eof || self.error
    }
}

#[derive(Clone, Copy, Debug)]
struct Watch {
    sequence: u64,
    terminal: Terminal,
}

#[derive(Default)]
pub struct TerminalState {
    by_fd: BTreeMap<i32, Watch>,
}

impl TerminalState {
    /// Kernel update keeps the same watch, including already consumed EOF.
    pub fn update(&mut self, fd: i32, sequence: u64) {
        if let Some(watch) = self.by_fd.get_mut(&fd) {
            watch.sequence = sequence;
        } else {
            self.register(fd, sequence);
        }
    }
    pub fn contains_fd(&self, fd: i32) -> bool {
        self.by_fd.contains_key(&fd)
    }
    pub fn contains_sequence(&self, sequence: u64) -> bool {
        self.by_fd.values().any(|watch| watch.sequence == sequence)
    }

    pub fn observe_sequence(&mut self, sequence: u64, event: Terminal) {
        for watch in self
            .by_fd
            .values_mut()
            .filter(|watch| watch.sequence == sequence)
        {
            watch.terminal.eof |= event.eof;
            watch.terminal.error |= event.error;
        }
    }

    pub fn remove_fd(&mut self, fd: i32) {
        self.by_fd.remove(&fd);
    }
    /// Commit only after successful kernel registration/reconfiguration. Reusing
    /// an FD number or sequence starts a fresh watch, never inherits old EOF.
    pub fn register(&mut self, fd: i32, sequence: u64) {
        self.by_fd.insert(
            fd,
            Watch {
                sequence,
                terminal: Terminal::default(),
            },
        );
    }

    /// Cleanup for an old callback must not remove a recycled FD's new watch.
    pub fn remove(&mut self, fd: i32, sequence: u64) {
        if self
            .by_fd
            .get(&fd)
            .is_some_and(|watch| watch.sequence == sequence)
        {
            self.by_fd.remove(&fd);
        }
    }

    /// Ignore ordinary data and stale kernel batches. Terminal bits only grow
    /// within a registration, supporting repeated level-triggered observation.
    pub fn observe(&mut self, fd: i32, sequence: u64, event: Terminal) -> bool {
        let Some(watch) = self.by_fd.get_mut(&fd) else {
            return false;
        };
        if watch.sequence != sequence {
            return false;
        }
        watch.terminal.eof |= event.eof;
        watch.terminal.error |= event.error;
        watch.terminal.ready()
    }

    pub fn pending(&self) -> impl Iterator<Item = (i32, u64, Terminal)> + '_ {
        self.by_fd.iter().filter_map(|(&fd, watch)| {
            watch
                .terminal
                .ready()
                .then_some((fd, watch.sequence, watch.terminal))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_data_is_suppressed_and_terminal_state_repeats() {
        let mut state = TerminalState::default();
        state.register(7, u64::MAX);
        assert!(!state.observe(7, u64::MAX, Terminal::default()));
        assert_eq!(state.pending().count(), 0);
        let eof = Terminal {
            eof: true,
            error: false,
        };
        assert!(state.observe(7, u64::MAX, eof));
        for _ in 0..3 {
            assert_eq!(
                state.pending().collect::<Vec<_>>(),
                vec![(7, u64::MAX, eof)]
            );
        }
        assert!(state.observe(
            7,
            u64::MAX,
            Terminal {
                eof: false,
                error: true
            }
        ));
        assert_eq!(
            state.pending().next().unwrap().2,
            Terminal {
                eof: true,
                error: true
            }
        );
    }

    #[test]
    fn recycled_descriptor_rejects_old_observation_and_cleanup() {
        let mut state = TerminalState::default();
        state.register(7, 11);
        state.observe(
            7,
            11,
            Terminal {
                eof: true,
                error: false,
            },
        );
        state.register(7, 12);
        assert_eq!(state.pending().count(), 0);
        assert!(!state.observe(
            7,
            11,
            Terminal {
                eof: true,
                error: true
            }
        ));
        state.remove(7, 11);
        assert!(state.observe(
            7,
            12,
            Terminal {
                eof: true,
                error: false
            }
        ));
        state.remove(7, 12);
        assert_eq!(state.pending().count(), 0);
        assert!(!state.observe(
            7,
            12,
            Terminal {
                eof: true,
                error: true
            }
        ));
    }

    #[test]
    fn independent_registrations_and_same_sequence_rearm() {
        let mut state = TerminalState::default();
        for fd in [5, 6] {
            state.register(fd, fd as u64);
        }
        state.observe(
            5,
            5,
            Terminal {
                eof: true,
                error: false,
            },
        );
        state.observe(
            6,
            6,
            Terminal {
                eof: false,
                error: true,
            },
        );
        state.register(5, 5);
        assert_eq!(
            state.pending().collect::<Vec<_>>(),
            vec![(
                6,
                6,
                Terminal {
                    eof: false,
                    error: true
                }
            )]
        );
    }
}
