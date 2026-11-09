//! The owner an O_ASYNC descriptor signals (Linux `struct fown_struct`),
//! for descriptors whose readiness this layer implements; Darwin keeps it
//! for host ones.

use std::sync::Mutex;

use super::signal::IoOwner;
use crate::errno::{EINVAL, ESRCH};

pub const F_SETOWN: u64 = 8;
pub const F_GETOWN: u64 = 9;
pub const F_SETSIG: u64 = 10;
pub const F_GETSIG: u64 = 11;
pub const F_SETOWN_EX: u64 = 15;
pub const F_GETOWN_EX: u64 = 16;

const F_OWNER_TID: i32 = 0;
const F_OWNER_PID: i32 = 1;
const F_OWNER_PGRP: i32 = 2;

/// `struct f_owner_ex` { int type; pid_t pid; }.
#[repr(C)]
#[derive(Clone, Copy)]
struct OwnerEx {
    kind: i32,
    pid: i32,
}

/// A file's owner and signal, as a signal to it is sent.
#[derive(Clone, Copy, Default)]
pub struct Owner {
    /// F_OWNER_TID, F_OWNER_PID or F_OWNER_PGRP.
    kind: i32,
    /// The owner as `pidns::syscall_pid` numbers it; 0 for none.
    pid: i32,
    /// F_SETSIG's signal; 0 for SIGIO without siginfo.
    signum: i32,
    /// The real and effective uids of the caller that set the owner,
    /// which `sigio_perm` checks the target against.
    uid: u32,
    euid: u32,
}

impl Owner {
    /// `send_sigio` for POLL_IN through `fd`: nothing without an owner.
    pub fn readable(&self, fd: i32) {
        let owner = match self.kind {
            _ if self.pid == 0 => return,
            F_OWNER_TID => IoOwner::Thread(self.pid),
            F_OWNER_PID => IoOwner::Process(self.pid),
            _ => IoOwner::Group(self.pid),
        };
        super::signal::send_io(owner, self.signum, fd, (self.uid, self.euid));
    }
}

#[derive(Default)]
pub struct Fown {
    state: Mutex<Owner>,
}

impl Fown {
    /// The owner now, to signal once the file's lock is released.
    pub fn owner(&self) -> Owner {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, kind: i32, pid: i32) -> i64 {
        let target = match super::pidns::syscall_pid(pid) {
            Ok(target) => target,
            Err(error) => return error,
        };
        let exists = match kind {
            _ if target == 0 => true,
            F_OWNER_TID => {
                super::thread::find(target).is_some() || super::thread::owner(target) != 0
            }
            F_OWNER_PID => super::pidns::contains(target),
            _ => {
                super::pidns::contains(target)
                    || super::pidns::group(target).is_some_and(|members| !members.is_empty())
            }
        };
        if !exists {
            return -(ESRCH as i64);
        }
        let id = super::cred::current();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.kind = kind;
        state.pid = target;
        (state.uid, state.euid) = (id.uid[0], id.uid[1]);
        0
    }

    fn guest(&self) -> Result<(i32, i32), i64> {
        let state = self.owner();
        let pid = if state.pid == 0 {
            0
        } else {
            super::pidns::guest_pid(state.pid)?
        };
        Ok((state.kind, pid))
    }

    /// The owner commands of fcntl(2) (`f_setown`, `f_getown_ex`, ...);
    /// None for any other command.
    pub fn fcntl(&self, cmd: u64, arg: u64) -> Option<i64> {
        Some(match cmd {
            F_SETOWN => match arg as i32 {
                i32::MIN => -(EINVAL as i64),
                pid if pid < 0 => self.set(F_OWNER_PGRP, -pid),
                pid => self.set(F_OWNER_PID, pid),
            },
            F_GETOWN => match self.guest() {
                Ok((F_OWNER_PGRP, pid)) => -(pid as i64),
                Ok((_, pid)) => pid as i64,
                Err(error) => error,
            },
            F_SETOWN_EX => {
                // SAFETY: guest struct f_owner_ex.
                let owner = match unsafe { super::user_memory::read_value::<OwnerEx>(arg) } {
                    Ok(owner) => owner,
                    Err(error) => return Some(-(error as i64)),
                };
                if !matches!(owner.kind, F_OWNER_TID | F_OWNER_PID | F_OWNER_PGRP) {
                    return Some(-(EINVAL as i64));
                }
                self.set(owner.kind, owner.pid)
            }
            F_GETOWN_EX => match self.guest() {
                // SAFETY: guest struct f_owner_ex.
                Ok((kind, pid)) => {
                    match unsafe { super::user_memory::write_value(arg, &OwnerEx { kind, pid }) } {
                        Ok(()) => 0,
                        Err(error) => -(error as i64),
                    }
                }
                Err(error) => error,
            },
            F_SETSIG => {
                if arg > 64 {
                    return Some(-(EINVAL as i64));
                }
                self.state.lock().unwrap_or_else(|e| e.into_inner()).signum = arg as i32;
                0
            }
            F_GETSIG => self.owner().signum as i64,
            _ => return None,
        })
    }

    /// Fork: the owner and signal (Linux children share the file; here
    /// each process has a copy).
    pub(super) fn save(&self, w: &mut super::fork_state::Writer) {
        let state = self.owner();
        w.i32(state.kind);
        w.i32(state.pid);
        w.i32(state.signum);
        w.u32(state.uid);
        w.u32(state.euid);
    }

    pub(super) fn load(r: &mut super::fork_state::Reader) -> Fown {
        Fown {
            state: Mutex::new(Owner {
                kind: r.i32(),
                pid: r.i32(),
                signum: r.i32(),
                uid: r.u32(),
                euid: r.u32(),
            }),
        }
    }
}
