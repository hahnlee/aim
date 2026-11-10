//! The owner an O_ASYNC descriptor signals (Linux `struct fown_struct`),
//! for descriptors whose readiness this layer implements, and for host
//! ones with what Darwin cannot hold (`fasync`).

use std::sync::Mutex;

use super::signal::IoOwner;
use crate::errno::{self, EINVAL, ESRCH, Errno};

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
    /// `send_sigio` for `reason` (`signal::POLL_*`) through `fd`: nothing
    /// without an owner.
    pub fn notify(&self, fd: i32, reason: i32) {
        let owner = match self.kind {
            _ if self.pid == 0 => return,
            F_OWNER_TID => IoOwner::Thread(self.pid),
            F_OWNER_PID => IoOwner::Process(self.pid),
            _ => IoOwner::Group(self.pid),
        };
        super::signal::send_io(owner, self.signum, fd, reason, (self.uid, self.euid));
    }

    /// The owner as Darwin's F_SETOWN takes it (a pid, a negated group, or
    /// 0 for none) when Darwin can signal it: SIGIO to a process or group,
    /// and to a process only on a socket (Darwin's other files take the
    /// process's group in its place).
    pub fn host(&self, socket: bool) -> Option<i32> {
        match self.kind {
            _ if self.signum != 0 => None,
            _ if self.pid == 0 => Some(0),
            F_OWNER_PID if socket => Some(self.pid),
            F_OWNER_PGRP => Some(-self.pid),
            _ => None,
        }
    }
}

/// O_ASYNC as the record keeps it, for a file whose own status flags
/// cannot (a host tty's, an inotify's).
const ASYNC: u32 = 1;
/// Darwin holds the owner now, and signals for every holder of the file.
const DARWIN: u32 = 2;

/// What every holder of an open file sees of its owner.
#[derive(Clone, Copy, Default)]
struct Record {
    owner: Owner,
    flags: u32,
}

/// The record of an owner shared by the processes that hold its file (a
/// fork child, an exec'd image): a private unlinked file each of them
/// reads and writes, as Linux's one `struct file` keeps `f_owner`. The
/// O_ASYNC election ([`Fown::won`]) takes its lock on the same file.
struct Shared(i32);

impl Shared {
    fn new() -> Result<Shared, Errno> {
        let mut tmpl = std::env::temp_dir()
            .join("linux-abi-fown.XXXXXX")
            .into_os_string()
            .into_encoded_bytes();
        tmpl.push(0);
        // SAFETY: mkstemp fills in the template; the name goes at once.
        let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
        if fd < 0 {
            return Err(errno::last());
        }
        // SAFETY: our new file's name and descriptor.
        unsafe {
            libc::unlink(tmpl.as_ptr().cast());
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        Ok(Shared(super::fdtab::hide(fd)))
    }

    fn read(&self) -> Result<Record, Errno> {
        let mut v = [0i32; 6];
        // SAFETY: reads into a local array.
        let n = unsafe { libc::pread(self.0, v.as_mut_ptr().cast(), 24, 0) };
        if n != 24 {
            return Err(if n < 0 { errno::last() } else { errno::EIO });
        }
        let [kind, pid, signum, uid, euid, flags] = v;
        Ok(Record {
            owner: Owner {
                kind,
                pid,
                signum,
                uid: uid as u32,
                euid: euid as u32,
            },
            flags: flags as u32,
        })
    }

    fn write(&self, r: &Record) -> Result<(), Errno> {
        let o = r.owner;
        let v = [
            o.kind,
            o.pid,
            o.signum,
            o.uid as i32,
            o.euid as i32,
            r.flags as i32,
        ];
        // SAFETY: writes a local array.
        if unsafe { libc::pwrite(self.0, v.as_ptr().cast(), 24, 0) } != 24 {
            return Err(errno::last());
        }
        Ok(())
    }

    /// Whether this process signals the event at hand: the one that holds,
    /// or now takes, an exclusive POSIX record lock on the file. Darwin
    /// gives it to one process at a time and drops it when that process
    /// closes the file or exits, so another holder takes over.
    fn won(&self) -> bool {
        // SAFETY: an all-zero flock is valid.
        let mut whole: libc::flock = unsafe { std::mem::zeroed() };
        whole.l_type = libc::F_WRLCK as i16;
        whole.l_whence = libc::SEEK_SET as i16;
        // SAFETY: a lock request on our private file.
        if unsafe { libc::fcntl(self.0, libc::F_SETLK, &whole) } == 0 {
            return true;
        }
        let error = errno::last();
        if error == errno::EAGAIN || error == errno::EACCES {
            return false;
        }
        // Signalling twice is safer than not at all.
        crate::diag!("O_ASYNC election: errno {error}");
        true
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        super::fdtab::unhide(self.0);
        // SAFETY: our private file.
        unsafe { libc::close(self.0) };
    }
}

#[derive(Default)]
struct State {
    /// The record while no other process can hold the file.
    local: Record,
    shared: Option<Shared>,
}

#[derive(Default)]
pub struct Fown {
    state: Mutex<State>,
}

impl Fown {
    /// Darwin's owner `owner` of a host file (a pid, or a negated group),
    /// set by the caller as far as `sigio_perm` is concerned.
    pub fn from_host(owner: i32) -> Fown {
        let id = super::cred::current();
        let owner = Owner {
            kind: if owner < 0 { F_OWNER_PGRP } else { F_OWNER_PID },
            pid: owner.abs(),
            signum: 0,
            uid: id.uid[0],
            euid: id.uid[1],
        };
        Fown {
            state: Mutex::new(State {
                local: Record { owner, flags: 0 },
                shared: None,
            }),
        }
    }

