//! signalfd: a descriptor that reads the reader's pending signals in its
//! mask as Linux's `signalfd_siginfo` records (signalfd(2), `fs/signalfd.c`).
//!
//! Pending state belongs to the signal owner; a description holds only its
//! mask. Its guest end is a Darwin datagram socket, so it polls like an
//! eventfd: the description keeps one token record in it exactly while the
//! process or one of its threads has a signal in the mask pending
//! ([`refresh`], called by the signal owner whenever a pending set
//! changes). A Linux poll looks at the polling thread's own queue; here a
//! signal pending on another thread counts too. Each process has its own
//! socket for a description, so a forked child or a relaunched exec image
//! sees only its own signals: [`install`] puts a fresh one under every
//! guest fd that shares the description, keeping its flags.

use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering::SeqCst},
};

use super::{
    fdtab::{self, Kind},
    sigframe::Siginfo,
    signal,
};
use crate::errno::{self, EINVAL};

const NONBLOCK: u64 = 0o4000;
const CLOEXEC: u64 = 0o2000000;
const RECORD: usize = 128;

pub struct SignalFd {
    mask: AtomicU64,
    /// The hidden peer that sends the token, and a hidden twin of the guest
    /// end that drains it and is installed again after fork or exec.
    peer: i32,
    reader: i32,
    token: Mutex<bool>,
}

static DESCRIPTIONS: Mutex<Vec<Weak<SignalFd>>> = Mutex::new(Vec::new());

impl Drop for SignalFd {
    fn drop(&mut self) {
        for fd in [self.peer, self.reader] {
            if let Err(error) = fdtab::close_fork_private(fd) {
                crate::diag!("signalfd private end close: errno {error}");
            }
        }
    }
}

impl SignalFd {
    /// A description and its guest end, with the token for what is pending.
    fn open(mask: u64, flags: u64) -> Result<(i32, Arc<SignalFd>), i64> {
        let (fd, peer) = super::event::pair(flags)?;
        // SAFETY: duplicating the socket just created.
        let reader = unsafe { libc::dup(fd) };
        if reader < 0 {
            let error = errno::last();
            // SAFETY: closing the unpublished socket and its hidden peer.
            unsafe { libc::close(fd) };
            if let Err(error) = fdtab::close_fork_private(peer) {
                crate::diag!("signalfd private end close: errno {error}");
            }
            return Err(-(error as i64));
        }
        let description = Arc::new(SignalFd {
            mask: AtomicU64::new(mask & !signal::UNBLOCKABLE),
            peer,
            reader: fdtab::hide(reader),
            token: Mutex::new(false),
        });
        let mut all = DESCRIPTIONS.lock().unwrap();
        all.retain(|weak| weak.strong_count() > 0);
        all.push(Arc::downgrade(&description));
        drop(all);
        description.sync();
        Ok((fd, description))
    }

    /// Keep the token iff a signal in the mask is pending. The token's lock
    /// orders the snapshots, so the last change is the one that stays.
    fn sync(&self) {
        let mut token = self.token.lock().unwrap();
        let ready = self.mask.load(SeqCst) & signal::all_pending() != 0;
        if ready && !*token {
            *token = super::event::send_record(self.peer, 1);
        } else if !ready && *token {
            super::event::recv_record(self.reader);
            *token = false;
        }
    }
}

/// The signal owner: a pending set changed.
pub(super) fn refresh() {
    let all: Vec<_> = {
        let all = DESCRIPTIONS.lock().unwrap();
        all.iter().filter_map(Weak::upgrade).collect()
    };
    for description in all {
        description.sync();
    }
}

pub fn signalfd4(a: [u64; 6]) -> i64 {
    if a[2] != 8 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest sigset.
    let mask = user_read!(a[1], u64) & !signal::UNBLOCKABLE;
    // The syscall's flags argument is a Linux int.
    let flags = a[3] as u32 as u64;
    if flags & !(NONBLOCK | CLOEXEC) != 0 {
        return -(EINVAL as i64);
    }
    let fd = a[0] as i32;
    if fd != -1 {
        let pin = match fdtab::pin_guest(fd) {
            Ok(pin) => pin,
            Err(error) => return -(error as i64),
        };
        let Some(Kind::Signal(description)) = pin.kind() else {
            return -(EINVAL as i64);
        };
        description.mask.store(mask, SeqCst);
        description.sync();
        signal::wake_waiters();
        return fd as i64;
    }
    let (fd, description) = match SignalFd::open(mask, flags) {
        Ok(open) => open,
        Err(error) => return error,
    };
    fdtab::insert(fd, Kind::Signal(description));
    if let Err(error) = fdtab::publish_guest(fd) {
        fdtab::on_close(fd);
        // SAFETY: closing the unpublished socket.
        unsafe { libc::close(fd) };
        return -(error as i64);
    }
    fd as i64
}

