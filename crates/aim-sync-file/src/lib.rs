//! Linux `sync_file` fences as host descriptors (docs/graphics-buffers.md,
//! "Fences").
//!
//! A sync_file is a guest fd that becomes readable once, when the work it
//! stands for is done, and stays so. Here it is one end of an `AF_UNIX`
//! datagram socket pair. The producer holds the other end, a [`Writer`]:
//! the GPU module until Metal signals the fence's `MTLSharedEvent`, the
//! display server until a frame is on screen, the merge waiter until every
//! input fence has signaled. It signals by sending one [`Record`] (the
//! signal time and status) and closing its end.
//!
//! The fd is a real host descriptor, so it crosses processes as every guest
//! fd does (binder, `SCM_RIGHTS`, fork) and `poll`, `epoll` and `select`
//! wait for it natively: a datagram socket reports `POLLIN`, and nothing
//! else, once the record is queued. A producer that goes away without
//! signaling leaves the socket readable with `ECONNRESET`, which reads as a
//! fence signaled with an error, as Linux reports a fence whose driver
//! failed it. The record is only ever peeked, so every holder sees it.
//!
//! The socket carries a marker (its `SO_LINGER` time, as the syscall layer
//! marks its sockets), so a sync_file arriving from another process is
//! recognized ([`is_sync_file`]).
//!
//! [`metal`] connects fences to Metal's shared events, in both directions.

pub mod metal;

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex, OnceLock};

/// The `SO_LINGER` time that marks a sync_file's socket ("SF").
pub const MARK: i32 = 0x5346;

/// Linux `EPIPE`: the status of a fence whose producer went away.
const EPIPE: i32 = 32;

/// What a producer sends when the fence signals.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Record {
    timestamp_ns: i64,
    status: i32,
    _reserved: i32,
}

/// A fence's state, as `SYNC_IOC_FILE_INFO` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Active,
    /// Signaled at `timestamp_ns` (`CLOCK_MONOTONIC`, 0 when unknown) with
    /// `status` 1, or a negative Linux errno.
    Signaled {
        timestamp_ns: i64,
        status: i32,
    },
}

/// What the syscall layer provides: the descriptors kept here are the
/// host's, out of the guest's view.
pub struct Hooks {
    /// Move a descriptor the host keeps for itself out of the guest's way;
    /// takes it and returns its new number.
    pub hide: fn(RawFd) -> RawFd,
    /// Forget a hidden descriptor that is about to be closed.
    pub unhide: fn(RawFd),
    /// Publish this owned native fence; failure is a Linux errno.
    pub adopt: fn(RawFd) -> Result<(), i32>,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Install the syscall layer's hooks (once per process).
pub fn set_hooks(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

/// The descriptors this process keeps for pending fences: writers and the
/// merge waiter's inputs. A fork child inherits them and closes them
/// ([`close_inherited`]); they are the parent's to signal.
static PRIVATE: Mutex<Vec<RawFd>> = Mutex::new(Vec::new());

/// A hidden descriptor of ours, listed in [`PRIVATE`] while open.
struct Private(RawFd);

impl Private {
    fn new(fd: OwnedFd) -> Private {
        let fd = match HOOKS.get() {
            Some(h) => (h.hide)(fd.into_raw_fd()),
            None => fd.into_raw_fd(),
        };
        PRIVATE.lock().unwrap().push(fd);
        Private(fd)
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        PRIVATE.lock().unwrap().retain(|&f| f != self.0);
        if let Some(h) = HOOKS.get() {
            (h.unhide)(self.0);
        }
        // SAFETY: our descriptor, closed once.
        unsafe { libc::close(self.0) };
    }
}

/// The producer's end of a fence. Dropping it without [`Writer::signal`]
/// fails the fence.
pub struct Writer(Private);

impl Writer {
    /// Signal the fence now with `status` (1, or a negative Linux errno).
    pub fn signal(self, status: i32) {
        self.signal_at(monotonic_ns(), status);
    }

    /// Signal the fence with the given time and status.
    pub fn signal_at(self, timestamp_ns: i64, status: i32) {
        let r = Record {
            timestamp_ns,
            status,
            _reserved: 0,
        };
        // SAFETY: a send of a local record on our socket; an empty datagram
        // queue never blocks.
        unsafe {
            libc::send(
                self.0.0,
                (&r as *const Record).cast(),
                size_of::<Record>(),
                0,
            )
        };
    }
}

impl From<OwnedFd> for Writer {
    /// The writer of a fence made elsewhere, received over a socket.
    fn from(fd: OwnedFd) -> Writer {
        Writer(Private::new(fd))
    }
}

impl AsRawFd for Writer {
    fn as_raw_fd(&self) -> RawFd {
        self.0.0
    }
}

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// A new, unsignaled fence: the sync_file (close-on-exec) and its writer.
pub fn pair() -> io::Result<(OwnedFd, Writer)> {
    let mut sv = [0; 2];
    // SAFETY: socketpair into a local array; both fds are ours.
    let (file, writer) = unsafe {
        cvt(libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_DGRAM,
            0,
            sv.as_mut_ptr(),
        ))?;
        (OwnedFd::from_raw_fd(sv[0]), OwnedFd::from_raw_fd(sv[1]))
    };
    let mark = libc::linger {
        l_onoff: 0,
        l_linger: MARK,
    };
    // SAFETY: fcntl and setsockopt on our new sockets with local values.
    unsafe {
        for fd in [&file, &writer] {
            cvt(libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
        }
        cvt(libc::setsockopt(
            file.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&mark as *const libc::linger).cast(),
            size_of::<libc::linger>() as libc::socklen_t,
        ))?;
    }
    Ok((file, Writer(Private::new(writer))))
}

/// A fence that has already signaled.
pub fn signaled(timestamp_ns: i64, status: i32) -> io::Result<OwnedFd> {
    let (file, writer) = pair()?;
    writer.signal_at(timestamp_ns, status);
    Ok(file)
}

/// Hand a sync_file to the guest: its fd number from now on.
pub fn give_to_guest(file: OwnedFd) -> Result<RawFd, i32> {
    let fd = file.as_raw_fd();
    if let Some(h) = HOOKS.get() { (h.adopt)(fd)?; }
    Ok(file.into_raw_fd())
}

/// Whether `fd` is a sync_file's socket.
pub fn is_sync_file(fd: BorrowedFd) -> bool {
    let mut l = libc::linger {
        l_onoff: 0,
        l_linger: 0,
    };
    let mut len = size_of::<libc::linger>() as libc::socklen_t;
    let mut ty = 0;
    let mut ty_len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: getsockopt into locals.
    unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&mut l as *mut libc::linger).cast(),
            &mut len,
        ) == 0
            && l.l_linger == MARK
            && libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut ty as *mut libc::c_int).cast(),
                &mut ty_len,
            ) == 0
            && ty == libc::SOCK_DGRAM
    }
}

