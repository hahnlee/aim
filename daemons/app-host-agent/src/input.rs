//! Input for the app host's windows, as the original's InputReader and
//! InputDispatcher deliver it: the direct touchscreen's events, cooked
//! into motions (`aim_input_transport`), published on the input channel
//! of the front window whose touchable region holds a gesture's start, in
//! the window's coordinates; the windows in front of it that watch
//! outside touches are told of the start (`ACTION_OUTSIDE`, its position
//! only for the target's own app). Windows stack in the order they were added or focused;
//! the front one has the focus and is told so (`publishFocusEvent`). What
//! a window sends back (finished and timeline messages) is read and
//! dropped: no input times out (no ANR). Only the touchscreen is read
//! (#1434).

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

pub use aim_input_transport::Region;
use aim_input_transport::{ABS_MT_POSITION_X, ABS_MT_POSITION_Y, Touch, focus, touched};

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

/// A window: its channel's index and server end, and its region.
struct Target {
    index: i32,
    channel: Arc<OwnedFd>,
    region: Region,
}

impl Target {
    fn origin(&self) -> (f32, f32) {
        (self.region.frame[0] as f32, self.region.frame[1] as f32)
    }
}


/// The windows, back to front, and the display's size in pixels.
#[derive(Default)]
struct Windows {
    stack: Vec<Target>,
    display: (f32, f32),
    /// The window of the gesture under way.
    gesture: Option<i32>,
}

pub struct Dispatcher {
    windows: Mutex<Windows>,
    next_seq: Mutex<(u32, i32)>,
}

impl Dispatcher {
    pub fn new() -> Arc<Dispatcher> {
        let dispatcher = Arc::new(Dispatcher { windows: Mutex::new(Windows::default()), next_seq: Mutex::new((0, 0)) });
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

    /// The channel `index` of a window of `region` on a display of
    /// `display` pixels was made: the window comes to the front and takes
    /// the focus. It goes when the app closes its end.
    pub fn add_window(self: &Arc<Self>, index: i32, channel: OwnedFd, display: (i32, i32), region: Region) {
        let channel = Arc::new(channel);
        let drained = channel.clone();
        let dispatcher = self.clone();
        std::thread::spawn(move || {
            drain(&drained);
            dispatcher.remove_window(index);
        });
        let mut windows = self.windows.lock().unwrap();
        windows.display = (display.0 as f32, display.1 as f32);
        windows.stack.push(Target { index, channel, region });
        self.refocus(&mut windows, None);
    }

    /// The window of channel `index` comes to the front and takes the focus.
    pub fn focus_window(&self, index: i32) -> bool {
        let mut windows = self.windows.lock().unwrap();
        let Some(at) = windows.stack.iter().position(|t| t.index == index) else { return false };
        let previous = windows.stack.last().map(|t| t.index);
        let target = windows.stack.remove(at);
        windows.stack.push(target);
        self.refocus(&mut windows, previous);
        true
    }

    /// The window of channel `index` has `region` now.
    pub fn set_region(&self, index: i32, region: Region) -> bool {
        let mut windows = self.windows.lock().unwrap();
        windows.stack.iter_mut().find(|t| t.index == index).map(|t| t.region = region).is_some()
    }

    fn remove_window(&self, index: i32) {
        let mut windows = self.windows.lock().unwrap();
        let previous = windows.stack.last().map(|t| t.index);
        windows.stack.retain(|t| t.index != index);
        if windows.gesture == Some(index) {
            windows.gesture = None;
        }
        if previous == Some(index) {
            self.refocus(&mut windows, None);
        }
    }

    /// Tells the window that had the focus, `previous` (unless it is the
    /// front one), that it lost it, and the front one that it has it.
    fn refocus(&self, windows: &mut Windows, previous: Option<i32>) {
        let front = windows.stack.last().map(|t| t.index);
        if previous == front {
            return;
        }
        for target in &windows.stack {
            if Some(target.index) == previous || Some(target.index) == front {
                let (seq, id) = self.ids();
                send(&target.channel, &focus(seq, id, Some(target.index) == front));
            }
        }
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
                        let mut windows = self.windows.lock().unwrap();
                        let scale = (windows.display.0 / x_axis.1 as f32, windows.display.1 / y_axis.1 as f32);
                        for motion in touch.report(time, x_axis.0, y_axis.0, scale) {
                            // A gesture goes to the front window that takes its
                            // start, if any; the windows in front of it that
                            // watch outside touches are told
                            // (`findTouchedWindowTargets`).
                            if let Some(start) = motion.gesture_start() {
                                let regions: Vec<Region> = windows.stack.iter().map(|t| t.region).collect();
                                let (at, outside) = touched(&regions, start);
                                windows.gesture = at.map(|at| windows.stack[at].index);
                                let owner = at.map(|at| regions[at].owner);
                                for t in outside.into_iter().map(|i| &windows.stack[i]) {
                                    let (seq, id) = self.ids();
                                    let own = Some(t.region.owner) == owner;
                                    send(&t.channel, &motion.outside(own).message(seq, id, t.origin()));
                                }
                            }
                            let target = windows.gesture.and_then(|index| windows.stack.iter().find(|t| t.index == index));
                            if let Some(target) = target {
                                let (seq, id) = self.ids();
                                send(&target.channel, &motion.message(seq, id, target.origin()));
                            }
                            if motion.ends_gesture() {
                                windows.gesture = None;
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

