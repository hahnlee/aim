//! Input for the app host's windows, as the original's InputReader and
//! InputDispatcher deliver it: the direct touchscreen's events, cooked
//! into motions (`aim_input_transport`), published on the focused
//! window's input channel. The focused window is the
//! latest one added; it is told so (`publishFocusEvent`). What the window
//! sends back (finished and timeline messages) is read and dropped: no
//! input times out (no ANR). Only the touchscreen is read (#1434).

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

use aim_input_transport::{ABS_MT_POSITION_X, ABS_MT_POSITION_Y, Touch, focus};

/// `IdGenerator::Source::INPUT_DISPATCHER`, in an event id's top bits.
const ID_SOURCE_DISPATCHER: i32 = 1 << 30;

/// `<linux/input-event-codes.h>`.
const EV_SYN: u16 = 0;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
const INPUT_PROP_DIRECT: usize = 1;

/// `<linux/input.h>`'s ioctls.
const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((b'E' as u64) << 8) | nr
}
const EVIOCGPROP: u64 = ioc(2, 0x09, 8);
const EVIOCGBIT_ABS: u64 = ioc(2, 0x20 + EV_ABS as u64, 8);
const EVIOCSCLOCKID: u64 = ioc(1, 0xa0, 4);
const fn eviocgabs(axis: u16) -> u64 {
    ioc(2, 0x40 + axis as u64, 24)
}

/// The window input goes to: its channel's server end and the display's
/// size in pixels.
struct Target {
    channel: Arc<OwnedFd>,
    width: f32,
    height: f32,
}

pub struct Dispatcher {
    target: Mutex<Option<Target>>,
    next_seq: Mutex<(u32, i32)>,
}

impl Dispatcher {
    pub fn new() -> Arc<Dispatcher> {
        let dispatcher = Arc::new(Dispatcher { target: Mutex::new(None), next_seq: Mutex::new((0, 0)) });
        let reader = dispatcher.clone();
        std::thread::spawn(move || reader.read_touchscreen());
        dispatcher
    }

    /// A message's sequence number and event id.
    fn ids(&self) -> (u32, i32) {
        let mut next = self.next_seq.lock().unwrap();
        next.0 = next.0.wrapping_add(1).max(1);
        next.1 = next.1.wrapping_add(1) & 0x3fff_ffff;
        (next.0, ID_SOURCE_DISPATCHER | next.1)
    }

    /// A window's channel was made: it takes the focus.
    pub fn add_window(&self, channel: OwnedFd, width: i32, height: i32) {
        let channel = Arc::new(channel);
        let drained = channel.clone();
        std::thread::spawn(move || drain(&drained));
        let (seq, id) = self.ids();
        send(&channel, &focus(seq, id, true));
        *self.target.lock().unwrap() = Some(Target { channel, width: width as f32, height: height as f32 });
    }

    /// Reads the touchscreen, as long as the agent runs.
    fn read_touchscreen(&self) {
        let (mut device, x_axis, y_axis) = match open_touchscreen() {
            Ok(Some(found)) => found,
            Ok(None) => return log::info!("no touchscreen: no touch input"),
            Err(error) => return log::error!("touchscreen: {error}"),
        };
        let mut touch = Touch::default();
        let mut dropped = false;
        let mut event = [0u8; 24];
        loop {
            if let Err(error) = device.read_exact(&mut event) {
                return log::error!("touchscreen: {error}");
            }
            let sec = i64::from_ne_bytes(event[0..8].try_into().unwrap());
            let usec = i64::from_ne_bytes(event[8..16].try_into().unwrap());
            let kind = u16::from_ne_bytes(event[16..18].try_into().unwrap());
            let code = u16::from_ne_bytes(event[18..20].try_into().unwrap());
            let value = i32::from_ne_bytes(event[20..24].try_into().unwrap());
            match (kind, code) {
                // The events up to the next report are incomplete: dropped
                // with it, as InputReader drops them.
                (EV_SYN, SYN_DROPPED) => dropped = true,
                (EV_SYN, SYN_REPORT) => {
                    if !std::mem::take(&mut dropped) {
                        let time = sec * 1_000_000_000 + usec * 1_000;
                        let target = self.target.lock().unwrap();
                        if let Some(target) = target.as_ref() {
                            let scale = (target.width / x_axis.1 as f32, target.height / y_axis.1 as f32);
                            for motion in touch.report(time, x_axis.0, y_axis.0, scale) {
                                let (seq, id) = self.ids();
                                send(&target.channel, &motion.message(seq, id));
                            }
                        }
                    }
                }
                (EV_ABS, _) if !dropped => touch.abs(code, value),
                _ => {}
            }
        }
    }
}

/// The first direct device with multitouch positions (as EventHub
/// classifies a touchscreen), on the touchscreen's clock, and its x and y
/// axes' minimum and extent.
fn open_touchscreen() -> std::io::Result<Option<(File, (i32, i32), (i32, i32))>> {
    let mut paths: Vec<_> = std::fs::read_dir("/dev/input")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
        .collect();
    paths.sort();
    for path in paths {
        let device = File::open(&path)?;
        let fd = device.as_raw_fd();
        let (mut props, mut abs) = ([0u8; 8], [0u8; 8]);
        // SAFETY: ioctls filling buffers of the sizes their numbers carry.
        let direct = unsafe { libc::ioctl(fd, EVIOCGPROP as _, props.as_mut_ptr()) } >= 0
            && props[INPUT_PROP_DIRECT / 8] & (1 << (INPUT_PROP_DIRECT % 8)) != 0;
        let positions = unsafe { libc::ioctl(fd, EVIOCGBIT_ABS as _, abs.as_mut_ptr()) } >= 0
            && u64::from_ne_bytes(abs) & (1 << ABS_MT_POSITION_X) != 0;
        if !(direct && positions) {
            continue;
        }
        let axis = |code| -> std::io::Result<(i32, i32)> {
            let mut info = [0i32; 6];
            // SAFETY: `struct input_absinfo`, six ints.
            if unsafe { libc::ioctl(fd, eviocgabs(code) as _, info.as_mut_ptr()) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok((info[1], info[2] - info[1] + 1))
        };
        let clock = libc::CLOCK_MONOTONIC as libc::c_int;
        // SAFETY: an int argument.
        if unsafe { libc::ioctl(fd, EVIOCSCLOCKID as _, &clock) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        log::info!("touchscreen {}", path.display());
        return Ok(Some((device, axis(ABS_MT_POSITION_X)?, axis(ABS_MT_POSITION_Y)?)));
    }
    Ok(None)
}

/// `InputChannel::sendMessage`: one packet, retried when interrupted.
fn send(channel: &OwnedFd, message: &[u8]) {
    loop {
        // SAFETY: a packet from a live buffer on a socket this process owns.
        let n = unsafe { libc::send(channel.as_raw_fd(), message.as_ptr().cast(), message.len(), libc::MSG_NOSIGNAL) };
        if n >= 0 {
            return;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return log::warn!("input channel: {error}");
        }
    }
}

/// Reads what the window sends back until it closes its end.
fn drain(channel: &OwnedFd) {
    let mut buffer = [0u8; 256];
    loop {
        // SAFETY: a receive into a local buffer.
        let n = unsafe { libc::recv(channel.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if n == 0 || (n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted) {
            return;
        }
    }
}