    fn record(&self) -> Record {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &state.shared {
            Some(shared) => shared.read().unwrap_or_else(|error| {
                crate::diag!("O_ASYNC owner record: errno {error}");
                state.local
            }),
            None => state.local,
        }
    }

    /// Change the record, made shared first: whatever process holds the
    /// file may copy it from now on.
    fn update(&self, change: impl FnOnce(&mut Record)) -> Result<(), Errno> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shared.is_none() {
            let shared = Shared::new()?;
            shared.write(&state.local)?;
            state.shared = Some(shared);
        }
        let shared = state.shared.as_ref().unwrap();
        let mut record = shared.read()?;
        change(&mut record);
        shared.write(&record)?;
        state.local = record;
        Ok(())
    }

    /// The owner now, to signal once the file's lock is released.
    pub fn owner(&self) -> Owner {
        self.record().owner
    }

    /// O_ASYNC of a file whose status flags cannot keep it.
    pub fn is_async(&self) -> bool {
        self.record().flags & ASYNC != 0
    }

    pub fn set_async(&self, on: bool) -> Result<(), Errno> {
        self.update(|r| {
            r.flags = if on {
                r.flags | ASYNC
            } else {
                r.flags & !ASYNC
            }
        })
    }

    /// Whether Darwin holds the owner, so no holder signals here.
    pub fn darwin(&self) -> bool {
        self.record().flags & DARWIN != 0
    }

    pub fn set_darwin(&self, on: bool) -> Result<(), Errno> {
        self.update(|r| {
            r.flags = if on {
                r.flags | DARWIN
            } else {
                r.flags & !DARWIN
            }
        })
    }

    /// Whether this process signals the event at hand; one of the
    /// processes holding the file does.
    pub fn won(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shared.as_ref().is_none_or(Shared::won)
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
        let updated = self.update(|r| {
            r.owner.kind = kind;
            r.owner.pid = target;
            (r.owner.uid, r.owner.euid) = (id.uid[0], id.uid[1]);
        });
        updated.map_or_else(|error| -(error as i64), |()| 0)
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
                let updated = self.update(|r| r.owner.signum = arg as i32);
                updated.map_or_else(|error| -(error as i64), |()| 0)
            }
            F_GETSIG => self.owner().signum as i64,
            _ => return None,
        })
    }

    /// Fork: the record, and the shared file the child holds too (Linux
    /// children share the file and its owner).
    pub(super) fn save(&self, w: &mut super::fork_state::Writer) {
        let r = self.record();
        w.i32(r.owner.kind);
        w.i32(r.owner.pid);
        w.i32(r.owner.signum);
        w.u32(r.owner.uid);
        w.u32(r.owner.euid);
        w.u32(r.flags);
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        w.opt(state.shared.as_ref(), |w, shared| {
            w.retain_private(shared.0);
            w.i32(shared.0);
        });
    }

    pub(super) fn load(r: &mut super::fork_state::Reader) -> Fown {
        let owner = Owner {
            kind: r.i32(),
            pid: r.i32(),
            signum: r.i32(),
            uid: r.u32(),
            euid: r.u32(),
        };
        let local = Record {
            owner,
            flags: r.u32(),
        };
        Fown {
            state: Mutex::new(State {
                local,
                shared: r.opt(|r| Shared(r.i32())),
            }),
        }
    }
}
