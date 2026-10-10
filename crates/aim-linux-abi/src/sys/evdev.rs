//! Evdev character devices, `/dev/input/eventN` (`docs/input.md`).
//!
//! The guest's `/dev/input` is the device directory of the display server
//! (`linux-run --display SOCKET`: `SOCKET.input`,
//! `aim_host_display::input`). Each device there is a listening Unix
//! socket; opening it connects, and the connection is the open file. The
//! fd is that host socket, so poll and epoll see it readable while events
//! wait, and hung up when the device goes away; listing the directory lists
//! the devices, and inotify on it reports hotplug.
//!
//! What Linux's evdev does per open file happens here:
//! - `read` returns whole `struct input_event`s (64-bit time), in the
//!   clock the client chose (`EVIOCSCLOCKID`); `ENODEV` once the device is
//!   gone or revoked.
//! - `write` injects events into the device.
//! - What the device is (name, id, capabilities, axes, keymap) the server
//!   answers as it is now: `EVIOCSABS` and `EVIOCSKEYCODE` change it for
//!   every open file. The key, switch, LED, sound and slot state requests
//!   answer from the state the client has read so far, which is the device
//!   state minus what is still queued: Linux gets the same consistency by
//!   dropping queued events of the type it reports. After `SYN_DROPPED`,
//!   and for the codes an event mask drops, the state is the device's.
//! - Event masks (`EVIOCSMASK`) are the server's: masked events are never
//!   queued, so poll stays exact. A clock change flushes the queue and
//!   queues `SYN_DROPPED` (a [`FLUSH`] record marks the end of what goes).
//! - The open file survives exec: its socket carries a marker (the
//!   `SO_LINGER` time) and its client id, clock and access (the unused
//!   receive timeout; reads never block in `recv`), and [`adopt`] finds the
//!   device from the socket's peer name.
//! - stat shows the node as the character device `13:(64+N)`, `root:input`
//!   0660, and `/sys/dev/char`, `/sys/class/input` and
//!   `/sys/devices/virtual/input` describe it, as EventHub reads them.
//! - A node whose server died without removing it (its directory is not
//!   locked) is removed when a client finds it: opening it, listing the
//!   directory, or reading a device that hung up.

use std::os::fd::{AsFd, IntoRawFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aim_host_display::input::codes::*;
use aim_host_display::input::{
    AbsInfo, Descriptor, FLUSH, Hello, KeyEntry, MASK_SET, Mask, OP_DESCRIBE, OP_FLUSH, OP_GRAB,
    OP_KEYCODE, OP_MASK, OP_OPEN, OP_SET_ABS, Record, State, VERSION, connect_in, is_masked,
    mask_codes, record_bytes, server, test_bit,
};
use aim_host_display::wire;

use super::clock::Base;
use super::dir::{self, Entry};
use super::fdtab::{self, Kind};
use super::procfs::Node;
use crate::errno::{self, EAGAIN, EBADF, EFAULT, EINVAL, ENODEV, ENOENT, ENOSYS};
use crate::vfs::{self, Area, Resolved};

/// `INPUT_MAJOR`, and the first minor of the evdev nodes.
const INPUT_MAJOR: u32 = 13;
const EVDEV_MINOR_BASE: u32 = 64;
/// `AID_INPUT`: `ueventd.rc` gives `/dev/input/*` to root:input 0660.
const AID_INPUT: u32 = 1004;

/// `sizeof(struct input_event)` on arm64.
const EVENT: usize = 24;

const O_ACCMODE: u64 = 0o3;
const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;

// Linux clock ids EVIOCSCLOCKID accepts.
const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_BOOTTIME: i32 = 7;

/// The `SO_LINGER` time marking an evdev socket (lingering stays off).
pub const MARK: i32 = 0x4556;

/// One open file of a device.
pub struct Evdev {
    /// As the device was at open; what changes is asked of the server.
    desc: Descriptor,
    /// The device directory on the host, for control connections.
    dir: PathBuf,
    writable: bool,
    client: Mutex<Client>,
}

struct Client {
    state: State,
    clock: i32,
    revoked: bool,
    /// `SYN_DROPPED` was read: the state is the device's again once asked.
    stale: bool,
    /// Event masks set through this file, by type; None: unknown (after
    /// exec), ask the server.
    masks: Option<Vec<Option<Mask>>>,
}

fn neg(e: errno::Errno) -> i64 {
    -(e as i64)
}

/// The device index of a node name, `eventN`.
fn index_of(name: &str) -> Option<u32> {
    name.strip_prefix("event")?.parse().ok()
}

/// The indices of the devices present, sorted.
fn present() -> Vec<u32> {
    let Some(dir) = vfs::input_dir() else {
        return Vec::new();
    };
    let mut v: Vec<u32> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| index_of(e.file_name().to_str()?))
        .collect();
    v.sort();
    v
}

impl Evdev {
    pub fn path(&self) -> String {
        format!("/dev/input/event{}", self.desc.index)
    }