/// Read whole records: the first may block, as `signalfd_dequeue`; the
/// rest take only what is already pending.
pub fn read(fd: i32, description: &SignalFd, iov: &[libc::iovec]) -> i64 {
    let len: usize = iov.iter().map(|v| v.iov_len).sum();
    if len < RECORD {
        return -(EINVAL as i64);
    }
    let mut bytes = Vec::new();
    while bytes.len() + RECORD <= len {
        let nonblock = !bytes.is_empty() || fdtab::nonblocking(fd);
        match signal::read_signal(&description.mask, nonblock) {
            Ok(info) => bytes.extend_from_slice(&record(&info)),
            Err(_) if !bytes.is_empty() => break,
            Err(error) => return error,
        }
    }
    match super::user_memory::scatter(iov, &bytes) {
        Ok(done) => done as i64,
        Err(error) => -(error as i64),
    }
}

/// Fork and host exec: the mask, read into a new description of this
/// process.
pub(super) fn save(description: &SignalFd) -> u64 {
    description.mask.load(SeqCst)
}

pub(super) fn load(mask: u64) -> Result<Arc<SignalFd>, crate::errno::Errno> {
    let (fd, description) =
        SignalFd::open(mask, 0).map_err(|error| (-error) as crate::errno::Errno)?;
    // SAFETY: the unpublished socket; `reader` keeps it open.
    unsafe { libc::close(fd) };
    Ok(description)
}

/// Put this process's socket of `description` under guest `fd`, keeping
/// the fd's FD_CLOEXEC and the file's O_NONBLOCK.
pub(super) fn install(fd: i32, description: &SignalFd) -> Result<(), crate::errno::Errno> {
    // SAFETY: fcntl and dup2 on the inherited guest fd and our hidden end.
    unsafe {
        let (fd_flags, fl) = (
            libc::fcntl(fd, libc::F_GETFD),
            libc::fcntl(fd, libc::F_GETFL),
        );
        if fd_flags < 0
            || fl < 0
            || libc::dup2(description.reader, fd) < 0
            || libc::fcntl(fd, libc::F_SETFD, fd_flags) < 0
            || libc::fcntl(fd, libc::F_SETFL, fl & libc::O_NONBLOCK) < 0
        {
            return Err(errno::last());
        }
    }
    Ok(())
}

/// The reader of a description in a record, which identifies it among
/// the records of one exec.
pub(super) fn exec_text(description: &SignalFd) -> String {
    format!("{}:{}", description.mask.load(SeqCst), description.reader)
}

