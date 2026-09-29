//! Input devices of the display server (`docs/input.md`).
//!
//! Input is not a HAL: the window's AppKit events become Linux evdev
//! devices, which the syscall layer shows as `/dev/input/eventN` and the
//! original inputflinger (EventHub) reads as it would a kernel's.
//!
//! - **Where.** The devices of the display server at `SOCKET` are Unix
//!   stream sockets named `eventN` in [`device_dir`]`(SOCKET)`, one per
//!   device, listening while the device exists. The directory is the guest's
//!   `/dev/input`: listing it lists the devices, and its inotify events are
//!   hotplug.
//! - **Open.** A connection is one open file of the device. The client sends
//!   a [`Hello`] with [`OP_OPEN`]; the server answers with the device's
//!   [`Descriptor`] (capabilities and current state) and from then on sends
//!   [`Record`]s, a whole packet (ending in `SYN_REPORT`) at a time. Records
//!   the client writes are injected into the device, as writes to an evdev
//!   node are.
//! - **Control.** The requests an open file makes of the device go on a
//!   separate connection: grab ([`OP_GRAB`]), the device as it is now
//!   ([`OP_DESCRIBE`]), axis changes ([`OP_SET_ABS`]), event masks
//!   ([`OP_MASK`]), the keymap ([`OP_KEYCODE`]) and flushing a client's
//!   queue ([`OP_FLUSH`]). The server answers one `i32` (0 or a negative
//!   Linux errno), and some a record after it.
//!
//! The server ([`server`]) applies the input core's rules: events a device
//! does not declare and values that do not change are dropped, and an empty
//! packet is not sent. A client's event masks drop the events it masked.

pub mod codes;
pub mod keymap;
pub mod server;
pub mod translate;

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use codes::*;

/// Sent in every [`Hello`]; the server closes a connection of another
/// version.
pub const VERSION: u32 = 2;

/// Open the device: the server answers with a [`Descriptor`].
pub const OP_OPEN: u32 = 1;
/// `client` = the open client's id, `arg` = 1 to grab, 0 to release.
pub const OP_GRAB: u32 = 2;
/// The [`Descriptor`] as the device is now, its state the device's.
pub const OP_DESCRIBE: u32 = 3;
/// `EVIOCSABS`: `arg` = the axis, an [`AbsInfo`] follows.
pub const OP_SET_ABS: u32 = 4;
/// `EVIOCGMASK`/`EVIOCSMASK` of `client`: `arg` = the event type, and
/// [`MASK_SET`] with a [`Mask`] following to set it; the answer is followed
/// by the mask (all set when none was given).
pub const OP_MASK: u32 = 5;
/// `EVIOCGKEYCODE`/`EVIOCSKEYCODE`: a [`KeyEntry`] follows, `arg` = 1 to
/// set; the answer is followed by the entry.
pub const OP_KEYCODE: u32 = 6;
/// `EVIOCSCLOCKID` with events queued: the server writes a [`FLUSH`]
/// record and `SYN_DROPPED` to `client`, which drops what came before.
pub const OP_FLUSH: u32 = 7;
/// Set the event type's mask ([`OP_MASK`]).
pub const MASK_SET: u64 = 1 << 32;

/// A [`Record`] kind that is no event (Linux types are below `EV_CNT`):
/// the end of the events [`OP_FLUSH`] drops.
pub const FLUSH: u16 = 0xffff;

/// An event type's mask: bit N passes code N (`EVIOCSMASK`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mask(pub [u8; 96]);

impl Default for Mask {
    fn default() -> Self {
        Mask([0; 96])
    }
}

/// A keymap entry: the key at scan code `scancode`, or at position
/// `index` when `by_index` is set (`struct input_keymap_entry`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyEntry {
    pub by_index: u32,
    pub index: u32,
    pub scancode: u32,
    pub keycode: u32,
}

/// The first record on every connection, client to server.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Hello {
    pub version: u32,
    pub op: u32,
    pub client: u64,
    pub arg: u64,
}