    /// `read(2)`: whole events, as many as fit.
    fn read(&self, fd: i32, buf: u64, len: usize) -> i64 {
        if len != 0 && len < EVENT {
            return neg(EINVAL);
        }
        let want = (len / EVENT).clamp(1, 256);
        let mut recs = vec![Record::default(); want];
        loop {
            let mut c = self.client.lock().unwrap();
            if c.revoked {
                return neg(ENODEV);
            }
            let bytes = if len == 0 {
                1
            } else {
                want * size_of::<Record>()
            };
            let flags = libc::MSG_DONTWAIT | if len == 0 { libc::MSG_PEEK } else { 0 };
            // SAFETY: receives into our record buffer.
            let n = unsafe { libc::recv(fd, recs.as_mut_ptr().cast(), bytes, flags) };
            if n == 0 {
                self.gone();
                return neg(ENODEV);
            }
            if n < 0 {
                let e = errno::last();
                if e != EAGAIN {
                    return neg(e);
                }
                drop(c);
                if fdtab::nonblocking(fd) {
                    return neg(EAGAIN);
                }
                let r = fdtab::wait_for(fd, libc::POLLIN);
                if r < 0 {
                    return r;
                }
                continue;
            }
            if len == 0 {
                return 0;
            }
            let Some(n) = complete(fd, &mut recs, n as usize) else {
                return neg(ENODEV);
            };
            let offset = clock_offset(c.clock);
            let mut out = 0;
            for r in &recs[..n] {
                // A flush the requester did not drain: nothing to deliver.
                if r.kind == FLUSH {
                    continue;
                }
                self.desc.apply(&mut c.state, r);
                c.stale |= r.kind == EV_SYN && r.code == SYN_DROPPED;
                let t = r.time_ns + offset;
                let ev = [
                    t.div_euclid(1_000_000_000),
                    t.rem_euclid(1_000_000_000) / 1000,
                    (r.kind as i64) | ((r.code as i64) << 16) | ((r.value as i64) << 32),
                ];
                // SAFETY: guest buffer of `len` bytes, room for `want`.
                unsafe { ((buf as *mut [i64; 3]).add(out)).write_unaligned(ev) };
                out += 1;
            }
            if out == 0 {
                continue;
            }
            return (out * EVENT) as i64;
        }
    }

    /// `write(2)`: whole events, injected into the device.
    fn write(&self, fd: i32, buf: u64, len: usize) -> i64 {
        if !self.writable {
            return neg(EBADF);
        }
        if len != 0 && len < EVENT {
            return neg(EINVAL);
        }
        if self.client.lock().unwrap().revoked {
            return neg(ENODEV);
        }
        let recs: Vec<Record> = (0..len / EVENT)
            .map(|i| {
                // SAFETY: guest buffer of `len` bytes.
                let ev = unsafe { ((buf as *const [i64; 3]).add(i)).read_unaligned() };
                Record {
                    time_ns: 0,
                    kind: ev[2] as u16,
                    code: (ev[2] >> 16) as u16,
                    value: (ev[2] >> 32) as i32,
                }
            })
            // Not events: the server's own records.
            .filter(|r| (r.kind as usize) < EV_CNT)
            .collect();
        // SAFETY: the fd is our socket for the duration of the call.
        let sock = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        match wire::send(sock, record_bytes(&recs), None) {
            Ok(()) => (len / EVENT * EVENT) as i64,
            Err(_) => neg(ENODEV),
        }
    }

    /// A control connection to the device with request `op`, `arg`, and
    /// `payload` after it.
    fn control(&self, op: u32, arg: u64, payload: &[u8]) -> Option<std::os::unix::net::UnixStream> {
        let s = connect_in(&self.dir, &format!("event{}", self.desc.index)).ok()?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let hello = Hello {
            version: VERSION,
            op,
            client: self.desc.client,
            arg,
        };
        wire::send(s.as_fd(), wire::bytes(&hello), None).ok()?;
        if !payload.is_empty() {
            wire::send(s.as_fd(), payload, None).ok()?;
        }
        Some(s)
    }

    /// A request answered with an `i32` and, when it is 0, a `T`.
    fn request<T: Copy + Default>(&self, op: u32, arg: u64, payload: &[u8]) -> Result<T, i64> {
        let s = self.control(op, arg, payload).ok_or(neg(ENODEV))?;
        let mut r = [0u8; 4];
        std::io::Read::read_exact(&mut &s, &mut r).map_err(|_| neg(ENODEV))?;
        match i32::from_ne_bytes(r) {
            0 => wire::recv_record::<T>(s.as_fd())
                .ok()
                .flatten()
                .ok_or(neg(ENODEV)),
            e => Err(e as i64),
        }
    }

    /// A request answered with an `i32` alone.
    fn command(&self, op: u32, arg: u64, payload: &[u8]) -> i64 {
        let Some(s) = self.control(op, arg, payload) else {
            return neg(ENODEV);
        };
        let mut r = [0u8; 4];
        match std::io::Read::read_exact(&mut &s, &mut r) {
            Ok(()) => i32::from_ne_bytes(r) as i64,
            Err(_) => neg(ENODEV),
        }
    }

    /// The device as it is now, its state the device's.
    fn live(&self) -> Result<Descriptor, i64> {
        describe(&self.dir, self.desc.index).ok_or(neg(ENODEV))
    }

