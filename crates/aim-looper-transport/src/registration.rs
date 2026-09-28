//! Paired kernel-filter transition, not Android's callback registry.
use crate::{Interest, Readiness};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registration {
    pub read: bool,
    pub write: bool,
    pub error_only: bool,
    pub token: u64,
}

impl Registration {
    fn wants(self, interest: Interest) -> bool {
        match interest {
            Interest::Read => self.read,
            Interest::Write => self.write,
            Interest::ErrorOnly => self.error_only,
        }
    }
}

#[derive(Debug)]
pub struct TransitionError {
    pub operation: io::Error,
    /// Some means the prior kernel state could not be restored. The caller must
    /// rebuild/discard this readiness set, never claim the old state is intact.
    pub rollback: Option<io::Error>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Register(u64),
    Update(u64),
    Remove,
}

fn transition(
    previous: Registration,
    next: Registration,
    mut apply: impl FnMut(Interest, Change) -> io::Result<()>,
) -> Result<(), TransitionError> {
    if [previous, next]
        .iter()
        .any(|r| r.error_only && (r.read || r.write))
    {
        return Err(TransitionError {
            operation: io::Error::from_raw_os_error(libc::EINVAL),
            rollback: None,
        });
    }
    // Validate previous filters in the kernel, preserving recycled-FD ENOENT.
    for interest in [Interest::Read, Interest::Write, Interest::ErrorOnly] {
        if previous.wants(interest) {
            apply(interest, Change::Update(previous.token)).map_err(|operation| {
                TransitionError {
                    operation,
                    rollback: None,
                }
            })?;
        }
    }
    // Remove obsolete filters first: Read and ErrorOnly share a kernel filter,
    // but EV_CLEAR cannot be cleared by updating that filter in place.
    let interests = [Interest::Read, Interest::Write, Interest::ErrorOnly];
    let mut plan = Vec::new();
    for interest in interests {
        if previous.wants(interest) && !next.wants(interest) {
            plan.push((interest, Change::Remove, Change::Register(previous.token)));
        }
    }
    for interest in interests {
        if next.wants(interest) {
            plan.push(if previous.wants(interest) {
                (
                    interest,
                    Change::Update(next.token),
                    Change::Update(previous.token),
                )
            } else {
                (interest, Change::Register(next.token), Change::Remove)
            });
        }
    }
    let mut changed = Vec::new();
    for (interest, change, undo) in plan {
        if let Err(operation) = apply(interest, change) {
            let mut rollback = None;
            for (restored, restore) in changed.into_iter().rev() {
                if let Err(error) = apply(restored, restore) {
                    rollback.get_or_insert(error);
                }
            }
            return Err(TransitionError {
                operation,
                rollback,
            });
        }
        changed.push((interest, undo));
    }
    Ok(())
}

impl Readiness {
    /// The caller owns the authoritative previous mask/token and serializes
    /// transitions/close for this host FD. No callback ownership is transferred.
    /// An empty mask means no filters, not Linux's implicit error-only watch.
    pub fn transition(
        &self,
        fd: BorrowedFd<'_>,
        previous: Registration,
        next: Registration,
    ) -> Result<(), TransitionError> {
        self.transition_raw(fd.as_raw_fd(), previous, next)
    }

