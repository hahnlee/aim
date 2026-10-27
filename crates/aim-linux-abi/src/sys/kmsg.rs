//! The kernel log buffer (printk's ring): records written to `/dev/kmsg`,
//! read back from `/dev/kmsg` (each open description its own reader),
//! `/proc/kmsg` and syslog(2), as Linux v6.12 serves them
//! (kernel/printk/printk.c).
//!
//! The ring is one file, the runtime `/dev/kmsg` guest-init creates, shared
//! by every guest process: a header, then each record as one line in
//! `/dev/kmsg`'s read format (`prefix,seq,usec,-;text`, the text escaped as
//! `msg_print_ext_body` escapes it). A record keeps the offset it was
//! written at, so a `/dev/kmsg` reader's position is its host description's
//! file offset, shared across dup, fork and SCM_RIGHTS as Linux shares the
//! open file's. The ring holds `LOG_BUF_LEN` bytes of records; older ones
//! are dropped and their blocks punched out. Changes are made under an
//! exclusive flock of the layer's own description. A blocking read waits
//! on the file's vnode events.
//!
//! `/proc/kmsg` is guest-init's marker file in the runtime `/proc`; reads
//! of a description of it take the shared syslog position. poll on either
//! reports a regular file's readiness (#1944).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex, OnceLock};

use super::fdtab::{self, Kind};
use crate::errno::{EAGAIN, EINTR, EINVAL, EIO, EPERM, Errno};
use crate::vfs;

/// `1 << CONFIG_LOG_BUF_SHIFT` of GKI kernels (17).
const LOG_BUF_LEN: u64 = 1 << 17;
/// `PRINTKRB_RECORD_MAX`: the longest record a write may make.
const RECORD_MAX: usize = 1024;
/// `MESSAGE_LOGLEVEL_DEFAULT`, `LOG_USER`.
const DEFAULT_LEVEL: u32 = 4;
const LOG_USER: u32 = 1;
/// `CONSOLE_LOGLEVEL_DEFAULT`, `CONSOLE_LOGLEVEL_MIN`.
const CONSOLE_DEFAULT: u64 = 7;
const CONSOLE_MIN: u64 = 1;
/// The devkmsg ratelimit (`DEFAULT_RATELIMIT_INTERVAL`, `_BURST`).
const RATELIMIT_INTERVAL_NS: u64 = 5_000_000_000;
const RATELIMIT_BURST: u32 = 10;
const ESPIPE: Errno = 29;
const EPIPE: Errno = 32;
const CAP_SYS_ADMIN: u32 = 21;
const CAP_SYSLOG: u32 = 34;

const MAGIC: &[u8; 8] = b"AIMKMSG1";
const HEADER: u64 = 128;
const LINE_MAX: usize = 64 + 4 * RECORD_MAX;

/// syslog(2)'s actions.
const ACTION_CLOSE: i32 = 0;
const ACTION_OPEN: i32 = 1;
const ACTION_READ: i32 = 2;
const ACTION_READ_ALL: i32 = 3;
const ACTION_READ_CLEAR: i32 = 4;
const ACTION_CLEAR: i32 = 5;
const ACTION_CONSOLE_OFF: i32 = 6;
const ACTION_CONSOLE_ON: i32 = 7;
const ACTION_CONSOLE_LEVEL: i32 = 8;
const ACTION_SIZE_UNREAD: i32 = 9;
const ACTION_SIZE_BUFFER: i32 = 10;

/// Which kernel log file a description is.
pub enum Open {
    /// `/dev/kmsg`, with its writer's ratelimit (`devkmsg_user.rs`).
    Dev(Mutex<Ratelimit>),
    /// `/proc/kmsg`.
    Proc,
}

pub struct Ratelimit {
    begin: u64,
    printed: u32,
    missed: u32,
}

impl Drop for Ratelimit {
    /// `ratelimit_state_exit` at release (RATELIMIT_MSG_ON_RELEASE).
    fn drop(&mut self) {
        if self.missed > 0 {
            let text = format!(
                "{}: {} output lines suppressed due to ratelimiting",
                super::procfs::own_comm(),
                self.missed
            );
            let _ = with_ring(|ring| ring.append(0, DEFAULT_LEVEL, text.as_bytes()));
        }
    }
}