/// The fence's state, without waiting.
pub fn state(fd: BorrowedFd) -> State {
    let mut r = Record::default();
    // SAFETY: a peek into a local record.
    let n = unsafe {
        libc::recv(
            fd.as_raw_fd(),
            (&mut r as *mut Record).cast(),
            size_of::<Record>(),
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if n == size_of::<Record>() as isize {
        return State::Signaled {
            timestamp_ns: r.timestamp_ns,
            status: r.status,
        };
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::EAGAIN) if n < 0 => State::Active,
        _ => State::Signaled {
            timestamp_ns: 0,
            status: -EPIPE,
        },
    }
}

/// Wait up to `timeout_ms` (-1 for ever) for the fence; whether it
/// signaled.
pub fn wait(fd: BorrowedFd, timeout_ms: i32) -> bool {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one pollfd on our stack.
        let n = unsafe { libc::poll(&mut p, 1, timeout_ms) };
        if n >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return n > 0;
        }
    }
}

/// The state of a fence made of signaled fences: the last signal time, and
/// the first error if any.
fn combine(states: &[State]) -> State {
    let mut out = (0, 1);
    for s in states {
        if let State::Signaled {
            timestamp_ns,
            status,
        } = *s
        {
            out.0 = out.0.max(timestamp_ns);
            if status < 0 && out.1 == 1 {
                out.1 = status;
            }
        }
    }
    State::Signaled {
        timestamp_ns: out.0,
        status: out.1,
    }
}

/// `SYNC_IOC_MERGE`: a new fence that signals when both have.
pub fn merge(a: BorrowedFd, b: BorrowedFd) -> io::Result<OwnedFd> {
    let states = [state(a), state(b)];
    if !states.contains(&State::Active) {
        let State::Signaled {
            timestamp_ns,
            status,
        } = combine(&states)
        else {
            unreachable!()
        };
        return signaled(timestamp_ns, status);
    }
    let (file, writer) = pair()?;
    let mut inputs = Vec::new();
    for fd in [a, b] {
        inputs.push(Private::new(fd.try_clone_to_owned()?));
    }
    waiter()?.add(
        inputs,
        &states,
        Box::new(move |state| {
            if let State::Signaled {
                timestamp_ns,
                status,
            } = state
            {
                writer.signal_at(timestamp_ns, status);
            }
        }),
    )?;
    Ok(file)
}