    /// The masks of event type `kind` and of the types (0), as this file
    /// set them.
    fn masks(&self, c: &mut Client, kind: u16) -> Result<[Option<Mask>; 2], i64> {
        if c.masks.is_none() {
            let mut all = vec![None; EV_CNT];
            for k in 0..EV_CNT as u16 {
                if mask_codes(k as u32) > 0 {
                    let m: Mask = self.request(OP_MASK, k as u64, &[])?;
                    all[k as usize] = (m != Mask([0xff; 96])).then_some(m);
                }
            }
            c.masks = Some(all);
        }
        let m = c.masks.as_ref().unwrap();
        Ok([m[0], m.get(kind as usize).copied().flatten()])
    }

    /// The state the state requests of type `kind` answer from: what the
    /// client has read, the device's after `SYN_DROPPED`, and the device's
    /// for the codes its masks drop.
    fn state(&self, c: &mut Client, kind: u16) -> Result<State, i64> {
        if c.stale {
            c.state = self.live()?.state;
            c.stale = false;
        }
        let [types, codes] = self.masks(c, kind)?;
        if types.is_none() && codes.is_none() {
            return Ok(c.state);
        }
        let live = self.live()?.state;
        let mut masks = vec![None; EV_CNT];
        masks[0] = types;
        masks[kind as usize] = codes;
        let mut st = c.state;
        match kind {
            EV_ABS => {
                for code in 0..ABS_CNT as u16 {
                    if is_masked(&masks, EV_ABS, code) {
                        st.abs[code as usize] = live.abs[code as usize];
                        if is_mt_axis(code) {
                            let i = (code - ABS_MT_TOUCH_MAJOR) as usize;
                            for (slot, l) in st.mt.iter_mut().zip(&live.mt) {
                                slot[i] = l[i];
                            }
                        }
                    }
                }
            }
            _ => {
                let (Some(mine), Some(theirs)) = (st.bits_mut(kind), live.bits(kind)) else {
                    return Ok(st);
                };
                for code in 0..=max_code(kind).unwrap_or(0) {
                    if is_masked(&masks, kind, code) {
                        let (byte, bit) = (code as usize / 8, 1 << (code % 8));
                        mine[byte] = (mine[byte] & !bit) | (theirs[byte] & bit);
                    }
                }
            }
        }
        Ok(st)
    }

    /// `EVIOCSCLOCKID`: a new clock flushes what is queued, which ends with
    /// `SYN_DROPPED` (`evdev_set_clk_type`).
    fn set_clock(&self, fd: i32, c: &mut Client, clock: i32) -> i64 {
        if c.clock == clock {
            return 0;
        }
        c.clock = clock;
        mark(fd, &self.desc, c.clock, self.writable);
        let mut b = 0u8;
        // SAFETY: peeks one byte into a local.
        let queued = unsafe {
            libc::recv(
                fd,
                (&mut b as *mut u8).cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        } > 0;
        if !queued {
            return 0;
        }
        let r = self.command(OP_FLUSH, 0, &[]);
        if r < 0 {
            return r;
        }
        // Drop everything up to the server's FLUSH record; the events still
        // change the state, as they did the device's.
        let mut rec = [Record::default(); 1];
        loop {
            // SAFETY: one record into a local.
            let n = unsafe {
                libc::recv(
                    fd,
                    rec.as_mut_ptr().cast(),
                    size_of::<Record>(),
                    libc::MSG_DONTWAIT,
                )
            };
            let n = match n {
                n if n > 0 => n as usize,
                0 => return neg(ENODEV),
                _ if errno::last() == EAGAIN => {
                    fdtab::wait_for(fd, libc::POLLIN);
                    continue;
                }
                _ => return neg(errno::last()),
            };
            if complete(fd, &mut rec, n).is_none() {
                return neg(ENODEV);
            }
            if rec[0].kind == FLUSH {
                return 0;
            }
            self.desc.apply(&mut c.state, &rec[0]);
        }
    }

    /// The device hung up: if its server died, its nodes go too.
    fn gone(&self) {
        server::remove_stale(&self.dir);
    }
}

/// Complete the records of which `n` bytes arrived in `recs`: the server
/// writes whole packets, so the rest of a cut record is on its way.
/// Returns the number of records.
fn complete(fd: i32, recs: &mut [Record], mut n: usize) -> Option<usize> {
    while n % size_of::<Record>() != 0 {
        let rest = size_of::<Record>() - n % size_of::<Record>();
        // SAFETY: completes the record inside the buffer.
        let m = unsafe {
            libc::recv(
                fd,
                recs.as_mut_ptr().cast::<u8>().add(n).cast(),
                rest,
                libc::MSG_DONTWAIT,
            )
        };
        match m {
            0 => return None,
            m if m > 0 => n += m as usize,
            _ if errno::last() == EAGAIN => {
                fdtab::wait_for(fd, libc::POLLIN);
            }
            _ => return None,
        }
    }
    Some(n / size_of::<Record>())
}

/// Mark `fd` as an open file of the device `desc` for [`adopt`]: the
/// `SO_LINGER` marker, and the client id, clock and access in the receive
/// timeout.
fn mark(fd: i32, desc: &Descriptor, clock: i32, writable: bool) {
    let l = libc::linger {
        l_onoff: 0,
        l_linger: MARK,
    };
    let tv = libc::timeval {
        tv_sec: desc.client as libc::time_t,
        tv_usec: (clock + 1) | (writable as i32) << 4,
    };
    // SAFETY: socket options on our socket from locals.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&l as *const libc::linger).cast(),
            size_of::<libc::linger>() as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&tv as *const libc::timeval).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// An evdev socket that arrived across exec (or `SCM_RIGHTS`): its open
/// file again, the device as it is now.
pub fn adopt(fd: i32) {
    let Some(dir) = vfs::input_dir() else { return };
    let mut sa: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let mut len = size_of::<libc::sockaddr_un>() as libc::socklen_t;
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut tl = size_of::<libc::timeval>() as libc::socklen_t;
    // SAFETY: the peer's name and a socket option into locals.
    let ok = unsafe {
        libc::getpeername(fd, (&mut sa as *mut libc::sockaddr_un).cast(), &mut len) == 0
            && libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                (&mut tv as *mut libc::timeval).cast(),
                &mut tl,
            ) == 0
    };
    let name: Vec<u8> = sa
        .sun_path
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    let Some(index) = std::str::from_utf8(&name)
        .ok()
        .and_then(|n| n.rsplit('/').next())
        .and_then(index_of)
    else {
        return;
    };
    if !ok {
        return;
    }
    let Some(live) = describe(dir, index) else {
        return;
    };
    let e = Evdev {
        desc: Descriptor {
            client: tv.tv_sec as u64,
            ..live
        },
        dir: dir.to_owned(),
        writable: tv.tv_usec >> 4 & 1 != 0,
        client: Mutex::new(Client {
            state: live.state,
            clock: (tv.tv_usec & 0xf) - 1,
            revoked: false,
            stale: false,
            masks: None,
        }),
    };
    fdtab::insert(fd, Kind::Evdev(Arc::new(e)));
}