/// One input event. The time is the guest's `CLOCK_MONOTONIC`; the syscall
/// layer converts it to the client's clock and to `struct input_event`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub time_ns: i64,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// `struct input_absinfo`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AbsInfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

/// The event codes a device declares, one bitmap per type, each as long as
/// `EVIOCGBIT` returns it.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Bits {
    pub ev: [u8; 8],
    pub key: [u8; 96],
    pub rel: [u8; 8],
    pub abs: [u8; 8],
    pub msc: [u8; 8],
    pub led: [u8; 8],
    pub snd: [u8; 8],
    pub ff: [u8; 16],
    pub sw: [u8; 8],
}

/// The contact slots a multitouch device has at most.
pub const MAX_SLOTS: usize = 10;

/// A device's state: what `EVIOCGKEY`, `EVIOCGABS` and the others read.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct State {
    pub key: [u8; 96],
    pub led: [u8; 8],
    pub snd: [u8; 8],
    pub sw: [u8; 8],
    /// Axis values; `abs[ABS_MT_SLOT]` is the current slot.
    pub abs: [i32; ABS_CNT],
    /// Per-contact axis values, `ABS_MT_FIRST..=ABS_MT_LAST`, by slot.
    pub mt: [[i32; MT_AXES]; MAX_SLOTS],
}

/// A device: identity, capabilities and (as sent to a client) its state.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Descriptor {
    /// The client this descriptor was sent to, for [`OP_GRAB`].
    pub client: u64,
    /// N of `eventN`.
    pub index: u32,
    /// Contact slots (0: not a multitouch device).
    pub mt_slots: u32,
    /// `struct input_id`: bus type, vendor, product, version.
    pub id: [u16; 4],
    /// NUL-terminated.
    pub name: [u8; 80],
    pub props: [u8; 8],
    pub bits: Bits,
    /// Axis ranges; the values are in `state`.
    pub absinfo: [AbsInfo; ABS_CNT],
    pub state: State,
}