// signalfd_copyinfo over siginfo_layout: each layout copies its fields;
// every other byte, padding included, is zero.
fn record(info: &Siginfo) -> [u8; RECORD] {
    let mut out = [0; RECORD];
    let mut put =
        |offset: usize, bytes: &[u8]| out[offset..offset + bytes.len()].copy_from_slice(bytes);
    let f = info.fields;
    put(0, &info.signo.to_le_bytes());
    put(4, &info.errno.to_le_bytes());
    put(8, &info.code.to_le_bytes());
    // _kill / _rt / _sigchld: pid, uid.
    let sender = |put: &mut dyn FnMut(usize, &[u8])| {
        put(12, &(f[0] as u32).to_le_bytes());
        put(16, &((f[0] >> 32) as u32).to_le_bytes());
    };
    // sigval: ssi_int and ssi_ptr.
    let value = |put: &mut dyn FnMut(usize, &[u8])| {
        put(44, &(f[1] as u32).to_le_bytes());
        put(48, &f[1].to_le_bytes());
    };
    let (sig, code) = (info.signo, info.code);
    // NSIGILL, NSIGFPE, NSIGSEGV, NSIGBUS, NSIGTRAP, NSIGCHLD, NSIGSYS.
    let limit = match sig {
        4 => 11,
        8 => 15,
        11 => 10,
        7 => 5,
        5 => 6,
        17 => 6,
        31 => 2,
        _ => 0,
    };
    match code {
        1..0x80 if code <= limit && sig == 17 => {
            sender(&mut put);
            put(40, &(f[1] as u32).to_le_bytes());
            put(56, &f[2].to_le_bytes());
            put(64, &f[3].to_le_bytes());
        }
        1..0x80 if code <= limit && sig == 31 => {
            put(88, &f[0].to_le_bytes());
            put(84, &(f[1] as u32).to_le_bytes());
            put(96, &((f[1] >> 32) as u32).to_le_bytes());
        }
        1..0x80 if code <= limit => {
            put(72, &f[0].to_le_bytes());
            // BUS_MCEERR_AR, BUS_MCEERR_AO: si_addr_lsb.
            if sig == 7 && matches!(code, 4 | 5) {
                put(80, &(f[1] as u16).to_le_bytes());
            }
        }
        // NSIGPOLL, or SI_SIGIO: band, fd.
        1..=6 | -5 => {
            put(28, &(f[0] as u32).to_le_bytes());
            put(20, &(f[1] as u32).to_le_bytes());
        }
        // SI_TIMER: tid, overrun, sigval.
        -2 => {
            put(24, &(f[0] as u32).to_le_bytes());
            put(32, &((f[0] >> 32) as u32).to_le_bytes());
            value(&mut put);
        }
        ..=-1 => {
            sender(&mut put);
            value(&mut put);
        }
        _ => sender(&mut put),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::GuestContext,
        errno::{EAGAIN, EBADF, EFAULT},
        sys::{self, fs},
    };

    fn isolated(name: &str) -> bool {
        fdtab::isolated_kernel_test(&format!("sys::signalfd::tests::{name}"))
    }

    fn context() -> &'static mut GuestContext {
        let ctx = Box::leak(Box::new(unsafe { std::mem::zeroed() }));
        let stacks = unsafe { crate::context::bind(ctx, 0) };
        sys::register_current(ctx, stacks);
        ctx
    }

    fn create(mask: u64, flags: u64) -> i64 {
        signalfd4([u64::MAX, &mask as *const u64 as u64, 8, flags, 0, 0])
    }

    fn receive(fd: i64, bytes: &mut [u8]) -> i64 {
        fs::read([
            fd as u64,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
            0,
        ])
    }

    fn queue(sig: u64, value: u64, thread: bool) {
        let info = Siginfo::from_sender(sig as i32, -1, 123, 456);
        let mut info = info;
        info.fields[1] = value;
        let pid = sys::process::getpid() as u64;
        let result = if thread {
            signal::rt_tgsigqueueinfo([
                pid,
                sys::thread::gettid() as u64,
                sig,
                &info as *const _ as u64,
                0,
                0,
            ])
        } else {
            signal::rt_sigqueueinfo([pid, sig, &info as *const _ as u64, 0, 0, 0])
        };
        assert_eq!(result, 0);
    }

    #[test]
    fn signalfd4_01() {
        if isolated("signalfd4_01") {
            return;
        }
        let _view = crate::vfs::test_view();
        for flags in [0, CLOEXEC, NONBLOCK, NONBLOCK | CLOEXEC] {
            let fd = create(0, flags);
            assert!(fd >= 0, "create: {fd}");
            let fd = fd as u64;
            assert_eq!(
                fs::fcntl([fd, 1, 0, 0, 0, 0]),
                i64::from(flags & CLOEXEC != 0)
            );
            assert_eq!(
                fs::fcntl([fd, 3, 0, 0, 0, 0]) & NONBLOCK as i64,
                (flags & NONBLOCK) as i64
            );
            for input in [0, 1, 2, 3, u64::MAX, 0] {
                assert_eq!(fs::fcntl([fd, 2, input, 0, 0, 0]), 0);
                assert_eq!(fs::fcntl([fd, 1, 0, 0, 0, 0]), (input & 1) as i64);
            }
            assert_eq!(fs::close([fd, 0, 0, 0, 0, 0]), 0);
        }
    }

    #[test]
    fn signalfd4_02() {
        if isolated("signalfd4_02") {
            return;
        }
        let _view = crate::vfs::test_view();
        let mask = 0u64;
        let ptr = &mask as *const _ as u64;
        for size in [0, 7, 9, 128] {
            assert_eq!(signalfd4([u64::MAX, ptr, size, 0, 0, 0]), -(EINVAL as i64));
        }
        for flags in [1, 2, 0x8000_0000, NONBLOCK | 1] {
            assert_eq!(create(0, flags), -(EINVAL as i64));
        }
        assert_eq!(signalfd4([u64::MAX, 0, 8, 0, 0, 0]), -(EFAULT as i64));
        assert_eq!(
            signalfd4([(-2i64) as u64, ptr, 8, 0, 0, 0]),
            -(EBADF as i64)
        );
        let ordinary = sys::event::eventfd2([0; 6]);
        assert!(ordinary >= 0);
        assert_eq!(
            signalfd4([ordinary as u64, ptr, 8, 0, 0, 0]),
            -(EINVAL as i64)
        );
        assert_eq!(fs::close([ordinary as u64, 0, 0, 0, 0, 0]), 0);
    }

    #[test]
    fn signalfd01() {
        if isolated("signalfd01") {
            return;
        }
        let _view = crate::vfs::test_view();
        let ctx = context();
        let mask = (1u64 << 9) | (1 << 11) | (1 << 34);
        assert_eq!(
            signal::rt_sigprocmask([0, &mask as *const _ as u64, 0, 8, 0, 0]),
            0
        );
        let fd = sys::handle(
            ctx,
            74,
            [
                u64::MAX,
                &mask as *const _ as u64,
                8,
                NONBLOCK | CLOEXEC,
                0,
                0,
            ],
        );
        assert!(fd >= 0);
        let alias = fs::dup([fd as u64, 0, 0, 0, 0, 0]);
        assert!(alias >= 0);
        let mut bytes = [0xabu8; 385];
        assert_eq!(receive(fd, &mut bytes[..127]), -(EINVAL as i64));
        assert_eq!(receive(fd, &mut bytes), -(EAGAIN as i64));
        queue(10, 10, false);
        queue(10, 99, false); // Standard signals coalesce.
        queue(12, 12, true); // Thread-directed queue is consulted first.
        queue(35, 35, false);
        queue(35, 36, false); // Real-time signals retain their FIFO values.
        assert_eq!(receive(alias, &mut bytes), 384);
        for (index, (sig, value)) in [(12u32, 12u64), (10, 10), (35, 35)].into_iter().enumerate() {
            let mut expected = [0; 128];
            expected[..4].copy_from_slice(&sig.to_le_bytes());
            expected[8..12].copy_from_slice(&(-1i32).to_le_bytes());
            expected[12..16].copy_from_slice(&123u32.to_le_bytes());
            expected[16..20].copy_from_slice(&456u32.to_le_bytes());
            expected[44..48].copy_from_slice(&(value as u32).to_le_bytes());
            expected[48..56].copy_from_slice(&value.to_le_bytes());
            assert_eq!(&bytes[index * 128..(index + 1) * 128], &expected);
        }
        assert_eq!(bytes[384], 0xab);
        let empty = signal::UNBLOCKABLE;
        assert_eq!(
            signalfd4([fd as u64, &empty as *const _ as u64, 8, 0, 0, 0]),
            fd
        );
        assert_eq!(receive(alias, &mut bytes), -(EAGAIN as i64));
        let updated = 1u64 << 34;
        assert_eq!(
            signalfd4([alias as u64, &updated as *const _ as u64, 8, 0, 0, 0]),
            alias
        );
        assert_eq!(receive(fd, &mut bytes), 128);
        assert_eq!(u64::from_le_bytes(bytes[48..56].try_into().unwrap()), 36);
        assert_eq!(fs::fcntl([fd as u64, 1, 0, 0, 0, 0]), 1);
        assert_eq!(fs::fcntl([alias as u64, 1, 0, 0, 0, 0]), 0);
        assert_eq!(receive(fd, &mut bytes), -(EAGAIN as i64));
        for fd in [fd, alias] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    fn block(mask: u64) {
        assert_eq!(
            signal::rt_sigprocmask([0, &mask as *const _ as u64, 0, 8, 0, 0]),
            0
        );
    }

    fn polled(fd: i64, ms: i64) -> i64 {
        let mut entry = libc::pollfd {
            fd: fd as i32,
            events: 1,
            revents: 0,
        };
        let timeout = [ms / 1000, ms % 1000 * 1_000_000];
        let n = sys::poll::ppoll([
            &mut entry as *mut _ as u64,
            1,
            timeout.as_ptr() as u64,
            0,
            0,
            0,
        ]);
        assert!(n == 0 || entry.revents == 1, "revents {}", entry.revents);
        n
    }

    fn later(sig: u64) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            queue(sig, 7, false);
        })
    }

    #[test]
    fn pending_signal_is_readable_in_poll_and_epoll() {
        if isolated("pending_signal_is_readable_in_poll_and_epoll") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask | 1 << 35);
        let fd = create(mask, NONBLOCK);
        assert!(fd >= 0);
        let ep = sys::epoll::epoll_create1([0; 6]);
        assert!(ep >= 0);
        let interest = [1u64, 42];
        assert_eq!(
            sys::epoll::epoll_ctl([ep as u64, 1, fd as u64, interest.as_ptr() as u64, 0, 0]),
            0
        );
        assert_eq!(polled(fd, 0), 0);
        queue(36, 1, false); // Outside the mask.
        assert_eq!(polled(fd, 0), 0);
        queue(35, 1, false);
        let mut event = [0u64; 2];
        let wait = |event: &mut [u64; 2], ms| {
            sys::epoll::epoll_pwait([ep as u64, event.as_mut_ptr() as u64, 1, ms, 0, 0])
        };
        assert_eq!((polled(fd, 0), wait(&mut event, 0), event), (1, 1, [1, 42]));
        let mut bytes = [0u8; 256];
        assert_eq!(receive(fd, &mut bytes), 128);
        assert_eq!((polled(fd, 0), wait(&mut event, 0)), (0, 0));
        // A mask that now covers a pending signal is readable, and stops
        // being readable when it no longer does.
        let wide = mask | 1 << 35;
        assert_eq!(
            signalfd4([fd as u64, &wide as *const _ as u64, 8, 0, 0, 0]),
            fd
        );
        assert_eq!(polled(fd, 0), 1);
        assert_eq!(
            signalfd4([fd as u64, &mask as *const _ as u64, 8, 0, 0, 0]),
            fd
        );
        assert_eq!(polled(fd, 0), 0);
        // Arrival wakes a waiting poll and epoll.
        let sender = later(35);
        assert_eq!(polled(fd, 5000), 1);
        sender.join().unwrap();
        assert_eq!(receive(fd, &mut bytes), 128);
        let sender = later(35);
        assert_eq!(wait(&mut event, 5000), 1);
        sender.join().unwrap();
        for fd in [fd, ep] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    #[test]
    fn blocking_read_waits_for_a_signal() {
        if isolated("blocking_read_waits_for_a_signal") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask);
        let fd = create(mask, 0);
        assert!(fd >= 0);
        let sender = later(35);
        let mut bytes = [0u8; 128];
        assert_eq!(receive(fd, &mut bytes), 128);
        sender.join().unwrap();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 35);
        // A mask update wakes a reader to look again.
        let other = 1u64 << 35;
        let alias = fs::dup([fd as u64, 0, 0, 0, 0, 0]);
        block(mask | other);
        queue(36, 1, false);
        let updater = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert_eq!(
                signalfd4([alias as u64, &other as *const _ as u64, 8, 0, 0, 0]),
                alias
            );
        });
        assert_eq!(receive(fd, &mut bytes), 128);
        updater.join().unwrap();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 36);
        for fd in [fd, alias] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    #[test]
    fn readv_fills_records_across_vectors() {
        if isolated("readv_fills_records_across_vectors") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask);
        let fd = create(mask, NONBLOCK);
        queue(35, 1, false);
        queue(35, 2, false);
        let mut bytes = [0u8; 300];
        let (first, rest) = bytes.split_at_mut(100);
        let vectors = [
            libc::iovec {
                iov_base: first.as_mut_ptr().cast(),
                iov_len: 100,
            },
            libc::iovec {
                iov_base: rest.as_mut_ptr().cast(),
                iov_len: 200,
            },
        ];
        assert_eq!(
            fs::readv([fd as u64, vectors.as_ptr() as u64, 2, 0, 0, 0]),
            256
        );
        assert_eq!(u64::from_le_bytes(bytes[48..56].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(bytes[176..184].try_into().unwrap()), 2);
        assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    /// The fork and exec state round trips in this process: the restored
    /// description is a new socket under the same fds, with the mask, the
    /// fd flags and O_NONBLOCK, readable only for signals pending here.
    fn reinstalled(restore: impl FnOnce(i64, i64)) {
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask);
        let fd = create(mask, NONBLOCK | CLOEXEC);
        let alias = fs::dup([fd as u64, 0, 0, 0, 0, 0]);
        queue(35, 1, false);
        assert_eq!(polled(alias, 0), 1);
        restore(fd, alias);
        // The old socket's token stays with the old description.
        let mut bytes = [0u8; 128];
        assert_eq!(polled(fd, 0), 0);
        assert_eq!(receive(alias, &mut bytes), -(crate::errno::EAGAIN as i64));
        assert_eq!(fs::fcntl([fd as u64, 1, 0, 0, 0, 0]), 1);
        assert_eq!(fs::fcntl([alias as u64, 1, 0, 0, 0, 0]), 0);
        queue(35, 2, false);
        assert_eq!(polled(alias, 0), 1);
        assert_eq!(receive(fd, &mut bytes), 128);
        assert_eq!(u64::from_le_bytes(bytes[48..56].try_into().unwrap()), 2);
        let other = 1u64 << 35;
        assert_eq!(
            signalfd4([alias as u64, &other as *const _ as u64, 8, 0, 0, 0]),
            alias
        );
        let Some(Kind::Signal(description)) = fdtab::get(fd as i32) else {
            panic!("restored signalfd missing");
        };
        assert_eq!(description.mask.load(SeqCst), other);
        for fd in [fd, alias] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    /// Consume this process's pending signal, as a new process has none.
    fn forget_pending() {
        let set = 1u64 << 34;
        let timeout = [0i64; 2];
        assert_eq!(
            signal::rt_sigtimedwait([&set as *const _ as u64, 0, timeout.as_ptr() as u64, 8, 0, 0]),
            35
        );
    }

    #[test]
    fn fork_child_has_its_own_readiness() {
        if isolated("fork_child_has_its_own_readiness") {
            return;
        }
        reinstalled(|fd, alias| {
            let mut writer = sys::fork_state::Writer::default();
            fdtab::fork_save(&mut writer);
            let saved = writer.into_bytes();
            for fd in [fd, alias] {
                fdtab::withdraw_guest(fd as i32).unwrap();
                fdtab::on_close(fd as i32);
            }
            forget_pending();
            let mut reader = sys::fork_state::Reader::new(&saved);
            fdtab::fork_restore(&mut reader);
            assert!(reader.ok());
        });
    }

    #[test]
    fn exec_keeps_signalfd() {
        if isolated("exec_keeps_signalfd") {
            return;
        }
        reinstalled(|fd, alias| {
            let text = fdtab::regular_exec_text().unwrap();
            for fd in [fd, alias] {
                fdtab::on_close(fd as i32);
            }
            forget_pending();
            fdtab::restore_regular_exec(&text).unwrap();
        });
    }

    #[test]
    fn record_layouts() {
        let mut info = Siginfo::new(17, 1); // CLD_EXITED
        info.fields = [7 | 8 << 32, 3, 11, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let out = record(&info);
        let word =
            |out: &[u8; 128], at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        let long =
            |out: &[u8; 128], at: usize| u64::from_le_bytes(out[at..at + 8].try_into().unwrap());
        assert_eq!((word(&out, 12), word(&out, 16), word(&out, 40)), (7, 8, 3));
        assert_eq!(
            (long(&out, 56), long(&out, 64), long(&out, 48)),
            (11, 12, 0)
        );
        info.code = 0x80; // SI_KERNEL: pid and uid only.
        let out = record(&info);
        assert_eq!((word(&out, 12), word(&out, 40), long(&out, 56)), (7, 0, 0));
        info.code = -6; // SI_TKILL: the _rt layout.
        let out = record(&info);
        assert_eq!((word(&out, 12), word(&out, 44), long(&out, 48)), (7, 3, 3));
        info.signo = 7; // SIGBUS, BUS_MCEERR_AR: address and its lsb.
        info.code = 4;
        let out = record(&info);
        assert_eq!(
            (long(&out, 72), out[80], word(&out, 12)),
            (7 | 8 << 32, 3, 0)
        );
        info.code = 6; // Beyond NSIGBUS: the _sigpoll layout.
        let out = record(&info);
        assert_eq!((word(&out, 28), word(&out, 20), long(&out, 72)), (7, 3, 0));
        info.code = -2; // SI_TIMER: tid, overrun, value.
        let out = record(&info);
        assert_eq!((word(&out, 24), word(&out, 32), long(&out, 48)), (7, 8, 3));
        info.signo = 31; // SIGSYS, SYS_SECCOMP: call address, syscall, arch.
        info.code = 1;
        let out = record(&info);
        assert_eq!(
            (long(&out, 88), word(&out, 84), word(&out, 96)),
            (7 | 8 << 32, 3, 0)
        );
        assert!(out[100..].iter().all(|&byte| byte == 0));
    }
}