/// Device `index` in `dir` as it is now, its state the device's.
fn describe(dir: &std::path::Path, index: u32) -> Option<Descriptor> {
    let s = connect_in(dir, &format!("event{index}")).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let hello = Hello {
        version: VERSION,
        op: OP_DESCRIBE,
        ..Default::default()
    };
    wire::send(s.as_fd(), wire::bytes(&hello), None).ok()?;
    wire::recv_record::<Descriptor>(s.as_fd()).ok().flatten()
}

/// Nanoseconds to add to a `CLOCK_MONOTONIC` time for `clock`.
fn clock_offset(clock: i32) -> i64 {
    let base = match clock {
        CLOCK_REALTIME => Base::Realtime,
        CLOCK_BOOTTIME => Base::Boottime,
        _ => return 0,
    };
    base.now() as i64 - Base::Monotonic.now() as i64
}

fn evdev_of(fd: i32) -> Option<Arc<Evdev>> {
    match fdtab::get(fd)? {
        Kind::Evdev(e) => Some(e),
        _ => None,
    }
}

/// `openat` of a device node; None for any other path.
pub fn open(r: &Resolved, flags: u64) -> Option<i64> {
    if r.area != Area::Input {
        return None;
    }
    let name = r.guest.rsplit('/').next()?;
    let index = index_of(name)?;
    let dir = PathBuf::from(r.host.to_str().ok()?).parent()?.to_owned();
    let stream = match connect_in(&dir, name) {
        Ok(s) => s,
        // A node whose server has gone is a device that no longer exists,
        // and goes.
        Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => {
            server::remove_stale(&dir);
            return Some(neg(ENODEV));
        }
        Err(e) => {
            return Some(neg(errno::from_darwin(
                e.raw_os_error().unwrap_or(libc::EIO),
            )));
        }
    };
    let on: libc::c_int = 1;
    // SAFETY: setsockopt on our socket with a local int.
    unsafe {
        libc::setsockopt(
            std::os::fd::AsRawFd::as_raw_fd(&stream),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&on as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    let hello = Hello {
        version: VERSION,
        op: OP_OPEN,
        ..Default::default()
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let desc = match wire::send(stream.as_fd(), wire::bytes(&hello), None)
        .and_then(|()| wire::recv_record::<Descriptor>(stream.as_fd()))
    {
        Ok(Some(d)) if d.index == index => d,
        _ => return Some(neg(ENODEV)),
    };
    let fd = stream.into_raw_fd();
    let writable = flags & O_ACCMODE != 0;
    mark(fd, &desc, CLOCK_REALTIME, writable);
    // SAFETY: fcntl on the fd we just made (std opens it close-on-exec).
    unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    fdtab::set_flags(fd, flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0);
    fdtab::insert(
        fd,
        Kind::Evdev(Arc::new(Evdev {
            desc,
            dir,
            writable,
            client: Mutex::new(Client {
                state: desc.state,
                clock: CLOCK_REALTIME,
                revoked: false,
                stale: false,
                masks: Some(vec![None; EV_CNT]),
            }),
        })),
    );
    if let Err(error) = fdtab::publish_guest(fd) {
        fdtab::on_close(fd); unsafe { libc::close(fd); }
        return Some(-(error as i64));
    }
    Some(fd as i64)
}

/// read(2) on an evdev fd; None when `fd` is not one.
pub fn read(fd: i32, buf: u64, len: usize) -> Option<i64> {
    Some(evdev_of(fd)?.read(fd, buf, len))
}

/// write(2) on an evdev fd; None when `fd` is not one.
pub fn write(fd: i32, buf: u64, len: usize) -> Option<i64> {
    Some(evdev_of(fd)?.write(fd, buf, len))
}

/// `bits_to_user`: at most `size` bytes of the bitmap up to `max`; returns
/// the bytes copied.
fn put_bits(bits: &[u8], max: u16, size: usize, arg: u64) -> i64 {
    let n = bitmap_bytes(max).min(bits.len()).min(size);
    // SAFETY: guest buffer of `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bits.as_ptr(), arg as *mut u8, n) };
    n as i64
}

fn put<T: Copy>(arg: u64, v: T) -> i64 {
    if arg == 0 {
        return neg(EFAULT);
    }
    // SAFETY: guest buffer the ioctl names.
    unsafe { (arg as *mut T).write_unaligned(v) };
    0
}

// ioctl encoding (asm-generic).
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

const EVIOCGVERSION: u64 = 0x8004_4501;
const EVIOCGID: u64 = 0x8008_4502;
const EVIOCGREP: u64 = 0x8008_4503;
const EVIOCSREP: u64 = 0x4008_4503;
const EVIOCGKEYCODE: u64 = 0x8008_4504;
const EVIOCGKEYCODE_V2: u64 = 0x8028_4504;
const EVIOCSKEYCODE: u64 = 0x4008_4504;
const EVIOCSKEYCODE_V2: u64 = 0x4028_4504;
const EVIOCGEFFECTS: u64 = 0x8004_4584;
const EVIOCRMFF: u64 = 0x4004_4581;
const EVIOCGRAB: u64 = 0x4004_4590;
const EVIOCREVOKE: u64 = 0x4004_4591;
const EVIOCGMASK: u64 = 0x8010_4592;
const EVIOCSMASK: u64 = 0x4010_4593;
const EVIOCSCLOCKID: u64 = 0x4004_45a0;

/// `struct input_keymap_entry`'s `INPUT_KEYMAP_BY_INDEX`.
const KEYMAP_BY_INDEX: u8 = 1;
/// `sizeof(struct input_keymap_entry)`.
const KEYMAP_ENTRY: usize = 40;

/// A `struct input_keymap_entry` at `arg` as a request
/// (`input_scancode_to_scalar`: a scan code is 1, 2 or 4 bytes).
fn key_entry(arg: u64) -> Result<KeyEntry, i64> {
    // SAFETY: guest struct of KEYMAP_ENTRY bytes.
    let ke = unsafe { (arg as *const [u8; KEYMAP_ENTRY]).read_unaligned() };
    let (flags, len) = (ke[0], ke[1] as usize);
    if len > 32 {
        return Err(neg(EINVAL));
    }
    let mut e = KeyEntry {
        keycode: u32::from_ne_bytes(ke[4..8].try_into().unwrap()),
        ..Default::default()
    };
    if flags & KEYMAP_BY_INDEX != 0 {
        e.by_index = 1;
        e.index = u16::from_ne_bytes([ke[2], ke[3]]) as u32;
        return Ok(e);
    }
    let sc = &ke[8..8 + len];
    e.scancode = match len {
        1 => sc[0] as u32,
        2 => u16::from_ne_bytes([sc[0], sc[1]]) as u32,
        4 => u32::from_ne_bytes(sc.try_into().unwrap()),
        _ => return Err(neg(EINVAL)),
    };
    Ok(e)
}

/// `struct input_mask` at `arg`: type, size and pointer of the codes.
fn input_mask(arg: u64) -> (u32, usize, u64) {
    // SAFETY: guest struct of 16 bytes.
    let m = unsafe { (arg as *const [u32; 4]).read_unaligned() };
    (m[0], m[1] as usize, m[2] as u64 | (m[3] as u64) << 32)
}

/// An evdev ioctl (`'E'`); None when `fd` is not an evdev fd or the
/// request is not one (the generic ones, `FIONBIO` and the like, apply).
pub fn ioctl(fd: i32, cmd: u64, arg: u64) -> Option<i64> {
    if (cmd >> 8) & 0xff != u64::from(b'E') {
        return None;
    }
    let e = evdev_of(fd)?;
    let mut c = e.client.lock().unwrap();
    if c.revoked {
        return Some(neg(ENODEV));
    }
    let d = &e.desc;
    let (dir, size, nr) = (
        cmd >> 30,
        ((cmd >> 16) & 0x3fff) as usize,
        (cmd & 0xff) as u16,
    );
    // Every request but these takes a pointer; a zero-length copy needs
    // none (getevent sizes a bitmap with EVIOCGBIT(ev, 0) on NULL).
    if arg == 0 && size != 0 && ![EVIOCGRAB, EVIOCREVOKE, EVIOCRMFF].contains(&cmd) {
        return Some(neg(EFAULT));
    }
    let r = match cmd {
        EVIOCGVERSION => put(arg, EV_VERSION),
        EVIOCGID => put(arg, d.id),
        // No device repeats keys (no EV_REP): Android does.
        EVIOCGREP | EVIOCSREP => neg(ENOSYS),
        EVIOCGKEYCODE | EVIOCSKEYCODE => {
            // SAFETY: guest unsigned int[2].
            let v = unsafe { (arg as *const [u32; 2]).read_unaligned() };
            let set = cmd == EVIOCSKEYCODE;
            let req = KeyEntry {
                scancode: v[0],
                keycode: v[1],
                ..Default::default()
            };
            match e.request::<KeyEntry>(OP_KEYCODE, set as u64, wire::bytes(&req)) {
                Ok(got) if !set => put(arg + 4, got.keycode),
                Ok(_) => 0,
                Err(r) => r,
            }
        }
        EVIOCGKEYCODE_V2 | EVIOCSKEYCODE_V2 => {
            let set = cmd == EVIOCSKEYCODE_V2;
            match key_entry(arg)
                .and_then(|req| e.request::<KeyEntry>(OP_KEYCODE, set as u64, wire::bytes(&req)))
            {
                Ok(got) if !set => {
                    // SAFETY: the guest struct read above.
                    unsafe {
                        let p = arg as *mut u8;
                        p.add(1).write(4);
                        (p.add(2) as *mut u16).write_unaligned(got.index as u16);
                        (p.add(4) as *mut u32).write_unaligned(got.keycode);
                        (p.add(8) as *mut u32).write_unaligned(got.scancode);
                    }
                    0
                }
                Ok(_) => 0,
                Err(r) => r,
            }
        }
        EVIOCGEFFECTS => put(arg, 0i32),
        EVIOCRMFF => neg(ENOSYS),
        EVIOCGRAB => {
            drop(c);
            return Some(e.command(OP_GRAB, (arg != 0) as u64, &[]));
        }
        EVIOCREVOKE if arg != 0 => neg(EINVAL),
        EVIOCREVOKE => {
            // The server sees the connection end, which also ends a grab;
            // poll then reports the fd hung up, as Linux does.
            c.revoked = true;
            // SAFETY: shutting down our own socket.
            unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
            0
        }
        EVIOCGMASK => {
            let (kind, codes_size, codes) = input_mask(arg);
            let n = mask_codes(kind);
            let xfer = n.div_ceil(64) * 8;
            let xfer = xfer.min(codes_size);
            if codes == 0 && codes_size != 0 {
                return Some(neg(EFAULT));
            }
            let mask = if n == 0 {
                Ok(Mask::default())
            } else {
                e.request::<Mask>(OP_MASK, kind as u64, &[])
            };
            match mask {
                Ok(m) => {
                    // SAFETY: guest buffer of `codes_size` bytes.
                    unsafe {
                        std::ptr::copy_nonoverlapping(m.0.as_ptr(), codes as *mut u8, xfer);
                        std::ptr::write_bytes((codes as *mut u8).add(xfer), 0, codes_size - xfer);
                    }
                    0
                }
                Err(r) => r,
            }
        }
        EVIOCSMASK => {
            let (kind, codes_size, codes) = input_mask(arg);
            let n = mask_codes(kind);
            if n == 0 {
                return Some(0);
            }
            if codes == 0 && codes_size != 0 {
                return Some(neg(EFAULT));
            }
            let mut m = Mask::default();
            let len = (n.div_ceil(64) * 8).min(codes_size);
            // SAFETY: guest buffer of `codes_size` bytes.
            unsafe { std::ptr::copy_nonoverlapping(codes as *const u8, m.0.as_mut_ptr(), len) };
            match e.request::<Mask>(OP_MASK, MASK_SET | kind as u64, wire::bytes(&m)) {
                Ok(_) => {
                    if let Some(masks) = &mut c.masks {
                        masks[kind as usize] = Some(m);
                    }
                    0
                }
                Err(r) => r,
            }
        }
        EVIOCSCLOCKID => {
            // SAFETY: guest int.
            let id = unsafe { (arg as *const i32).read_unaligned() };
            if ![CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_BOOTTIME].contains(&id) {
                return Some(neg(EINVAL));
            }
            e.set_clock(fd, &mut c, id)
        }
        _ if dir == IOC_WRITE && nr == 0x80 => neg(ENOSYS), // EVIOCSFF
        _ if dir == IOC_WRITE && (0xc0..=0xff).contains(&nr) => {
            // EVIOCSABS: the axis for every open file; a short struct has
            // no resolution.
            if !test_bit(&d.bits.ev, EV_ABS) {
                return Some(neg(EINVAL));
            }
            let mut info = AbsInfo::default();
            let n = size.min(size_of::<AbsInfo>());
            // SAFETY: guest buffer of `size` bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    arg as *const u8,
                    (&mut info as *mut AbsInfo).cast::<u8>(),
                    n,
                )
            };
            e.command(OP_SET_ABS, (nr & ABS_MAX) as u64, wire::bytes(&info))
        }
        _ if dir != IOC_READ => neg(EINVAL),
        _ => match nr {
            0x06 => {
                // EVIOCGNAME: the string and its NUL, cut to `size`.
                let name = d.name();
                let n = (name.len() + 1).min(size);
                let mut v = name.to_vec();
                v.push(0);
                // SAFETY: guest buffer of `size` bytes.
                unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), arg as *mut u8, n) };
                n as i64
            }
            // EVIOCGPHYS, EVIOCGUNIQ: virtual devices have neither.
            0x07 | 0x08 => neg(ENOENT),
            0x09 => put_bits(&d.props, INPUT_PROP_MAX, size, arg),
            0x0a => match e.state(&mut c, EV_ABS) {
                Ok(st) => mt_slots(d, &st, size, arg),
                Err(r) => r,
            },
            0x18..=0x1b => {
                let ev = [EV_KEY, EV_LED, EV_SND, EV_SW][(nr - 0x18) as usize];
                match e.state(&mut c, ev) {
                    Ok(st) => put_bits(
                        st.bits(ev).unwrap_or(&[]),
                        max_code(ev).unwrap_or(0),
                        size,
                        arg,
                    ),
                    Err(r) => r,
                }
            }
            // The keys change with the keymap: the device's now.
            0x21 => match e.live() {
                Ok(live) => put_bits(&live.bits.key, KEY_MAX, size, arg),
                Err(r) => r,
            },
            0x20..=0x3f => {
                let ev = nr & EV_MAX;
                match (d.bits.of(ev), max_code(ev)) {
                    (Some(bits), Some(max)) => put_bits(bits, max, size, arg),
                    _ => neg(EINVAL),
                }
            }
            0x40..=0x7f if !test_bit(&d.bits.ev, EV_ABS) => neg(EINVAL),
            0x40..=0x7f => {
                // The axis as it is now (`EVIOCSABS` changes it), its value
                // the device's (Linux does not flush for it).
                let axis = (nr & ABS_MAX) as usize;
                match e.live() {
                    Ok(live) => {
                        let info = AbsInfo {
                            value: live.state.abs[axis],
                            ..live.absinfo[axis]
                        };
                        let n = size.min(size_of_val(&info));
                        // SAFETY: guest buffer of `size` bytes.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                (&info as *const AbsInfo).cast::<u8>(),
                                arg as *mut u8,
                                n,
                            )
                        };
                        0
                    }
                    Err(r) => r,
                }
            }
            _ => neg(EINVAL),
        },
    };
    Some(r)
}