impl Open {
    /// The kind a fork child restores.
    pub fn restore(proc: bool) -> Open {
        if proc {
            Open::Proc
        } else {
            Open::Dev(Mutex::new(Ratelimit::new()))
        }
    }
}

/// The ring's header: the record counters and the readers kept in it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Header {
    next_seq: u64,
    end: u64,
    first_seq: u64,
    first_off: u64,
    syslog_seq: u64,
    syslog_off: u64,
    syslog_partial: u64,
    clear_seq: u64,
    clear_off: u64,
    punched: u64,
    console: u64,
    saved_console: u64,
}

impl Header {
    fn empty() -> Header {
        Header {
            end: HEADER,
            first_off: HEADER,
            syslog_off: HEADER,
            clear_off: HEADER,
            punched: 0,
            console: CONSOLE_DEFAULT,
            // LOGLEVEL_DEFAULT: none saved.
            saved_console: u64::MAX,
            ..Header::default()
        }
    }

    fn encode(&self) -> [u8; HEADER as usize] {
        let mut out = [0u8; HEADER as usize];
        out[..8].copy_from_slice(MAGIC);
        for (i, v) in self.fields().iter().enumerate() {
            out[8 + 8 * i..16 + 8 * i].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<Header> {
        if bytes.len() < HEADER as usize || &bytes[..8] != MAGIC {
            return None;
        }
        let v = |i: usize| u64::from_le_bytes(bytes[8 + 8 * i..16 + 8 * i].try_into().unwrap());
        Some(Header {
            next_seq: v(0),
            end: v(1),
            first_seq: v(2),
            first_off: v(3),
            syslog_seq: v(4),
            syslog_off: v(5),
            syslog_partial: v(6),
            clear_seq: v(7),
            clear_off: v(8),
            punched: v(9),
            console: v(10),
            saved_console: v(11),
        })
    }

    fn fields(&self) -> [u64; 12] {
        [
            self.next_seq,
            self.end,
            self.first_seq,
            self.first_off,
            self.syslog_seq,
            self.syslog_off,
            self.syslog_partial,
            self.clear_seq,
            self.clear_off,
            self.punched,
            self.console,
            self.saved_console,
        ]
    }
}

/// One record as `/dev/kmsg` reads it.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    prefix: u32,
    seq: u64,
    usec: u64,
    text: Vec<u8>,
}

impl Record {
    /// `msg_print_ext_header` and `msg_print_ext_body`.
    fn line(&self) -> Vec<u8> {
        let mut out = format!("{},{},{},-;", self.prefix, self.seq, self.usec).into_bytes();
        for &c in &self.text {
            if c < b' ' || c >= 127 || c == b'\\' {
                out.extend_from_slice(format!("\\x{c:02x}").as_bytes());
            } else {
                out.push(c);
            }
        }
        out.push(b'\n');
        out
    }

    fn parse(line: &[u8]) -> Option<Record> {
        let semi = line.iter().position(|&c| c == b';')?;
        let head = std::str::from_utf8(&line[..semi]).ok()?;
        let mut fields = head.split(',');
        let prefix = fields.next()?.parse().ok()?;
        let seq = fields.next()?.parse().ok()?;
        let usec = fields.next()?.parse().ok()?;
        let body = line[semi + 1..].strip_suffix(b"\n")?;
        let mut text = Vec::with_capacity(body.len());
        let mut i = 0;
        while i < body.len() {
            if body[i] == b'\\' && body.get(i + 1) == Some(&b'x') && i + 4 <= body.len() {
                text.push(
                    u8::from_str_radix(std::str::from_utf8(&body[i + 2..i + 4]).ok()?, 16).ok()?,
                );
                i += 4;
            } else {
                text.push(body[i]);
                i += 1;
            }
        }
        Some(Record {
            prefix,
            seq,
            usec,
            text,
        })
    }

