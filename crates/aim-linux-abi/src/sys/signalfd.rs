//! signalfd: a descriptor that reads the reader's pending signals in its
//! mask as Linux's `signalfd_siginfo` records (signalfd(2), `fs/signalfd.c`).
//!
//! Pending state belongs to the signal owner. The mask is the open file's,
//! so it lives in an unlinked host file every process holding the file
//! shares (fork passes it, exec keeps it): a `signalfd4(fd, …)` update in
//! one is what the others read and poll, as Linux's share the `struct
//! file`. The guest end is a Darwin datagram socket, so it polls like an
//! eventfd: the description keeps one token record in it exactly while the
//! process or one of its threads has a signal in the mask pending
//! ([`refresh`], called by the signal owner whenever a pending set
//! changes; [`ready`], whenever a poll looks at it, as `signalfd_poll`
//! reads the mask then). A Linux poll looks at the polling thread's own
//! queue; here a signal pending on another thread counts too (#2653). A
//! blocked read reads the mask whenever a signal is sent to the process
//! (`signalfd_notify`). It reports
//! POLLIN only. Each process has its own socket for a
//! description, so a forked child or a relaunched exec image sees only its
//! own signals: [`install`] puts a fresh one under every guest fd that
//! shares the description, keeping its flags.

use std::os::fd::BorrowedFd;
use std::sync::{Arc, Mutex, Weak};

use super::{
    fdtab::{self, Kind},
    sigframe::Siginfo,
    signal, user_memory,
};
use crate::errno::{self, EINVAL, Errno};

const NONBLOCK: u64 = 0o4000;
const CLOEXEC: u64 = 0o2000000;
const RECORD: usize = 128;

pub struct SignalFd {
    /// The hidden unlinked file holding the mask.
    shared: i32,
    /// The hidden peer that sends the token, and a hidden twin of the guest
    /// end that drains it and is installed again after fork or exec.
    peer: i32,
    reader: i32,
    token: Mutex<bool>,
}

static DESCRIPTIONS: Mutex<Vec<Weak<SignalFd>>> = Mutex::new(Vec::new());

impl Drop for SignalFd {
    fn drop(&mut self) {
        for fd in [self.shared, self.peer, self.reader] {
            if let Err(error) = fdtab::close_fork_private(fd) {
                crate::diag!("signalfd private end close: errno {error}");
            }
        }
    }
}