/// `EVIOCGMTSLOTS`: `{ u32 code; i32 values[] }`, one value per slot.
fn mt_slots(d: &Descriptor, state: &State, size: usize, arg: u64) -> i64 {
    // SAFETY: guest u32.
    let code = unsafe { (arg as *const u32).read_unaligned() };
    if d.mt_slots == 0 || code > u16::MAX as u32 || !is_mt_axis(code as u16) {
        return neg(EINVAL);
    }
    let room = size.saturating_sub(4) / 4;
    for slot in 0..(d.mt_slots as usize).min(room) {
        let v = state.mt_value(slot, code as u16);
        // SAFETY: guest buffer of `size` bytes, room for `room` values.
        unsafe { ((arg + 4) as *mut i32).add(slot).write_unaligned(v) };
    }
    0
}

/// The node's stat: the character device `13:(64+N)`, root:input 0660.
fn as_device(st: &mut libc::stat, index: u32) {
    st.st_mode = libc::S_IFCHR | 0o660;
    // Darwin's dev_t encoding (major in the top byte); stat converts it.
    st.st_rdev = ((INPUT_MAJOR << 24) | (EVDEV_MINOR_BASE + index)) as i32;
    st.st_uid = 0;
    st.st_gid = AID_INPUT;
    st.st_size = 0;
}