    pub(crate) fn transition_raw(
        &self,
        fd: i32,
        previous: Registration,
        next: Registration,
    ) -> Result<(), TransitionError> {
        transition(previous, next, |interest, change| match change {
            Change::Register(token) => self.register_raw(fd, interest, token),
            Change::Update(token) => self.update_raw(fd, interest, token),
            Change::Remove => self.remove_raw(fd, interest),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    const EMPTY: Registration = Registration {
        read: false,
        write: false,
        error_only: false,
        token: 0,
    };
    const BOTH: Registration = Registration {
        read: true,
        write: true,
        error_only: false,
        token: 7,
    };

    #[test]
    fn error_only_conversion_and_retoken_preserve_readiness_contract() {
        let queue = Readiness::new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let errors = Registration {
            error_only: true,
            token: 10,
            ..EMPTY
        };
        queue.transition(reader.as_fd(), EMPTY, errors).unwrap();
        writer.write_all(b"x").unwrap();
        assert!(queue.wait(8, 0).unwrap().is_empty());
        let normal = Registration {
            read: true,
            token: 11,
            ..EMPTY
        };
        queue.transition(reader.as_fd(), errors, normal).unwrap();
        for _ in 0..2 {
            let ready = queue.wait(8, 0).unwrap();
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].interest, Interest::Read);
            assert_eq!(ready[0].token, 11);
        }
        queue.transition(reader.as_fd(), normal, errors).unwrap();
        assert!(queue.wait(8, 0).unwrap().is_empty());
        drop(writer);
        assert!(queue.wait(8, 1000).unwrap()[0].eof);
        let retoken = Registration {
            token: 12,
            ..errors
        };
        queue.transition(reader.as_fd(), errors, retoken).unwrap();
        for _ in 0..2 {
            let ready = queue.wait(8, 0).unwrap();
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].token, 12);
            assert!(ready[0].eof);
            assert_eq!(ready[0].interest, Interest::ErrorOnly);
        }
        queue.transition(reader.as_fd(), retoken, EMPTY).unwrap();
        assert!(queue.wait(8, 0).unwrap().is_empty());
    }

    #[test]
    fn failed_mode_conversion_removes_new_mode_before_restoring_old() {
        let errors = Registration {
            error_only: true,
            token: 10,
            ..EMPTY
        };
        let mut calls = vec![];
        let error = transition(errors, BOTH, |interest, change| {
            calls.push((interest, change));
            if calls.len() == 4 {
                Err(io::Error::from_raw_os_error(libc::ENOMEM))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.rollback.is_none());
        assert_eq!(
            calls,
            vec![
                (Interest::ErrorOnly, Change::Update(10)),
                (Interest::ErrorOnly, Change::Remove),
                (Interest::Read, Change::Register(7)),
                (Interest::Write, Change::Register(7)),
                (Interest::Read, Change::Remove),
                (Interest::ErrorOnly, Change::Register(10)),
            ]
        );
    }

    #[test]
    fn real_pair_update_removes_unwanted_filter() {
        let queue = Readiness::new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        queue.transition(reader.as_fd(), EMPTY, BOTH).unwrap();
        let read_only = Registration {
            read: true,
            write: false,
            error_only: false,
            token: 8,
        };
        queue.transition(reader.as_fd(), BOTH, read_only).unwrap();
        writer.write_all(b"x").unwrap();
        let events = queue.wait(16, 1000).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token, 8);
        assert_eq!(events[0].interest, Interest::Read);
        queue.transition(reader.as_fd(), read_only, EMPTY).unwrap();
        assert!(queue.wait(16, 0).unwrap().is_empty());
    }

    #[test]
    fn second_filter_failure_rolls_back_first() {
        let mut calls = vec![];
        let error = transition(EMPTY, BOTH, |interest, change| {
            calls.push((interest, change));
            if calls.len() == 2 {
                Err(io::Error::from_raw_os_error(libc::ENOMEM))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.operation.raw_os_error(), Some(libc::ENOMEM));
        assert!(error.rollback.is_none());
        assert_eq!(
            calls,
            vec![
                (Interest::Read, Change::Register(7)),
                (Interest::Write, Change::Register(7)),
                (Interest::Read, Change::Remove),
            ]
        );
    }

    #[test]
    fn failed_update_restores_prior_token_and_reports_rollback_failure() {
        for fail_rollback in [false, true] {
            let mut calls = vec![];
            let next = Registration { token: 9, ..BOTH };
            let error = transition(BOTH, next, |interest, change| {
                calls.push((interest, change));
                if calls.len() == 4 || (calls.len() == 5 && fail_rollback) {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(calls[4], (Interest::Read, Change::Update(7)));
            assert_eq!(error.rollback.is_some(), fail_rollback);
        }
    }

    #[test]
    fn missing_old_registration_does_not_create_filters() {
        let mut calls = 0;
        let error = transition(BOTH, BOTH, |_, _| {
            calls += 1;
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.operation.raw_os_error(), Some(libc::ENOENT));
        assert!(error.rollback.is_none());
    }
}