/// A new unlinked file holding `mask`, hidden.
fn mask_file(mask: u64) -> Result<i32, Errno> {
    let mut template = std::env::temp_dir()
        .join("linux-abi-signalfd.XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    template.push(0);
    // SAFETY: mkstemp fills in the template; the name is removed at once.
    let fd = unsafe { libc::mkstemp(template.as_mut_ptr().cast()) };
    if fd < 0 {
        return Err(errno::last());
    }
    // SAFETY: removing the name just created.
    unsafe { libc::unlink(template.as_ptr().cast()) };
    let fd = fdtab::hide(fd);
    if let Err(error) = write_mask(fd, mask) {
        if let Err(error) = fdtab::close_fork_private(fd) {
            crate::diag!("signalfd mask file close: errno {error}");
        }
        return Err(error);
    }
    Ok(fd)
}

fn write_mask(fd: i32, mask: u64) -> Result<(), Errno> {
    // SAFETY: writing 8 bytes from a local.
    match unsafe { libc::pwrite(fd, (&mask as *const u64).cast(), 8, 0) } {
        8 => Ok(()),
        n if n < 0 => Err(errno::last()),
        _ => Err(errno::EIO),
    }
}

impl SignalFd {
    /// A description of the mask in `shared`, which it takes, and its
    /// guest end, with the token for what is pending.
    fn open(shared: i32, flags: u64) -> Result<(i32, Arc<SignalFd>), Errno> {
        let close = |fds: &[i32]| {
            for &fd in fds {
                if let Err(error) = fdtab::close_fork_private(fd) {
                    crate::diag!("signalfd private end close: errno {error}");
                }
            }
        };
        let (fd, peer) = match super::event::pair(flags) {
            Ok(pair) => pair,
            Err(error) => {
                close(&[shared]);
                return Err((-error) as Errno);
            }
        };
        // SAFETY: duplicating the socket just created.
        let reader = unsafe { libc::dup(fd) };
        if reader < 0 {
            let error = errno::last();
            // SAFETY: closing the unpublished socket.
            unsafe { libc::close(fd) };
            close(&[shared, peer]);
            return Err(error);
        }
        let description = Arc::new(SignalFd {
            shared,
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

    fn mask(&self) -> Result<u64, Errno> {
        let mut mask = 0u64;
        // SAFETY: reading 8 bytes into a local.
        match unsafe { libc::pread(self.shared, (&mut mask as *mut u64).cast(), 8, 0) } {
            8 => Ok(mask),
            n if n < 0 => Err(errno::last()),
            _ => Err(errno::EIO),
        }
    }

    /// Keep the token iff a signal in the mask is pending, and say whether
    /// it is. The token's lock orders the snapshots, so the last change is
    /// the one that stays. A mask that cannot be read leaves the
    /// descriptor readable: its read reports the error.
    fn sync(&self) -> bool {
        let mut token = self.token.lock().unwrap();
        let ready = self
            .mask()
            .map_or(true, |mask| mask & signal::all_pending() != 0);
        if ready && !*token {
            *token = super::event::send_record(self.peer, 1);
        } else if !ready && *token {
            super::event::recv_record(self.reader);
            *token = false;
        }
        ready
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

/// A poll looks at `description`: whether it is readable now, with its
/// token brought up to date for a mask another process changed.
pub(super) fn ready(description: &SignalFd) -> bool {
    description.sync()
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
        if let Err(error) = write_mask(description.shared, mask) {
            return -(error as i64);
        }
        description.sync();
        signal::wake_readers();
        return fd as i64;
    }
    let opened = mask_file(mask).and_then(|shared| SignalFd::open(shared, flags));
    let (fd, description) = match opened {
        Ok(open) => open,
        Err(error) => return -(error as i64),
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
/// rest take only what is already pending. A record that cannot be copied
/// out ends the read with the ones before it, or EFAULT, as
/// `signalfd_read_iter` (the signal it held is gone).
pub fn read(fd: i32, description: &SignalFd, iov: &[libc::iovec]) -> i64 {
    let len: usize = iov.iter().map(|v| v.iov_len).sum();
    if len < RECORD {
        return -(EINVAL as i64);
    }
    let mut done = 0;
    while done + RECORD <= len {
        let nonblock = done != 0 || fdtab::nonblocking(fd);
        let info = match signal::read_signal(|| description.mask(), nonblock) {
            Ok(info) => info,
            Err(_) if done != 0 => break,
            Err(error) => return error,
        };
        if let Err(error) = place(iov, done, &record(&info)) {
            return if done != 0 {
                done as i64
            } else {
                -(error as i64)
            };
        }
        done += RECORD;
    }
    done as i64
}

/// Copy `bytes` into the vectors from byte `offset` on.
fn place(iov: &[libc::iovec], mut offset: usize, mut bytes: &[u8]) -> Result<(), Errno> {
    for vector in iov {
        if offset >= vector.iov_len {
            offset -= vector.iov_len;
            continue;
        }
        let count = (vector.iov_len - offset).min(bytes.len());
        user_memory::write_exact(vector.iov_base as u64 + offset as u64, &bytes[..count])?;
        bytes = &bytes[count..];
        offset = 0;
        if bytes.is_empty() {
            break;
        }
    }
    Ok(())
}

/// Fork and host exec: the mask's file, which a fork child or the new
/// image receives under the same number.
pub(super) fn shared(description: &SignalFd) -> BorrowedFd<'_> {
    // SAFETY: the description's own open fd.
    unsafe { BorrowedFd::borrow_raw(description.shared) }
}

/// A new description of this process for the mask file received as
/// `shared`, which the caller closes once restored.
pub(super) fn load(shared: i32) -> Result<Arc<SignalFd>, Errno> {
    // SAFETY: duplicating the received fd.
    let copy = unsafe { libc::fcntl(shared, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(errno::last());
    }
    let (fd, description) = SignalFd::open(fdtab::hide(copy), 0)?;
    // SAFETY: the unpublished socket; `reader` keeps it open.
    unsafe { libc::close(fd) };
    Ok(description)
}

/// Put this process's socket of `description` under guest `fd`, keeping
/// the fd's FD_CLOEXEC and the file's O_NONBLOCK.
pub(super) fn install(fd: i32, description: &SignalFd) -> Result<(), Errno> {
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
        assert_eq!(description_of(fd).mask(), Ok(other));
        for fd in [fd, alias] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    fn description_of(fd: i64) -> Arc<SignalFd> {
        let Some(Kind::Signal(description)) = fdtab::get(fd as i32) else {
            panic!("signalfd {fd} missing");
        };
        description
    }

    fn identity(fd: BorrowedFd<'_>) -> (i32, u64) {
        use std::os::fd::AsRawFd;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), &mut st) }, 0);
        (st.st_dev, st.st_ino)
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
            let parent = description_of(fd);
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
            // The child received the parent's mask file, which stays the
            // parent's here: an update through either is the other's.
            fdtab::take_restored_private_targets();
            let child = description_of(fd);
            assert_eq!(identity(shared(&child)), identity(shared(&parent)));
            let other = 1u64 << 35;
            assert_eq!(
                signalfd4([alias as u64, &other as *const _ as u64, 8, 0, 0, 0]),
                alias
            );
            assert_eq!(parent.mask(), Ok(other));
            write_mask(parent.shared, 1 << 34).unwrap();
            assert_eq!(child.mask(), Ok(1 << 34));
        });
    }

    #[test]
    fn exec_keeps_signalfd() {
        if isolated("exec_keeps_signalfd") {
            return;
        }
        reinstalled(|fd, alias| {
            let image = description_of(fd);
            let file = identity(shared(&image));
            let text = fdtab::regular_exec_text().unwrap();
            for fd in [fd, alias] {
                fdtab::on_close(fd as i32);
            }
            forget_pending();
            fdtab::restore_regular_exec(&text).unwrap();
            // The old image ends without dropping its description; the new
            // one has its mask file.
            std::mem::forget(image);
            assert_eq!(identity(shared(&description_of(fd))), file);
        });
    }

    fn poll_events(fd: i64, events: i16) -> (i64, i16) {
        let mut entry = libc::pollfd {
            fd: fd as i32,
            events,
            revents: 0,
        };
        let timeout = [0i64; 2];
        let n = sys::poll::ppoll([
            &mut entry as *mut _ as u64,
            1,
            timeout.as_ptr() as u64,
            0,
            0,
            0,
        ]);
        (n, entry.revents)
    }

    fn epoll_with(fd: i64, events: u64) -> i64 {
        let ep = sys::epoll::epoll_create1([0; 6]);
        assert!(ep >= 0);
        let interest = [events, 7];
        assert_eq!(
            sys::epoll::epoll_ctl([ep as u64, 1, fd as u64, interest.as_ptr() as u64, 0, 0]),
            0
        );
        ep
    }

    fn epoll_now(ep: i64) -> (i64, [u64; 2]) {
        let mut event = [0u64; 2];
        let n = sys::epoll::epoll_pwait([ep as u64, event.as_mut_ptr() as u64, 1, 0, 0, 0]);
        (n, event)
    }

    #[test]
    fn readiness_is_pollin_only() {
        if isolated("readiness_is_pollin_only") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask);
        let fd = create(mask, NONBLOCK);
        assert!(fd >= 0);
        // POLLIN, POLLPRI, POLLOUT, POLLRDNORM, POLLWRNORM.
        let all = 0x1 | 0x2 | 0x4 | 0x40 | 0x100;
        assert_eq!(poll_events(fd, all), (0, 0));
        let read_write = epoll_with(fd, 0x1 | 0x4 | 0x40);
        let write_only = epoll_with(fd, 0x4);
        assert_eq!(epoll_now(read_write).0, 0);
        queue(35, 1, false);
        assert_eq!(poll_events(fd, all), (1, 0x1));
        assert_eq!(poll_events(fd, 0x4 | 0x40), (0, 0));
        assert_eq!(epoll_now(read_write), (1, [0x1, 7]));
        assert_eq!(epoll_now(write_only).0, 0);
        // The write-only interest is registered all the same.
        let interest = [0x4u64, 8];
        assert_eq!(
            sys::epoll::epoll_ctl([
                write_only as u64,
                3,
                fd as u64,
                interest.as_ptr() as u64,
                0,
                0
            ]),
            0
        );
        // select: in the read set, never in the write set.
        let (mut read, mut write) = (1u64 << fd, 1u64 << fd);
        let timeout = [0i64; 2];
        assert_eq!(
            sys::poll::pselect6([
                fd as u64 + 1,
                &mut read as *mut _ as u64,
                &mut write as *mut _ as u64,
                0,
                timeout.as_ptr() as u64,
                0,
            ]),
            1
        );
        assert_eq!((read, write), (1 << fd, 0));
        for fd in [fd, read_write, write_only] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    #[test]
    fn poll_reads_a_mask_another_process_changed() {
        if isolated("poll_reads_a_mask_another_process_changed") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let (mask, other) = (1u64 << 34, 1u64 << 35);
        block(mask | other);
        let fd = create(mask, NONBLOCK);
        let ep = epoll_with(fd, 0x1);
        queue(36, 1, false);
        assert_eq!(polled(fd, 0), 0);
        // Another process holding the file writes the mask, with no signal
        // here to refresh the token.
        let description = description_of(fd);
        write_mask(description.shared, other).unwrap();
        assert_eq!(polled(fd, 0), 1);
        assert_eq!(epoll_now(ep), (1, [0x1, 7]));
        write_mask(description.shared, mask).unwrap();
        assert_eq!(epoll_now(ep).0, 0);
        assert_eq!(polled(fd, 0), 0);
        // A one-shot interest whose only readiness was stale stays armed,
        // as `ep_item_poll` found nothing to report.
        let once = epoll_with(fd, 0x1 | 1 << 30);
        write_mask(description.shared, other).unwrap();
        assert_eq!(polled(fd, 0), 1);
        write_mask(description.shared, mask).unwrap();
        assert_eq!(epoll_now(once).0, 0);
        queue(35, 1, false);
        assert_eq!(epoll_now(once), (1, [0x1, 7]));
        assert_eq!(epoll_now(once).0, 0);
        for fd in [fd, ep, once] {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    #[test]
    fn a_blocked_read_follows_a_mask_another_process_changed() {
        if isolated("a_blocked_read_follows_a_mask_another_process_changed") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let (mask, other) = (1u64 << 34, 1u64 << 35);
        block(mask | other);
        let fd = create(mask, 0);
        // Another process holding the file moves the mask, which does not
        // wake this one's reader; the next signal sent here does, outside
        // the mask the reader started with (`signalfd_notify`).
        let description = description_of(fd);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            write_mask(description.shared, other).unwrap();
            queue(36, 2, false);
        });
        let mut bytes = [0u8; RECORD];
        assert_eq!(receive(fd, &mut bytes), RECORD as i64);
        sender.join().unwrap();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 36);
        assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    #[test]
    fn a_fault_after_a_record_returns_the_records_before_it() {
        if isolated("a_fault_after_a_record_returns_the_records_before_it") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask);
        let fd = create(mask, NONBLOCK);
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        // SAFETY: a private anonymous mapping of our own.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * page,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(p, libc::MAP_FAILED);
        // SAFETY: our second page.
        assert_eq!(
            unsafe { libc::mprotect(p.byte_add(page), page, libc::PROT_NONE) },
            0
        );
        for value in 1..=3 {
            queue(35, value, false);
        }
        // The copy itself faults, as when another thread unmaps the buffer
        // during a read: the syscall layer has checked the range before.
        let description = description_of(fd);
        let read = |at: usize, len: usize| {
            let vector = libc::iovec {
                iov_base: at as *mut _,
                iov_len: len,
            };
            read(fd as i32, &description, &[vector])
        };
        let first = p as usize + page - RECORD;
        assert_eq!(read(first, 2 * RECORD), RECORD as i64);
        // SAFETY: the record copied into our first page.
        assert_eq!(unsafe { ((first + 48) as *const u64).read_unaligned() }, 1);
        // The signal whose record faulted is consumed, as on Linux; with
        // no record copied the read fails.
        assert_eq!(
            read(p as usize + page, RECORD),
            -(crate::errno::EFAULT as i64)
        );
        let mut bytes = [0u8; RECORD];
        assert_eq!(receive(fd, &mut bytes), -(crate::errno::EAGAIN as i64));
        // SAFETY: our mapping.
        assert_eq!(unsafe { libc::munmap(p, 2 * page) }, 0);
        assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    extern "C" fn exit_with_a_pending_signal(_: *mut libc::c_void) -> *mut libc::c_void {
        context();
        block(1 << 34);
        queue(35, 1, true);
        sys::thread::exit([0; 6])
    }

    #[test]
    fn an_exiting_thread_takes_its_pending_signals() {
        if isolated("an_exiting_thread_takes_its_pending_signals") {
            return;
        }
        let _view = crate::vfs::test_view();
        context();
        let mask = 1u64 << 34;
        block(mask);
        let fd = create(mask, NONBLOCK);
        // SAFETY: a joinable thread we join.
        unsafe {
            let mut thread: libc::pthread_t = std::mem::zeroed();
            assert_eq!(
                libc::pthread_create(
                    &mut thread,
                    std::ptr::null(),
                    exit_with_a_pending_signal,
                    std::ptr::null_mut()
                ),
                0
            );
            assert_eq!(libc::pthread_join(thread, std::ptr::null_mut()), 0);
        }
        // The token is gone from the socket itself, which a wait already
        // blocked on it watches, not only from what a new poll reads.
        let mut host = libc::pollfd {
            fd: fd as i32,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut host, 1, 0) }, 0);
        assert_eq!(polled(fd, 0), 0);
        let mut bytes = [0u8; RECORD];
        assert_eq!(receive(fd, &mut bytes), -(crate::errno::EAGAIN as i64));
        assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
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