/// Call `then` with the fence's state once it has signaled: now, or on
/// the waiter's thread.
pub fn on_signal(fd: BorrowedFd, then: impl FnOnce(State) + Send + 'static) -> io::Result<()> {
    let s = state(fd);
    if s != State::Active {
        then(s);
        return Ok(());
    }
    let input = Private::new(fd.try_clone_to_owned()?);
    waiter()?.add(vec![input], &[s], Box::new(then))
}

/// What waits for fences: a merge, or an [`on_signal`] callback.
struct Merge {
    inputs: Vec<Private>,
    /// Inputs not signaled yet.
    left: usize,
    then: Box<dyn FnOnce(State) + Send>,
}

/// One thread per process that waits for the inputs of merged fences and
/// of [`on_signal`] callbacks.
struct Waiter {
    kq: RawFd,
    merges: Mutex<(u64, HashMap<u64, Merge>)>,
}

impl Waiter {
    fn add(
        &self,
        inputs: Vec<Private>,
        states: &[State],
        then: Box<dyn FnOnce(State) + Send>,
    ) -> io::Result<()> {
        let mut merges = self.merges.lock().unwrap();
        merges.0 += 1;
        let id = merges.0;
        let mut changes = Vec::new();
        for (p, s) in inputs.iter().zip(states) {
            if *s == State::Active {
                // SAFETY: an all-zero kevent is valid.
                let mut k: libc::kevent = unsafe { std::mem::zeroed() };
                k.ident = p.0 as usize;
                k.filter = libc::EVFILT_READ;
                k.flags = libc::EV_ADD | libc::EV_ONESHOT;
                k.udata = id as *mut libc::c_void;
                changes.push(k);
            }
        }
        merges.1.insert(
            id,
            Merge {
                inputs,
                left: changes.len(),
                then,
            },
        );
        // SAFETY: registering local changes on our kqueue.
        let r = unsafe {
            libc::kevent(
                self.kq,
                changes.as_ptr(),
                changes.len() as i32,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if r < 0 {
            let e = io::Error::last_os_error();
            merges.1.remove(&id);
            return Err(e);
        }
        Ok(())
    }

    fn run(&self) {
        // SAFETY: an all-zero kevent is valid.
        let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: waiting on our kqueue into a local array.
            let n = unsafe {
                libc::kevent(
                    self.kq,
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as i32,
                    std::ptr::null(),
                )
            };
            for e in events.iter().take(n.max(0) as usize) {
                let id = e.udata as u64;
                let mut merges = self.merges.lock().unwrap();
                let Some(m) = merges.1.get_mut(&id) else {
                    continue;
                };
                m.left -= 1;
                if m.left == 0 {
                    let m = merges.1.remove(&id).unwrap();
                    drop(merges);
                    let states: Vec<State> = m
                        .inputs
                        // SAFETY: the inputs stay open until `m` drops.
                        .iter()
                        .map(|p| state(unsafe { BorrowedFd::borrow_raw(p.0) }))
                        .collect();
                    (m.then)(combine(&states));
                }
            }
        }
    }
}

/// This process's waiter; a fork child starts its own.
fn waiter() -> io::Result<Arc<Waiter>> {
    static WAITER: Mutex<Option<(u32, Arc<Waiter>)>> = Mutex::new(None);
    let mut w = WAITER.lock().unwrap();
    let pid = std::process::id();
    if let Some((p, waiter)) = w.as_ref()
        && *p == pid
    {
        return Ok(waiter.clone());
    }
    // SAFETY: a new kqueue of our own.
    let kq = cvt(unsafe { libc::kqueue() })?;
    // A kqueue is not inherited by a fork child, so it is hidden but not
    // listed with the descriptors a child closes.
    let kq = match HOOKS.get() {
        Some(h) => (h.hide)(kq),
        None => kq,
    };
    let waiter = Arc::new(Waiter {
        kq,
        merges: Mutex::new((0, HashMap::new())),
    });
    let run = waiter.clone();
    std::thread::Builder::new()
        .name("sync-file-wait".into())
        .spawn(move || run.run())?;
    *w = Some((pid, waiter.clone()));
    Ok(waiter)
}

/// The descriptors a fork child inherits from this process's pending
/// fences.
pub fn inherited() -> Vec<RawFd> {
    PRIVATE.lock().unwrap().clone()
}

/// In a fork child: close the parent's descriptors ([`inherited`]). Its
/// fences are the parent's to signal.
pub fn close_inherited(fds: &[RawFd]) {
    for &fd in fds {
        if let Some(h) = HOOKS.get() {
            (h.unhide)(fd);
        }
        // SAFETY: the parent's descriptor, inherited and not used here.
        unsafe { libc::close(fd) };
    }
}

/// The guest's `CLOCK_MONOTONIC`, in nanoseconds.
pub use aim_hostcall::clock::monotonic_ns;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;
    use std::time::Duration;