impl Default for Descriptor {
    fn default() -> Self {
        // SAFETY: plain old data; all zeroes is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

pub fn test_bit(bits: &[u8], n: u16) -> bool {
    bits.get(n as usize / 8)
        .is_some_and(|b| b & (1 << (n % 8)) != 0)
}

pub(crate) fn set_bit(bits: &mut [u8], n: u16, on: bool) {
    if let Some(b) = bits.get_mut(n as usize / 8) {
        if on {
            *b |= 1 << (n % 8);
        } else {
            *b &= !(1 << (n % 8));
        }
    }
}

impl Bits {
    /// The bitmap of event type `ev` (0: the types themselves).
    pub fn of(&self, ev: u16) -> Option<&[u8]> {
        Some(match ev {
            0 => &self.ev,
            EV_KEY => &self.key,
            EV_REL => &self.rel,
            EV_ABS => &self.abs,
            EV_MSC => &self.msc,
            EV_LED => &self.led,
            EV_SND => &self.snd,
            EV_FF => &self.ff,
            EV_SW => &self.sw,
            _ => return None,
        })
    }

    pub(crate) fn of_mut(&mut self, ev: u16) -> Option<&mut [u8]> {
        Some(match ev {
            0 => &mut self.ev,
            EV_KEY => &mut self.key,
            EV_REL => &mut self.rel,
            EV_ABS => &mut self.abs,
            EV_MSC => &mut self.msc,
            EV_LED => &mut self.led,
            EV_SND => &mut self.snd,
            EV_FF => &mut self.ff,
            EV_SW => &mut self.sw,
            _ => return None,
        })
    }

    fn supports(&self, ev: u16, code: u16) -> bool {
        test_bit(&self.ev, ev) && self.of(ev).is_some_and(|b| test_bit(b, code))
    }
}

impl State {
    /// The bitmap `EVIOCGKEY`, `EVIOCGLED`, `EVIOCGSND` or `EVIOCGSW`
    /// reads.
    pub fn bits(&self, ev: u16) -> Option<&[u8]> {
        Some(match ev {
            EV_KEY => &self.key,
            EV_LED => &self.led,
            EV_SND => &self.snd,
            EV_SW => &self.sw,
            _ => return None,
        })
    }

    pub fn bits_mut(&mut self, ev: u16) -> Option<&mut [u8]> {
        Some(match ev {
            EV_KEY => &mut self.key,
            EV_LED => &mut self.led,
            EV_SND => &mut self.snd,
            EV_SW => &mut self.sw,
            _ => return None,
        })
    }

    /// The value of per-contact axis `code` in `slot`.
    pub fn mt_value(&self, slot: usize, code: u16) -> i32 {
        self.mt[slot][(code - ABS_MT_TOUCH_MAJOR) as usize]
    }
}

impl Descriptor {
    fn new(index: u32, name: &str) -> Descriptor {
        let mut d = Descriptor {
            index,
            id: [BUS_VIRTUAL, 0, 0, 1],
            ..Default::default()
        };
        let n = name.len().min(d.name.len() - 1);
        d.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        d.declare(EV_SYN, SYN_REPORT);
        d
    }

    fn declare(&mut self, ev: u16, code: u16) {
        set_bit(&mut self.bits.ev, ev, true);
        if let Some(b) = self.bits.of_mut(ev) {
            set_bit(b, code, true);
        }
    }

    fn axis(&mut self, code: u16, minimum: i32, maximum: i32, resolution: i32) {
        self.declare(EV_ABS, code);
        self.absinfo[code as usize] = AbsInfo {
            minimum,
            maximum,
            resolution,
            ..Default::default()
        };
    }

    /// The name, without its NUL.
    pub fn name(&self) -> &[u8] {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(0);
        &self.name[..n]
    }

    /// Apply `r` to `state` the way the input core does: returns whether
    /// the event reaches clients. Undeclared events and values that do not
    /// change are dropped; a slot change passes when it moves the slot.
    pub fn apply(&self, state: &mut State, r: &Record) -> bool {
        if r.kind == EV_SYN {
            return true;
        }
        if !self.bits.supports(r.kind, r.code) {
            return false;
        }
        match r.kind {
            EV_KEY if r.value == 2 => true,
            EV_KEY | EV_LED | EV_SND | EV_SW => {
                let Some(b) = state.bits_mut(r.kind) else {
                    return false;
                };
                if test_bit(b, r.code) == (r.value != 0) {
                    return false;
                }
                set_bit(b, r.code, r.value != 0);
                true
            }
            EV_ABS if r.code == ABS_MT_SLOT => {
                let slot = &mut state.abs[ABS_MT_SLOT as usize];
                if r.value < 0 || r.value as u32 >= self.mt_slots || *slot == r.value {
                    return false;
                }
                *slot = r.value;
                true
            }
            EV_ABS => {
                let old = if is_mt_axis(r.code) && self.mt_slots > 0 {
                    let slot = state.abs[ABS_MT_SLOT as usize] as usize;
                    &mut state.mt[slot][(r.code - ABS_MT_TOUCH_MAJOR) as usize]
                } else {
                    &mut state.abs[r.code as usize]
                };
                if *old == r.value {
                    return false;
                }
                *old = r.value;
                true
            }
            EV_REL => r.value != 0,
            EV_MSC => true,
            _ => false,
        }
    }
}

/// The directory holding the input devices of the display server at
/// `socket`: `SOCKET.input`.
pub fn device_dir(socket: &Path) -> PathBuf {
    let mut s = OsString::from(socket.as_os_str());
    s.push(".input");
    PathBuf::from(s)
}

/// `eventN` of each device, by index.
pub const TOUCHSCREEN: u32 = 0;
pub const KEYBOARD: u32 = 1;
pub const MOUSE: u32 = 2;

fn per_mm(dpi: f64) -> i32 {
    (dpi / 25.4).round() as i32
}

/// The window as a direct multitouch screen in the display's pixels
/// (protocol B), with `dpi` for the axes' resolution.
pub fn touchscreen(width: u32, height: u32, dpi_x: f64, dpi_y: f64) -> Descriptor {
    let mut d = Descriptor::new(TOUCHSCREEN, "aim-touchscreen");
    d.mt_slots = MAX_SLOTS as u32;
    set_bit(&mut d.props, INPUT_PROP_DIRECT, true);
    d.declare(EV_KEY, BTN_TOUCH);
    d.axis(ABS_MT_SLOT, 0, MAX_SLOTS as i32 - 1, 0);
    d.axis(ABS_MT_POSITION_X, 0, width as i32 - 1, per_mm(dpi_x));
    d.axis(ABS_MT_POSITION_Y, 0, height as i32 - 1, per_mm(dpi_y));
    d.axis(ABS_MT_TRACKING_ID, 0, 65535, 0);
    for slot in &mut d.state.mt {
        slot[(ABS_MT_TRACKING_ID - ABS_MT_TOUCH_MAJOR) as usize] = -1;
    }
    d
}

/// A device to create: its descriptor and its keymap (scan code, key;
/// empty for a device without one).
pub struct Spec {
    pub desc: Descriptor,
    pub keymap: Vec<(u32, u16)>,
}

/// The keyboard's keymap: every key [`keymap`] maps, and Back (the
/// consumer page's AC Back), which the Mac's back gestures press
/// (`translate::Input::back`).
pub fn keyboard_keymap() -> Vec<(u32, u16)> {
    let mut map = keymap::keymap();
    map.push((keymap::AC_BACK, KEY_BACK));
    map
}

/// The Mac's keyboard: the keys of [`keyboard_keymap`].
pub fn keyboard() -> Descriptor {
    let mut d = Descriptor::new(KEYBOARD, "aim-keyboard");
    for (_, code) in keyboard_keymap() {
        d.declare(EV_KEY, code);
    }
    d
}

/// The pointer as an absolute mouse on the display (`touch.deviceType =
/// pointer` in its `.idc`): the tool in range under the Mac's cursor
/// (hover), the buttons other than the primary one (a touch), and
/// scrolling on both axes where the pointer is. Axes in display pixels.
pub fn mouse(width: u32, height: u32, dpi_x: f64, dpi_y: f64) -> Descriptor {
    let mut d = Descriptor::new(MOUSE, "aim-mouse");
    set_bit(&mut d.props, INPUT_PROP_POINTER, true);
    for code in [
        BTN_RIGHT,
        BTN_MIDDLE,
        BTN_SIDE,
        BTN_EXTRA,
        BTN_TOOL_MOUSE,
        BTN_TOUCH,
    ] {
        d.declare(EV_KEY, code);
    }
    d.axis(ABS_X, 0, width as i32 - 1, per_mm(dpi_x));
    d.axis(ABS_Y, 0, height as i32 - 1, per_mm(dpi_y));
    for code in [REL_HWHEEL, REL_WHEEL, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES] {
        d.declare(EV_REL, code);
    }
    d
}

/// Every device, by index.
pub fn devices(width: u32, height: u32, dpi_x: f64, dpi_y: f64) -> Vec<Spec> {
    let spec = |desc| Spec {
        desc,
        keymap: Vec::new(),
    };
    vec![
        spec(touchscreen(width, height, dpi_x, dpi_y)),
        Spec {
            desc: keyboard(),
            keymap: keyboard_keymap(),
        },
        spec(mouse(width, height, dpi_x, dpi_y)),
    ]
}

/// The events of `kind` a mask can select (`evdev_get_mask_cnt`): 0 for
/// the types it cannot.
pub fn mask_codes(kind: u32) -> usize {
    match kind as u16 {
        EV_REP => 0,
        k if kind < EV_CNT as u32 => max_code(k).map_or(0, |m| m as usize + 1),
        _ => 0,
    }
}

/// Whether `masks` (by event type, None: all pass) drop the event
/// (`__evdev_is_filtered`).
pub fn is_masked(masks: &[Option<Mask>], kind: u16, code: u16) -> bool {
    if kind == EV_SYN || kind as usize >= EV_CNT {
        return false;
    }
    if let Some(Some(m)) = masks.first()
        && !test_bit(&m.0, kind)
    {
        return true;
    }
    let n = mask_codes(kind as u32);
    if n == 0 || code as usize >= n {
        return false;
    }
    matches!(masks.get(kind as usize), Some(Some(m)) if !test_bit(&m.0, code))
}

unsafe extern "C" {
    /// Darwin: this thread's own working directory (-1: the process's).
    fn pthread_fchdir_np(fd: libc::c_int) -> libc::c_int;
}

/// A Unix stream socket and the address `name`, for [`in_dir`].
fn unix_socket(name: &str) -> io::Result<(OwnedFd, libc::sockaddr_un)> {
    // SAFETY: a new socket; the address is zeroed, then filled in bounds.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(fd);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        let mut a: libc::sockaddr_un = std::mem::zeroed();
        a.sun_family = libc::AF_UNIX as u8;
        if name.len() >= a.sun_path.len() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        for (i, b) in name.bytes().enumerate() {
            a.sun_path[i] = b as libc::c_char;
        }
        Ok((fd, a))
    }
}

/// Bind (`listen == true`, then listen) or connect a Unix stream socket to
/// `name` in `dir`, by the name alone from `dir` as this thread's working
/// directory: the directory's path may be longer than a socket address
/// holds.
fn in_dir(dir: &Path, name: &str, listen: bool) -> io::Result<OwnedFd> {
    let d = std::fs::File::open(dir)?;
    let (fd, a) = unix_socket(name)?;
    let len = size_of::<libc::sockaddr_un>() as u32;
    let p = (&a as *const libc::sockaddr_un).cast();
    // SAFETY: our socket, a local address and an open directory fd; the
    // thread's directory is restored before returning.
    let r = unsafe {
        if pthread_fchdir_np(d.as_raw_fd()) < 0 {
            return Err(io::Error::last_os_error());
        }
        let r = if listen {
            let r = libc::bind(fd.as_raw_fd(), p, len);
            if r == 0 {
                libc::listen(fd.as_raw_fd(), 16)
            } else {
                r
            }
        } else {
            libc::connect(fd.as_raw_fd(), p, len)
        };
        let e = io::Error::last_os_error();
        pthread_fchdir_np(-1);
        if r < 0 {
            return Err(e);
        }
        r
    };
    debug_assert_eq!(r, 0);
    Ok(fd)
}

/// Connect to device node `name` in `dir`.
pub fn connect_in(dir: &Path, name: &str) -> io::Result<UnixStream> {
    in_dir(dir, name, false).map(UnixStream::from)
}

fn listen_in(dir: &Path, name: &str) -> io::Result<UnixListener> {
    in_dir(dir, name, true).map(UnixListener::from)
}

/// Records as bytes.
pub fn record_bytes(r: &[Record]) -> &[u8] {
    // SAFETY: `Record` is plain old data without padding.
    unsafe { std::slice::from_raw_parts(r.as_ptr().cast(), size_of_val(r)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: u16, code: u16, value: i32) -> Record {
        Record {
            time_ns: 0,
            kind,
            code,
            value,
        }
    }

    #[test]
    fn layouts_match_linux() {
        assert_eq!(size_of::<AbsInfo>(), 24);
        assert_eq!(size_of::<Record>(), 16);
        assert_eq!(bitmap_bytes(KEY_MAX), size_of::<[u8; 96]>());
        assert_eq!(bitmap_bytes(FF_MAX), 16);
        for ev in [
            0, EV_KEY, EV_REL, EV_ABS, EV_MSC, EV_LED, EV_SND, EV_FF, EV_SW,
        ] {
            let d = Descriptor::default();
            assert_eq!(
                d.bits.of(ev).unwrap().len(),
                bitmap_bytes(max_code(ev).unwrap())
            );
        }
        assert_eq!(bitmap_bytes(INPUT_PROP_MAX), 8);
    }

    #[test]
    fn input_core_rules() {
        let d = touchscreen(1080, 1920, 254.0, 254.0);
        let mut s = d.state;
        assert!(d.apply(&mut s, &rec(EV_KEY, BTN_TOUCH, 1)));
        assert!(
            !d.apply(&mut s, &rec(EV_KEY, BTN_TOUCH, 1)),
            "unchanged key"
        );
        assert!(!d.apply(&mut s, &rec(EV_KEY, 30, 1)), "undeclared key");
        assert!(d.apply(&mut s, &rec(EV_ABS, ABS_MT_POSITION_X, 10)));
        assert!(!d.apply(&mut s, &rec(EV_ABS, ABS_MT_POSITION_X, 10)));
        assert!(!d.apply(&mut s, &rec(EV_ABS, ABS_MT_SLOT, 0)), "same slot");
        assert!(d.apply(&mut s, &rec(EV_ABS, ABS_MT_SLOT, 1)));
        assert!(
            d.apply(&mut s, &rec(EV_ABS, ABS_MT_POSITION_X, 10)),
            "other slot"
        );
        assert!(
            !d.apply(&mut s, &rec(EV_ABS, ABS_MT_SLOT, 10)),
            "no such slot"
        );
        assert_eq!(s.mt_value(0, ABS_MT_POSITION_X), 10);
        assert_eq!(s.mt_value(2, ABS_MT_TRACKING_ID), -1);
        assert_eq!(d.absinfo[ABS_MT_POSITION_X as usize].maximum, 1079);
        assert_eq!(d.absinfo[ABS_MT_POSITION_Y as usize].resolution, 10);
        assert!(test_bit(&d.props, INPUT_PROP_DIRECT));

        let m = mouse(1080, 1920, 254.0, 254.0);
        let mut s = m.state;
        assert!(!m.apply(&mut s, &rec(EV_REL, REL_WHEEL, 0)), "no motion");
        assert!(m.apply(&mut s, &rec(EV_REL, REL_WHEEL, -1)));
        assert!(m.apply(&mut s, &rec(EV_REL, REL_WHEEL, -1)), "relative");
        assert!(m.apply(&mut s, &rec(EV_ABS, ABS_X, 5)));
        assert!(test_bit(&m.props, INPUT_PROP_POINTER));
        assert!(!test_bit(&m.props, INPUT_PROP_DIRECT));
    }

    #[test]
    fn masks_filter_as_evdev_does() {
        let mut masks: Vec<Option<Mask>> = vec![None; EV_CNT];
        assert!(!is_masked(&masks, EV_KEY, 30));
        let mut keys = Mask::default();
        set_bit(&mut keys.0, 30, true);
        masks[EV_KEY as usize] = Some(keys);
        assert!(!is_masked(&masks, EV_KEY, 30));
        assert!(is_masked(&masks, EV_KEY, 31));
        // SYN and unknown codes or types are never masked.
        assert!(!is_masked(&masks, EV_SYN, SYN_REPORT));
        assert!(!is_masked(&masks, EV_KEY, 0x300));
        assert!(!is_masked(&masks, EV_REP, 0));
        // The type mask comes first.
        masks[0] = Some(Mask::default());
        assert!(is_masked(&masks, EV_KEY, 30));
        assert!(is_masked(&masks, EV_REL, 0));
        assert_eq!(mask_codes(0), EV_CNT);
        assert_eq!(mask_codes(EV_KEY as u32), 0x300);
        assert_eq!(mask_codes(EV_REP as u32), 0);
        assert_eq!(mask_codes(0x40), 0);
    }

    #[test]
    fn device_dir_is_beside_the_socket() {
        assert_eq!(
            device_dir(Path::new("/tmp/display.sock")),
            PathBuf::from("/tmp/display.sock.input")
        );
    }
}