    /// `record_print_text` with the syslog prefix and the time
    /// (CONFIG_PRINTK_TIME): every line of the text prefixed.
    fn syslog_text(&self) -> Vec<u8> {
        let prefix = format!(
            "<{}>[{:5}.{:06}] ",
            self.prefix,
            self.usec / 1_000_000,
            self.usec % 1_000_000
        );
        let mut out = Vec::new();
        for line in self.text.split(|&c| c == b'\n') {
            out.extend_from_slice(prefix.as_bytes());
            out.extend_from_slice(line);
            out.push(b'\n');
        }
        out
    }
}

/// The ring file, open on a description of the layer's own.
struct Ring {
    file: OwnedFd,
}

/// Holds the ring's flock for one change.
struct Locked<'a> {
    ring: &'a Ring,
    header: Header,
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: unlocking our own description.
        unsafe { libc::flock(self.ring.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl Ring {
    fn open() -> Result<Ring, Errno> {
        let (host, _) = vfs::lookup("/dev/kmsg");
        let c = std::ffi::CString::new(host.as_os_str().as_encoded_bytes()).map_err(|_| EIO)?;
        // SAFETY: opening the runtime's ring file.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(crate::errno::last());
        }
        Ok(Ring {
            file: unsafe { OwnedFd::from_raw_fd(fdtab::hide(fd)) },
        })
    }

    fn lock(&self) -> Result<Locked<'_>, Errno> {
        let fd = self.file.as_raw_fd();
        // SAFETY: flock of our own description.
        while unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            let error = crate::errno::last();
            if error != EINTR {
                return Err(error);
            }
        }
        let mut locked = Locked {
            ring: self,
            header: Header::empty(),
        };
        let mut bytes = [0u8; HEADER as usize];
        // SAFETY: local buffer.
        let n = unsafe { libc::pread(fd, bytes.as_mut_ptr().cast(), bytes.len(), 0) };
        match Header::decode(&bytes[..n.max(0) as usize]) {
            Some(header) => locked.header = header,
            // A ring nothing has written yet (guest-init creates it empty).
            None => {
                // SAFETY: our own description.
                unsafe { libc::ftruncate(fd, 0) };
                locked.store()?;
            }
        }
        Ok(locked)
    }

    fn pread(&self, off: u64, len: usize) -> Result<Vec<u8>, Errno> {
        let mut buf = vec![0u8; len];
        // SAFETY: local buffer.
        let n = unsafe {
            libc::pread(
                self.file.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                len,
                off as i64,
            )
        };
        if n < 0 {
            return Err(crate::errno::last());
        }
        buf.truncate(n as usize);
        Ok(buf)
    }

    /// The record line at `off`, and where the next one starts.
    fn line_at(&self, off: u64) -> Result<(Vec<u8>, u64), Errno> {
        let mut line = self.pread(off, LINE_MAX)?;
        let end = line.iter().position(|&c| c == b'\n').ok_or(EIO)?;
        line.truncate(end + 1);
        Ok((line, off + end as u64 + 1))
    }

    /// The records from `off` to `end`, each with its offset.
    fn records(&self, off: u64, end: u64) -> Result<Vec<(u64, Record)>, Errno> {
        let bytes = self.pread(off, (end - off) as usize)?;
        let mut out = Vec::new();
        let mut at = 0usize;
        while at < bytes.len() {
            let len = bytes[at..].iter().position(|&c| c == b'\n').ok_or(EIO)? + 1;
            out.push((
                off + at as u64,
                Record::parse(&bytes[at..at + len]).ok_or(EIO)?,
            ));
            at += len;
        }
        Ok(out)
    }

    /// Waits for the ring to change after `seen`, the end the caller found.
    fn wait(&self, seen: impl Fn(&Header) -> bool) -> Result<(), Errno> {
        // SAFETY: a new kqueue of our own.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(crate::errno::last());
        }
        let kq = unsafe { OwnedFd::from_raw_fd(kq) };
        let change = libc::kevent {
            ident: self.file.as_raw_fd() as usize,
            filter: libc::EVFILT_VNODE,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: libc::NOTE_WRITE | libc::NOTE_EXTEND,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one change, no events.
        if unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(crate::errno::last());
        }
        loop {
            if !seen(&self.lock()?.header) {
                return Ok(());
            }
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            // SAFETY: one event into a local.
            if unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    std::ptr::null(),
                )
            } < 0
            {
                return Err(crate::errno::last());
            }
        }
    }

    /// `vprintk_store` of one record, the oldest dropped to make room.
    fn append(&self, facility: u32, level: u32, text: &[u8]) -> Result<(), Errno> {
        let mut locked = self.lock()?;
        let h = &mut locked.header;
        // One trailing newline ends the record (LOG_NEWLINE).
        let text = text.strip_suffix(b"\n").unwrap_or(text);
        let record = Record {
            prefix: facility << 3 | level,
            seq: h.next_seq,
            usec: super::clock::Base::Monotonic.now() / 1_000,
            text: text.to_vec(),
        };
        let line = record.line();
        // SAFETY: writing our own buffer at the ring's end.
        if unsafe {
            libc::pwrite(
                self.file.as_raw_fd(),
                line.as_ptr().cast(),
                line.len(),
                h.end as i64,
            )
        } != line.len() as isize
        {
            return Err(crate::errno::last());
        }
        h.end += line.len() as u64;
        h.next_seq += 1;
        while h.end - h.first_off > LOG_BUF_LEN {
            let (_, next) = self.line_at(h.first_off)?;
            h.first_off = next;
            h.first_seq += 1;
        }
        let dead = h.first_off & !4095;
        if dead > h.punched.max(4096) {
            let start = h.punched.max(4096);
            let hole = libc::fpunchhole_t {
                fp_flags: 0,
                reserved: 0,
                fp_offset: start as i64,
                fp_length: (dead - start) as i64,
            };
            // SAFETY: F_PUNCHHOLE with a local struct.
            if unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_PUNCHHOLE, &hole) } == 0 {
                h.punched = dead;
            }
        }
        locked.store()
    }
}