/// stat of a path under `/dev/input`: a device node is shown as one.
pub fn stat(r: &Resolved, st: &mut libc::stat) {
    if r.area != Area::Input || st.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return;
    }
    if let Some(index) = r.guest.rsplit('/').next().and_then(index_of) {
        as_device(st, index);
    }
}

/// fstat of an evdev fd; None when `fd` is not one.
pub fn fstat(fd: i32, st: &mut libc::stat) -> Option<()> {
    let e = evdev_of(fd)?;
    as_device(st, e.desc.index);
    Some(())
}

/// Whether directory fd `fd` is the device directory (its sockets list as
/// character devices).
pub fn is_device_dir(fd: i32) -> bool {
    let Some(dir) = vfs::input_dir() else {
        return false;
    };
    let (mut a, mut b): (libc::stat, libc::stat) = unsafe { std::mem::zeroed() };
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // SAFETY: stats into local buffers.
    let both = unsafe { libc::fstat(fd, &mut a) == 0 && libc::stat(c.as_ptr(), &mut b) == 0 };
    both && (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
}

/// Directory entries of the device directory: sockets are devices, and
/// the nodes of a server that died are gone.
pub fn fix_types(entries: &mut Vec<Entry>) {
    const DT_SOCK: u8 = 12;
    const DT_CHR: u8 = 2;
    if vfs::input_dir().is_some_and(server::remove_stale) {
        entries.retain(|e| !e.name.starts_with(b"event"));
    }
    for e in entries {
        if e.ty == DT_SOCK {
            e.ty = DT_CHR;
        }
    }
}

/// The sysfs view of the devices, under `/sys` (`rest` is the path after
/// it): `/sys/dev/char/13:M` and `/sys/class/input/*` link to
/// `/sys/devices/virtual/input/inputN/eventN`. Links are absolute here,
/// where Linux's are relative; they resolve the same.
pub fn sys_node(rest: &str) -> Option<Node> {
    let devs = present();
    let dirs = |names: Vec<String>, ty| {
        Node::Dir(names.into_iter().map(|n| Entry::new(1, ty, n)).collect())
    };
    let input = |i: u32| format!("/sys/devices/virtual/input/input{i}");
    let minor = |i: u32| EVDEV_MINOR_BASE + i;
    Some(match rest {
        "dev" => dirs(vec!["char".into()], dir::DT_DIR),
        "dev/char" => dirs(
            devs.iter()
                .map(|&i| format!("{INPUT_MAJOR}:{}", minor(i)))
                .collect(),
            dir::DT_LNK,
        ),
        "class" => dirs(vec!["input".into()], dir::DT_DIR),
        "class/input" => dirs(
            devs.iter()
                .flat_map(|&i| [format!("event{i}"), format!("input{i}")])
                .collect(),
            dir::DT_LNK,
        ),
        "devices/virtual" => dirs(vec!["input".into()], dir::DT_DIR),
        "devices/virtual/input" => dirs(
            devs.iter().map(|&i| format!("input{i}")).collect(),
            dir::DT_DIR,
        ),
        _ => {
            if let Some(m) = rest.strip_prefix("dev/char/13:") {
                let i = m.parse::<u32>().ok()?.checked_sub(EVDEV_MINOR_BASE)?;
                return devs
                    .contains(&i)
                    .then(|| Node::Link(format!("{}/event{i}", input(i))));
            }
            if let Some(n) = rest.strip_prefix("class/input/") {
                let (i, dev) = match n.strip_prefix("event") {
                    Some(i) => (i, true),
                    None => (n.strip_prefix("input")?, false),
                };
                let i = i.parse::<u32>().ok().filter(|i| devs.contains(i))?;
                return Some(Node::Link(if dev {
                    format!("{}/event{i}", input(i))
                } else {
                    input(i)
                }));
            }
            let n = rest.strip_prefix("devices/virtual/input/input")?;
            let (i, tail) = n.split_once('/').unwrap_or((n, ""));
            let i = i.parse::<u32>().ok().filter(|i| devs.contains(i))?;
            match tail {
                "" => dirs(vec![format!("event{i}")], dir::DT_DIR),
                t if t == format!("event{i}") => {
                    dirs(vec!["dev".into(), "uevent".into()], dir::DT_REG)
                }
                t if t == format!("event{i}/dev") => {
                    Node::File(format!("{INPUT_MAJOR}:{}\n", minor(i)).into_bytes())
                }
                t if t == format!("event{i}/uevent") => {
                    let devpath = format!("/{}", rest.strip_suffix("/uevent")?);
                    Node::File(uevent_device(&devpath)?.attribute())
                }
                _ => return None,
            }
        }
    })
}

/// The uevent description of a present evdev node from its sysfs path
/// below `/sys` (`/devices/virtual/input/inputN/eventN`).
pub fn uevent_device(devpath: &str) -> Option<super::uevent::Device> {
    let n = devpath.strip_prefix("/devices/virtual/input/input")?;
    let (i, event) = n.split_once('/')?;
    let i = i.parse::<u32>().ok().filter(|i| present().contains(i))?;
    (event == format!("event{i}")).then(|| super::uevent::Device {
        devpath: devpath.to_string(),
        subsystem: "input",
        env: vec![
            format!("MAJOR={INPUT_MAJOR}"),
            format!("MINOR={}", EVDEV_MINOR_BASE + i),
            format!("DEVNAME=input/event{i}"),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_are_linux() {
        // _IOC(dir, 'E', nr, size) as <linux/input.h> computes them.
        let ioc = |dir: u64, nr: u64, size: u64| (dir << 30) | (size << 16) | (0x45 << 8) | nr;
        assert_eq!(EVIOCGVERSION, ioc(IOC_READ, 0x01, 4));
        assert_eq!(EVIOCGID, ioc(IOC_READ, 0x02, 8));
        assert_eq!(EVIOCGREP, ioc(IOC_READ, 0x03, 8));
        assert_eq!(EVIOCSREP, ioc(IOC_WRITE, 0x03, 8));
        assert_eq!(EVIOCRMFF, ioc(IOC_WRITE, 0x81, 4));
        assert_eq!(EVIOCGEFFECTS, ioc(IOC_READ, 0x84, 4));
        assert_eq!(EVIOCGRAB, ioc(IOC_WRITE, 0x90, 4));
        assert_eq!(EVIOCREVOKE, ioc(IOC_WRITE, 0x91, 4));
        assert_eq!(EVIOCSCLOCKID, ioc(IOC_WRITE, 0xa0, 4));
        assert_eq!(EVIOCGKEYCODE, ioc(IOC_READ, 0x04, 8));
        assert_eq!(EVIOCGKEYCODE_V2, ioc(IOC_READ, 0x04, KEYMAP_ENTRY as u64));
        assert_eq!(EVIOCSKEYCODE, ioc(IOC_WRITE, 0x04, 8));
        assert_eq!(EVIOCSKEYCODE_V2, ioc(IOC_WRITE, 0x04, KEYMAP_ENTRY as u64));
        assert_eq!(EVIOCGMASK, ioc(IOC_READ, 0x92, 16));
        assert_eq!(EVIOCSMASK, ioc(IOC_WRITE, 0x93, 16));
    }

    #[test]
    fn node_names() {
        assert_eq!(index_of("event12"), Some(12));
        assert_eq!(index_of("mice"), None);
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        as_device(&mut st, 2);
        // makedev(13, 66) in Darwin's encoding.
        assert_eq!(st.st_rdev as u32, (13 << 24) | 66);
        assert_eq!(st.st_mode & libc::S_IFMT, libc::S_IFCHR);
    }
}