    fn poll_bits(fd: BorrowedFd) -> i16 {
        let mut p = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN | libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one pollfd on our stack.
        unsafe { libc::poll(&mut p, 1, 0) };
        p.revents & !libc::POLLOUT
    }

    #[test]
    fn a_fence_is_readable_once_signaled_and_stays_so() {
        let (file, writer) = pair().unwrap();
        assert!(is_sync_file(file.as_fd()));
        assert_eq!(state(file.as_fd()), State::Active);
        assert_eq!(poll_bits(file.as_fd()), 0);
        assert!(!wait(file.as_fd(), 0));
        writer.signal_at(42, 1);
        for _ in 0..2 {
            assert_eq!(
                state(file.as_fd()),
                State::Signaled {
                    timestamp_ns: 42,
                    status: 1
                }
            );
            assert_eq!(poll_bits(file.as_fd()), libc::POLLIN, "no POLLHUP");
            assert!(wait(file.as_fd(), 0));
        }
        let dup = file.try_clone().unwrap();
        assert!(matches!(state(dup.as_fd()), State::Signaled { .. }));
    }

    #[test]
    fn a_dropped_writer_fails_the_fence() {
        let (file, writer) = pair().unwrap();
        drop(writer);
        assert_eq!(
            state(file.as_fd()),
            State::Signaled {
                timestamp_ns: 0,
                status: -EPIPE
            }
        );
        assert!(wait(file.as_fd(), 0));
        assert!(inherited().iter().all(|&fd| fd != file.as_raw_fd()));
    }

    #[test]
    fn other_sockets_are_not_sync_files() {
        let (a, _b) = std::os::unix::net::UnixDatagram::pair().unwrap();
        assert!(!is_sync_file(a.as_fd()));
        let (s, _t) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(!is_sync_file(s.as_fd()));
    }

    #[test]
    fn a_merge_of_signaled_fences_is_signaled_at_the_later_time() {
        let a = signaled(10, 1).unwrap();
        let b = signaled(20, 1).unwrap();
        let m = merge(a.as_fd(), b.as_fd()).unwrap();
        assert_eq!(
            state(m.as_fd()),
            State::Signaled {
                timestamp_ns: 20,
                status: 1
            }
        );
    }

    #[test]
    fn a_merge_waits_for_both_and_keeps_an_error() {
        let (a, wa) = pair().unwrap();
        let (b, wb) = pair().unwrap();
        let m = merge(a.as_fd(), b.as_fd()).unwrap();
        assert_eq!(state(m.as_fd()), State::Active);
        wa.signal_at(30, 1);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(state(m.as_fd()), State::Active);
        wb.signal_at(25, -5);
        assert!(wait(m.as_fd(), 5000));
        assert_eq!(
            state(m.as_fd()),
            State::Signaled {
                timestamp_ns: 30,
                status: -5
            }
        );
        // A merge with an already merged fence, and with itself.
        let (c, wc) = pair().unwrap();
        let mm = merge(m.as_fd(), c.as_fd()).unwrap();
        let same = merge(c.as_fd(), c.as_fd()).unwrap();
        wc.signal_at(40, 1);
        assert!(wait(mm.as_fd(), 5000) && wait(same.as_fd(), 5000));
        assert_eq!(
            state(same.as_fd()),
            State::Signaled {
                timestamp_ns: 40,
                status: 1
            }
        );
    }

    #[test]
    fn on_signal_runs_once_the_fence_signals() {
        let (file, writer) = pair().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let now = tx.clone();
        on_signal(file.as_fd(), move |s| tx.send(s).unwrap()).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
        writer.signal_at(50, 1);
        let signaled = State::Signaled {
            timestamp_ns: 50,
            status: 1,
        };
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(signaled));
        // An already signaled fence runs it at once.
        on_signal(file.as_fd(), move |s| now.send(s).unwrap()).unwrap();
        assert_eq!(rx.try_recv(), Ok(signaled));
    }
}