impl Locked<'_> {
    fn store(&self) -> Result<(), Errno> {
        let bytes = self.header.encode();
        // SAFETY: writing our own header.
        if unsafe {
            libc::pwrite(
                self.ring.file.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                0,
            )
        } != bytes.len() as isize
        {
            return Err(crate::errno::last());
        }
        Ok(())
    }

    /// Readers behind the oldest record move up to it.
    fn clamp(&mut self) {
        let h = &mut self.header;
        if h.syslog_seq < h.first_seq {
            (h.syslog_seq, h.syslog_off, h.syslog_partial) = (h.first_seq, h.first_off, 0);
        }
        if h.clear_seq < h.first_seq {
            (h.clear_seq, h.clear_off) = (h.first_seq, h.first_off);
        }
    }
}

static RING: OnceLock<Result<Ring, Errno>> = OnceLock::new();

fn with_ring<T>(f: impl FnOnce(&Ring) -> Result<T, Errno>) -> Result<T, Errno> {
    match RING.get_or_init(Ring::open) {
        Ok(ring) => f(ring),
        Err(error) => Err(*error),
    }
}

/// The (dev, ino) of a host path.
fn identity(guest: &str) -> Option<(libc::dev_t, libc::ino_t)> {
    let (host, _) = vfs::lookup(guest);
    let c = std::ffi::CString::new(host.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    (unsafe { libc::stat(c.as_ptr(), &mut st) } == 0).then_some((st.st_dev, st.st_ino))
}

static FILES: OnceLock<[Option<(libc::dev_t, libc::ino_t)>; 2]> = OnceLock::new();

/// Gives `fd` the kernel log's semantics if it is a description of the
/// runtime `/dev/kmsg` or `/proc/kmsg` (at open, and for an fd that
/// arrived from elsewhere). Whether it is.
pub fn adopt(fd: i32) -> bool {
    if !vfs::initialized() {
        return false;
    }
    let files = FILES.get_or_init(|| [identity("/dev/kmsg"), identity("/proc/kmsg")]);
    if files.iter().all(Option::is_none) {
        return false;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat of a guest fd into a local.
    if unsafe { libc::fstat(fd, &mut st) } != 0 || st.st_mode & libc::S_IFMT != libc::S_IFREG {
        return false;
    }
    let id = Some((st.st_dev, st.st_ino));
    let open = if id == files[0] {
        Open::restore(false)
    } else if id == files[1] {
        Open::restore(true)
    } else {
        return false;
    };
    fdtab::insert(fd, Kind::Kmsg(Arc::new(open)));
    true
}

/// open of `/proc/kmsg`: a description of guest-init's marker file.
pub fn open_proc(flags: u64, host_flags: i32) -> i64 {
    if let Err(error) = permitted(ACTION_OPEN) {
        return -(error as i64);
    }
    let (host, _) = vfs::lookup("/proc/kmsg");
    let Ok(c) = std::ffi::CString::new(host.as_os_str().as_encoded_bytes()) else {
        return -(EIO as i64);
    };
    if flags & 3 != 0 {
        return -(crate::errno::EACCES as i64);
    }
    // SAFETY: opening the marker.
    let fd = unsafe { libc::open(c.as_ptr(), host_flags & !(libc::O_CREAT | libc::O_TRUNC)) };
    if fd < 0 {
        return -(crate::errno::last() as i64);
    }
    if !adopt(fd) {
        // SAFETY: the fd just opened.
        unsafe { libc::close(fd) };
        return -(EIO as i64);
    }
    if let Err(error) = fdtab::publish_guest(fd) {
        fdtab::on_close(fd);
        // SAFETY: as above.
        unsafe { libc::close(fd) };
        return -(error as i64);
    }
    fd as i64
}

/// A record the kernel logs itself (`printk`); one that cannot be stored
/// is lost, as a full console loses it.
pub(super) fn printk(facility: u32, level: u32, text: &[u8]) {
    let _ = with_ring(|ring| ring.append(facility, level, text));
}

impl Ratelimit {
    pub const fn new() -> Ratelimit {
        Ratelimit {
            begin: 0,
            printed: 0,
            missed: 0,
        }
    }
}

/// `printk_ratelimited` at a call site whose state is `limit`
/// (`DEFAULT_RATELIMIT_STATE`): ten records per five seconds; the first of
/// the next interval is preceded by how many `site` suppressed.
pub(super) fn printk_ratelimited(limit: &Mutex<Ratelimit>, site: &str, level: u32, text: &[u8]) {
    let now = super::clock::Base::Monotonic.now();
    let mut l = limit.lock().unwrap();
    if l.begin == 0 || now - l.begin >= RATELIMIT_INTERVAL_NS {
        if l.missed > 0 {
            let missed = format!("{site}: {} callbacks suppressed", l.missed);
            printk(0, DEFAULT_LEVEL, missed.as_bytes());
        }
        (l.begin, l.printed, l.missed) = (now, 0, 0);
    }
    if l.printed >= RATELIMIT_BURST {
        l.missed += 1;
        return;
    }
    l.printed += 1;
    drop(l);
    printk(0, level, text);
}

/// `/proc/sys/kernel/<name>` as init wrote it.
fn sysctl(name: &str) -> Option<String> {
    let (host, _) = vfs::lookup(&format!("/proc/sys/kernel/{name}"));
    std::fs::read_to_string(host)
        .ok()
        .map(|v| v.trim().to_string())
}

/// `check_syslog_permissions` of a reader.
fn permitted(action: i32) -> Result<(), Errno> {
    let restricted = sysctl("dmesg_restrict").is_some_and(|v| v != "0")
        || !matches!(action, ACTION_READ_ALL | ACTION_SIZE_BUFFER);
    if !restricted || super::cred::capable(CAP_SYSLOG) || super::cred::capable(CAP_SYS_ADMIN) {
        Ok(())
    } else {
        Err(EPERM)
    }
}

/// `printk_devkmsg`: "on", "off" or "ratelimit" (the default).
fn devkmsg() -> String {
    sysctl("printk_devkmsg").unwrap_or_else(|| "ratelimit".into())
}

/// `devkmsg_open` and `kmsg_open`, for a description `adopt` just typed:
/// their checks, and a `/dev/kmsg` reader placed at the oldest record.
pub fn opened(fd: i32, flags: u64) -> Result<(), Errno> {
    match fdtab::get(fd) {
        Some(Kind::Kmsg(open)) => match *open {
            Open::Dev(_) => {
                if devkmsg() == "off" {
                    return Err(EPERM);
                }
                if flags & 3 != 1 {
                    permitted(ACTION_READ_ALL)?;
                    let first = with_ring(|ring| Ok(ring.lock()?.header.first_off))?;
                    // SAFETY: positioning the new description.
                    unsafe { libc::lseek(fd, first as i64, libc::SEEK_SET) };
                }
                Ok(())
            }
            Open::Proc => permitted(ACTION_OPEN),
        },
        _ => Ok(()),
    }
}

fn gather(iov: &[libc::iovec]) -> Result<Vec<u8>, i32> {
    super::user_memory::gather(iov)
}

fn scatter(iov: &[libc::iovec], bytes: &[u8]) -> Result<usize, i32> {
    super::user_memory::scatter(iov, bytes)
}

/// `devkmsg_write`: the whole write is one record, its `<N>` prefix the
/// level and facility.
pub fn write(open: &Open, iov: &[libc::iovec]) -> i64 {
    let Open::Dev(limit) = open else {
        return -(EINVAL as i64);
    };
    let total: usize = iov.iter().map(|v| v.iov_len).sum();
    if total > RECORD_MAX {
        return -(EINVAL as i64);
    }
    if devkmsg() == "ratelimit" {
        let now = super::clock::Base::Monotonic.now();
        let mut l = limit.lock().unwrap();
        if l.begin == 0 || now - l.begin >= RATELIMIT_INTERVAL_NS {
            (l.begin, l.printed) = (now, 0);
        }
        if l.printed >= RATELIMIT_BURST {
            l.missed += 1;
            return total as i64;
        }
        l.printed += 1;
    }
    let line = user_try!(gather(iov));
    let (facility, level, text) = parse_prefix(&line);
    match with_ring(|ring| ring.append(facility, level, text)) {
        Ok(()) => total as i64,
        Err(error) => -(error as i64),
    }
}

/// The level and facility a `<N>` prefix gives, and the text after it.
fn parse_prefix(line: &[u8]) -> (u32, u32, &[u8]) {
    if line.first() == Some(&b'<') {
        let digits = line[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if line.get(1 + digits) == Some(&b'>') {
            // simple_strtoul: an overflowing number wraps.
            let u = line[1..1 + digits].iter().fold(0u32, |n, &c| {
                n.wrapping_mul(10).wrapping_add((c - b'0') as u32)
            });
            let facility = if u >> 3 != 0 { u >> 3 } else { LOG_USER };
            return (facility, u & 7, &line[2 + digits..]);
        }
    }
    (LOG_USER, DEFAULT_LEVEL, line)
}

/// read/readv: `devkmsg_read` (one record) or `kmsg_read`.
pub fn read(fd: i32, open: &Open, iov: &[libc::iovec]) -> i64 {
    let len: usize = iov.iter().map(|v| v.iov_len).sum();
    let nonblock = fdtab::nonblocking(fd);
    let result = with_ring(|ring| match open {
        Open::Dev(_) => dev_read(ring, fd, iov, len, nonblock),
        Open::Proc => {
            if nonblock
                && ring.lock().map(|mut l| {
                    l.clamp();
                    l.header.syslog_seq == l.header.next_seq
                })?
            {
                return Err(EAGAIN);
            }
            let bytes = syslog_read(ring, len)?;
            user_try!(scatter(iov, &bytes));
            Ok(bytes.len() as i64)
        }
    });
    result.unwrap_or_else(|error| -(error as i64))
}

fn dev_read(
    ring: &Ring,
    fd: i32,
    iov: &[libc::iovec],
    len: usize,
    nonblock: bool,
) -> Result<i64, Errno> {
    loop {
        // SAFETY: the description's own offset.
        let off = unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) } as u64;
        let header = ring.lock()?.header;
        if off < header.first_off {
            // SAFETY: as devkmsg_read moves a reader past what was lost.
            unsafe { libc::lseek(fd, header.first_off as i64, libc::SEEK_SET) };
            return Err(EPIPE);
        }
        if off < header.end {
            let (line, next) = ring.line_at(off)?;
            if line.len() > len {
                return Err(EINVAL);
            }
            user_try!(scatter(iov, &line));
            // SAFETY: the description's own offset.
            unsafe { libc::lseek(fd, next as i64, libc::SEEK_SET) };
            return Ok(line.len() as i64);
        }
        if nonblock {
            return Err(EAGAIN);
        }
        ring.wait(|h| h.end == off)?;
    }
}

/// `devkmsg_llseek`: to the oldest record, the clear position or the end;
/// only with offset 0.
pub fn lseek(fd: i32, open: &Open, offset: i64, whence: i32) -> i64 {
    let Open::Dev(_) = open else {
        return -(ESPIPE as i64);
    };
    if offset != 0 {
        return -(ESPIPE as i64);
    }
    let result = with_ring(|ring| {
        let mut l = ring.lock()?;
        l.clamp();
        let to = match whence {
            libc::SEEK_SET => l.header.first_off,
            // Linux's SEEK_DATA (3): the clear position.
            3 => l.header.clear_off,
            libc::SEEK_END => l.header.end,
            _ => return Err(EINVAL),
        };
        // SAFETY: the description's own offset.
        unsafe { libc::lseek(fd, to as i64, libc::SEEK_SET) };
        Ok(0)
    });
    result.unwrap_or_else(|error| -(error as i64))
}

/// `syslog_print`: unread records from the syslog position, as many as
/// fit, a record cut short continued by the next read. Waits for one.
fn syslog_read(ring: &Ring, len: usize) -> Result<Vec<u8>, Errno> {
    ring.wait(|h| h.syslog_seq.max(h.first_seq) == h.next_seq)?;
    let mut l = ring.lock()?;
    l.clamp();
    let records = ring.records(l.header.syslog_off, l.header.end)?;
    let mut out = Vec::new();
    for (i, (_, record)) in records.iter().enumerate() {
        let text = record.syslog_text();
        let skip = l.header.syslog_partial as usize;
        let n = (text.len() - skip).min(len - out.len());
        out.extend_from_slice(&text[skip..skip + n]);
        if skip + n < text.len() {
            l.header.syslog_partial += n as u64;
            break;
        }
        let next = records.get(i + 1).map_or(l.header.end, |(off, _)| *off);
        (
            l.header.syslog_seq,
            l.header.syslog_off,
            l.header.syslog_partial,
        ) = (record.seq + 1, next, 0);
        if out.len() == len {
            break;
        }
    }
    l.store()?;
    Ok(out)
}

/// `syslog_print_all`: the newest records from the clear position that
/// fit, oldest first; with `clear` the clear position moves past them.
fn syslog_all(ring: &Ring, len: usize, clear: bool) -> Result<Vec<u8>, Errno> {
    let mut l = ring.lock()?;
    l.clamp();
    let records = ring.records(l.header.clear_off, l.header.end)?;
    let texts: Vec<Vec<u8>> = records.iter().map(|(_, r)| r.syslog_text()).collect();
    let mut total: usize = texts.iter().map(Vec::len).sum();
    let mut from = 0;
    while total > len && from < texts.len() {
        total -= texts[from].len();
        from += 1;
    }
    let mut out = Vec::new();
    for text in &texts[from..] {
        if out.len() + text.len() > len {
            break;
        }
        out.extend_from_slice(text);
    }
    if clear {
        (l.header.clear_seq, l.header.clear_off) = (l.header.next_seq, l.header.end);
        l.store()?;
    }
    Ok(out)
}

/// syslog(type, buf, len) (`do_syslog`).
pub fn syslog(a: [u64; 6]) -> i64 {
    let (action, buf, len) = (a[0] as i32, a[1], a[2] as i32);
    if let Err(error) = permitted(action) {
        return -(error as i64);
    }
    let copy_out = |bytes: Vec<u8>| match super::user_memory::write_exact(buf, &bytes) {
        Ok(()) => bytes.len() as i64,
        Err(error) => -(error as i64),
    };
    let result = with_ring(|ring| match action {
        ACTION_CLOSE | ACTION_OPEN => Ok(0),
        ACTION_READ => {
            if buf == 0 || len < 0 {
                return Err(EINVAL);
            }
            if len == 0 {
                return Ok(0);
            }
            Ok(copy_out(syslog_read(ring, len as usize)?))
        }
        ACTION_READ_ALL | ACTION_READ_CLEAR => {
            if buf == 0 || len < 0 {
                return Err(EINVAL);
            }
            if len == 0 {
                return Ok(0);
            }
            Ok(copy_out(syslog_all(
                ring,
                len as usize,
                action == ACTION_READ_CLEAR,
            )?))
        }
        ACTION_CLEAR => syslog_all(ring, 0, true).map(|_| 0),
        ACTION_CONSOLE_OFF | ACTION_CONSOLE_ON | ACTION_CONSOLE_LEVEL => {
            let mut l = ring.lock()?;
            let h = &mut l.header;
            match action {
                ACTION_CONSOLE_OFF => {
                    if h.saved_console == u64::MAX {
                        h.saved_console = h.console;
                    }
                    h.console = CONSOLE_MIN;
                }
                ACTION_CONSOLE_ON => {
                    if h.saved_console != u64::MAX {
                        h.console = h.saved_console;
                        h.saved_console = u64::MAX;
                    }
                }
                _ => {
                    if !(1..=8).contains(&len) {
                        return Err(EINVAL);
                    }
                    h.console = (len as u64).max(CONSOLE_MIN);
                    // Implicitly re-enables logging to the console.
                    h.saved_console = u64::MAX;
                }
            }
            l.store()?;
            Ok(0)
        }
        ACTION_SIZE_UNREAD => {
            let mut l = ring.lock()?;
            l.clamp();
            let unread: usize = ring
                .records(l.header.syslog_off, l.header.end)?
                .iter()
                .map(|(_, r)| r.syslog_text().len())
                .sum();
            Ok((unread - l.header.syslog_partial as usize) as i64)
        }
        ACTION_SIZE_BUFFER => Ok(LOG_BUF_LEN as i64),
        _ => Err(EINVAL),
    });
    result.unwrap_or_else(|error| -(error as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_read_back_as_written() {
        let record = Record {
            prefix: 14,
            seq: 7,
            usec: 1_234_567,
            text: b"a\\b\tc\nd\x7f".to_vec(),
        };
        let line = record.line();
        assert_eq!(line, b"14,7,1234567,-;a\\x5cb\\x09c\\x0ad\\x7f\n");
        assert_eq!(Record::parse(&line), Some(record.clone()));
        assert_eq!(
            record.syslog_text(),
            b"<14>[    1.234567] a\\b\tc\n<14>[    1.234567] d\x7f\n"
        );
    }

    #[test]
    fn prefixes_give_level_and_facility() {
        assert_eq!(parse_prefix(b"<3>boom"), (LOG_USER, 3, &b"boom"[..]));
        assert_eq!(parse_prefix(b"<14>x"), (1, 6, &b"x"[..]));
        assert_eq!(parse_prefix(b"<0>k"), (LOG_USER, 0, &b"k"[..]));
        assert_eq!(parse_prefix(b"<>e"), (LOG_USER, 0, &b"e"[..]));
        assert_eq!(
            parse_prefix(b"<3 x"),
            (LOG_USER, DEFAULT_LEVEL, &b"<3 x"[..])
        );
        assert_eq!(
            parse_prefix(b"plain"),
            (LOG_USER, DEFAULT_LEVEL, &b"plain"[..])
        );
    }

    #[test]
    fn headers_round_trip() {
        let mut h = Header::empty();
        h.next_seq = 3;
        h.end = 900;
        h.saved_console = u64::MAX;
        assert_eq!(Header::decode(&h.encode()), Some(h));
        assert_eq!(Header::decode(&[]), None);
    }

    fn ring(tag: &str) -> Ring {
        let path = std::env::temp_dir().join(format!("aim-kmsg-{tag}-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        Ring { file: file.into() }
    }

    #[test]
    fn the_ring_keeps_the_newest_records() {
        let ring = ring("trim");
        let text = vec![b'x'; 1000];
        for _ in 0..200 {
            ring.append(LOG_USER, 6, &text).unwrap();
        }
        let h = ring.lock().unwrap().header;
        assert_eq!(h.next_seq, 200);
        assert!(h.end - h.first_off <= LOG_BUF_LEN);
        let records = ring.records(h.first_off, h.end).unwrap();
        assert_eq!(records.len() as u64, 200 - h.first_seq);
        assert_eq!(records.last().unwrap().1.seq, 199);
        assert!(h.punched > 0 && h.punched <= h.first_off);
    }

    #[test]
    fn syslog_reads_consume_and_continue_cut_records() {
        let ring = ring("syslog");
        ring.append(LOG_USER, 3, b"first").unwrap();
        ring.append(LOG_USER, 4, b"second\n").unwrap();
        let first = Record::parse(&ring.line_at(HEADER).unwrap().0).unwrap();
        let whole = first.syslog_text();
        let part = syslog_read(&ring, 5).unwrap();
        assert_eq!(part, whole[..5]);
        let rest = syslog_read(&ring, 4096).unwrap();
        assert_eq!([&part[..], &rest[..]].concat()[..whole.len()], whole[..]);
        assert!(rest.ends_with(b"] second\n"));
        let h = ring.lock().unwrap().header;
        assert_eq!((h.syslog_seq, h.syslog_partial), (2, 0));
        // READ_ALL is not destructive and keeps the newest that fit.
        let all = syslog_all(&ring, 4096, false).unwrap();
        assert!(all.starts_with(b"<11>[") && all.ends_with(b"] second\n"));
        let newest = syslog_all(&ring, all.len() - 1, false).unwrap();
        assert!(newest.starts_with(b"<12>["));
        syslog_all(&ring, 0, true).unwrap();
        assert!(syslog_all(&ring, 4096, false).unwrap().is_empty());
    }
}
