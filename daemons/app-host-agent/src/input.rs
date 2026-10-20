//! Input for the app host's windows, as the original's InputReader and
//! InputDispatcher deliver it: the direct touchscreen's events, cooked
//! into motions (`aim_input_transport`), published on the input channel
//! of the front window whose touchable region holds a gesture's start, in
//! the window's coordinates; the windows in front of it that watch
//! outside touches are told of the start (`ACTION_OUTSIDE`, its position
//! only for the target's own app). Windows stack in the order they were added or focused;
//! the front one has the focus and is told so (`publishFocusEvent`). What
//! a window sends back (finished and timeline messages) is read and
//! dropped: no input times out (no ANR). The built-in keyboard's keys go
//! to the focused window as `KeyboardInputMapper` maps them through the
//! device's key layout and key character map, which EventHub finds by its
//! name; the dispatcher repeats a held key (`synthesizeKeyRepeatLocked`)
//! as the app host's input settings configure it. The window manager
//! policy's interception (`interceptKeyBeforeQueueing`, system keys) is
//! not modelled; the mouse and other keyboards are not read (#1434).
//! Motions and keys the app host injects (`input tap`, `input swipe`,
//! `input keyevent`, `input text`) are dispatched as the devices' are.

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Condvar, Mutex};

pub use aim_input_transport::Region;
use aim_input_transport::keyboard::{
    self, ConfigFile, ConfigFiles, Identifier, KeyEvent, KeyMap, Keyboard, Mapper, Repeat,
    RepeatConfiguration,
};
use aim_input_transport::{ABS_MT_POSITION_X, ABS_MT_POSITION_Y, Motion, Touch, focus, touched};

/// `IdGenerator::Source::INPUT_DISPATCHER`, in an event id's top bits.
const ID_SOURCE_DISPATCHER: i32 = 1 << 30;

/// `<linux/input-event-codes.h>`.
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const EV_MSC: u16 = 4;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
const MSC_SCAN: u16 = 4;
const INPUT_PROP_DIRECT: usize = 1;
/// `BUS_USB`, `BUS_BLUETOOTH`.
const BUS_USB: u16 = 0x03;
const BUS_BLUETOOTH: u16 = 0x05;
/// The bytes of a key bitmask (`KEY_MAX` bits).
const KEY_BYTES: usize = keyboard::KEY_MAX as usize / 8 + 1;

/// `<linux/input.h>`'s ioctls.
const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((b'E' as u64) << 8) | nr
}
const EVIOCGID: u64 = ioc(2, 0x02, 8);
const EVIOCGNAME: u64 = ioc(2, 0x06, 256);
const EVIOCGUNIQ: u64 = ioc(2, 0x08, 256);
const EVIOCGPROP: u64 = ioc(2, 0x09, 8);
const EVIOCGBIT_KEY: u64 = ioc(2, 0x20 + EV_KEY as u64, KEY_BYTES as u64);
const EVIOCGBIT_ABS: u64 = ioc(2, 0x20 + EV_ABS as u64, 8);
const EVIOCGBIT_MSC: u64 = ioc(2, 0x20 + EV_MSC as u64, 1);
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

/// The built-in keyboard as InputReader describes it
/// (`InputDeviceInfo`): its device id, identifier, sources, keyboard type,
/// whether it is external, and its key character map's file.
pub struct KeyboardInfo {
    pub id: i32,
    pub identifier: Identifier,
    pub sources: i32,
    pub keyboard_type: i32,
    pub is_external: bool,
    pub key_character_map: String,
}

/// The built-in keyboard: what it is, and its device and key map.
struct OpenKeyboard {
    info: KeyboardInfo,
    device: File,
    map: KeyMap,
}

pub struct Dispatcher {
    windows: Mutex<Windows>,
    next_seq: Mutex<(u32, i32)>,
    keyboard: Option<KeyboardInfo>,
    /// The dispatcher's key repeat and its configuration, and what wakes
    /// its timer.
    repeat: Mutex<(Repeat, RepeatConfiguration)>,
    repeat_changed: Condvar,
}

impl Dispatcher {
    pub fn new() -> Arc<Dispatcher> {
        let keyboard = match open_keyboard() {
            Ok(found) => found,
            Err(error) => {
                log::error!("keyboard: {error}");
                None
            }
        };
        let (info, device) = match keyboard {
            Some(k) => (Some(k.info), Some((k.device, k.map))),
            None => (None, None),
        };
        let dispatcher = Arc::new(Dispatcher {
            windows: Mutex::new(Windows::default()),
            next_seq: Mutex::new((0, 0)),
            keyboard: info,
            repeat: Mutex::new((Repeat::default(), RepeatConfiguration::default())),
            repeat_changed: Condvar::new(),
        });
        let reader = dispatcher.clone();
        std::thread::spawn(move || reader.read_touchscreen());
        if let Some((device, map)) = device {
            let reader = dispatcher.clone();
            std::thread::spawn(move || reader.read_keyboard(device, map));
            let repeater = dispatcher.clone();
            std::thread::spawn(move || repeater.repeat_keys());
        }
        dispatcher
    }

    /// The built-in keyboard, if there is one.
    pub fn keyboard(&self) -> Option<&KeyboardInfo> {
        self.keyboard.as_ref()
    }

    /// `setKeyRepeatConfiguration`.
    pub fn set_key_repeat(&self, config: RepeatConfiguration) {
        self.repeat.lock().unwrap().1 = config;
        self.repeat_changed.notify_all();
    }

    /// A key event injected (`InputManager.injectInputEvent`): trusted, of
    /// the virtual keyboard, dispatched as the keyboard's are.
    pub fn inject_key(&self, mut event: KeyEvent) {
        event.device = keyboard::VIRTUAL_KEYBOARD_ID;
        self.dispatch_key(event);
    }

    /// A key goes to the focused window (`dispatchKeyLocked`), its repeat
    /// state kept.
    fn dispatch_key(&self, event: KeyEvent) {
        let event = {
            let mut repeat = self.repeat.lock().unwrap();
            let config = repeat.1;
            repeat.0.dispatch(config, event)
        };
        self.repeat_changed.notify_all();
        self.send_key(&event);
    }

    /// Publishes `event` to the focused window, if any.
    fn send_key(&self, event: &KeyEvent) {
        let windows = self.windows.lock().unwrap();
        if let Some(target) = windows.stack.last() {
            let (seq, id) = self.ids();
            send(&target.channel, &event.message(seq, id));
        }
    }

    /// The dispatcher's key repeat: a held key's repeats, when due
    /// (`synthesizeKeyRepeatLocked`).
    fn repeat_keys(&self) {
        let mut repeat = self.repeat.lock().unwrap();
        loop {
            let now = monotonic_ns();
            let config = repeat.1;
            if let Some(event) = repeat.0.synthesize(config, now) {
                drop(repeat);
                self.send_key(&event);
                repeat = self.repeat.lock().unwrap();
                continue;
            }
            repeat = match repeat.0.next() {
                Some(next) => {
                    let wait = std::time::Duration::from_nanos((next - now).max(0) as u64);
                    self.repeat_changed.wait_timeout(repeat, wait).unwrap().0
                }
                None => self.repeat_changed.wait(repeat).unwrap(),
            };
        }
    }

    /// Reads the keyboard, as long as the agent runs: each key mapped by
    /// `KeyboardInputMapper` (with the HID usage an `MSC_SCAN` before it
    /// gives) and dispatched; autorepeats (value 2) are dropped.
    fn read_keyboard(&self, mut device: File, map: KeyMap) {
        let Some(info) = &self.keyboard else { return };
        let mut mapper = Mapper::new(info.id, info.sources);
        let mut usage = 0;
        let mut event = [0u8; 24];
        loop {
            if let Err(error) = device.read_exact(&mut event) {
                return log::error!("keyboard: {error}");
            }
            let sec = i64::from_ne_bytes(event[0..8].try_into().unwrap());
            let usec = i64::from_ne_bytes(event[8..16].try_into().unwrap());
            let kind = u16::from_ne_bytes(event[16..18].try_into().unwrap());
            let code = u16::from_ne_bytes(event[18..20].try_into().unwrap());
            let value = i32::from_ne_bytes(event[20..24].try_into().unwrap());
            match kind {
                // `HidUsageAccumulator`.
                EV_MSC if code == MSC_SCAN => usage = value,
                EV_SYN if code == SYN_REPORT => usage = 0,
                EV_KEY if value != 2 && keyboard::supported_scan_code(code) => {
                    let time = sec * 1_000_000_000 + usec * 1_000;
                    let usage_code = std::mem::take(&mut usage);
                    if let Some(key) =
                        mapper.process_key(&map, time, value != 0, code.into(), usage_code)
                    {
                        self.dispatch_key(key);
                    }
                }
                _ => {}
            }
        }
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
    pub fn add_window(
        self: &Arc<Self>,
        index: i32,
        channel: OwnedFd,
        display: (i32, i32),
        region: Region,
    ) {
        let channel = Arc::new(channel);
        let drained = channel.clone();
        let dispatcher = self.clone();
        std::thread::spawn(move || {
            drain(&drained);
            dispatcher.remove_window(index);
        });
        let mut windows = self.windows.lock().unwrap();
        windows.display = (display.0 as f32, display.1 as f32);
        windows.stack.push(Target {
            index,
            channel,
            region,
        });
        self.refocus(&mut windows, None);
    }

    /// The window of channel `index` comes to the front and takes the focus.
    pub fn focus_window(&self, index: i32) -> bool {
        let mut windows = self.windows.lock().unwrap();
        let Some(at) = windows.stack.iter().position(|t| t.index == index) else {
            return false;
        };
        let previous = windows.stack.last().map(|t| t.index);
        let target = windows.stack.remove(at);
        windows.stack.push(target);
        self.refocus(&mut windows, previous);
        true
    }

    /// The window of channel `index` has `region` now.
    pub fn set_region(&self, index: i32, region: Region) -> bool {
        let mut windows = self.windows.lock().unwrap();
        windows
            .stack
            .iter_mut()
            .find(|t| t.index == index)
            .map(|t| t.region = region)
            .is_some()
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
                send(
                    &target.channel,
                    &focus(seq, id, Some(target.index) == front),
                );
            }
        }
    }

    /// A motion injected into the display (`InputManager.injectInputEvent`),
    /// dispatched as the touchscreen's are.
    pub fn inject(&self, motion: &Motion) {
        let mut windows = self.windows.lock().unwrap();
        self.dispatch(&mut windows, motion);
    }

    /// A gesture goes to the front window that takes its start, if any; the
    /// windows in front of it that watch outside touches are told
    /// (`findTouchedWindowTargets`).
    fn dispatch(&self, windows: &mut Windows, motion: &Motion) {
        if let Some(start) = motion.gesture_start() {
            let regions: Vec<Region> = windows.stack.iter().map(|t| t.region).collect();
            let (at, outside) = touched(&regions, start);
            windows.gesture = at.map(|at| windows.stack[at].index);
            let owner = at.map(|at| regions[at].owner);
            for t in outside.into_iter().map(|i| &windows.stack[i]) {
                let (seq, id) = self.ids();
                let own = Some(t.region.owner) == owner;
                send(
                    &t.channel,
                    &motion.outside(own).message(seq, id, t.origin()),
                );
            }
        }
        let target = windows
            .gesture
            .and_then(|index| windows.stack.iter().find(|t| t.index == index));
        if let Some(target) = target {
            let (seq, id) = self.ids();
            send(&target.channel, &motion.message(seq, id, target.origin()));
        }
        if motion.ends_gesture() {
            windows.gesture = None;
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
                        let scale = (
                            windows.display.0 / x_axis.1 as f32,
                            windows.display.1 / y_axis.1 as f32,
                        );
                        for motion in touch.report(time, x_axis.0, y_axis.0, scale) {
                            self.dispatch(&mut windows, &motion);
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
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("event"))
        })
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
        return Ok(Some((
            device,
            axis(ABS_MT_POSITION_X)?,
            axis(ABS_MT_POSITION_Y)?,
        )));
    }
    Ok(None)
}

/// `CLOCK_MONOTONIC` in nanoseconds, the input devices' clock.
fn monotonic_ns() -> i64 {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    // SAFETY: filling a local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// A string an evdev ioctl fills: up to its first NUL.
fn ioctl_string(fd: libc::c_int, request: u64) -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the ioctl writes at most the 256 bytes its number carries.
    let n = unsafe { libc::ioctl(fd, request as _, buffer.as_mut_ptr()) };
    if n <= 0 {
        return String::new();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// A system property, empty if unset.
fn property(name: &std::ffi::CStr) -> String {
    let mut value = [0u8; 92];
    // SAFETY: `PROP_VALUE_MAX` bytes for the value.
    let n = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr().cast()) };
    String::from_utf8_lossy(&value[..n.max(0) as usize]).into_owned()
}

/// The built-in keyboard as EventHub opens it (`openDeviceLocked`): the
/// first device of the `KEYBOARD` class, its identifier read, its
/// configuration, key layout and key character map found by its name
/// (`loadConfigurationLocked`, `loadKeyMapLocked`), its classes, and
/// eligible as the built-in keyboard (`isEligibleBuiltInKeyboard`).
/// Another keyboard would be a device of its own, not read (#1434).
fn open_keyboard() -> std::io::Result<Option<OpenKeyboard>> {
    let mut paths: Vec<_> = std::fs::read_dir("/dev/input")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("event"))
        })
        .collect();
    paths.sort();
    let apex = property(c"input_device.config_file.apex");
    let root = std::env::var("ANDROID_ROOT").ok();
    let data = std::env::var("ANDROID_DATA").unwrap_or_default();
    let readable = |path: &str| std::fs::metadata(path).is_ok_and(|m| m.is_file());
    let files = ConfigFiles {
        prefixes: ConfigFiles::prefixes(Some(&apex), root.as_deref(), &data),
        readable: &readable,
    };
    let read = |path: &str| std::fs::read(path).ok();
    for path in paths {
        let device = File::open(&path)?;
        let fd = device.as_raw_fd();
        let mut keys = [0u8; KEY_BYTES];
        let mut msc = [0u8; 1];
        // SAFETY: ioctls filling buffers of the sizes their numbers carry.
        if unsafe { libc::ioctl(fd, EVIOCGBIT_KEY as _, keys.as_mut_ptr()) } < 0 {
            continue;
        }
        unsafe { libc::ioctl(fd, EVIOCGBIT_MSC as _, msc.as_mut_ptr()) };
        let has_key = |code: u16| keys[code as usize / 8] & (1 << (code % 8)) != 0;
        let has_msc_scan = msc[0] & (1 << MSC_SCAN) != 0;
        let mut ids = [0u16; 4];
        // SAFETY: `struct input_id`, four shorts.
        unsafe { libc::ioctl(fd, EVIOCGID as _, ids.as_mut_ptr()) };
        let identifier = Identifier {
            name: ioctl_string(fd, EVIOCGNAME),
            bus: ids[0],
            vendor: ids[1],
            product: ids[2],
            version: ids[3],
            unique_id: ioctl_string(fd, EVIOCGUNIQ),
        };
        let config = match files.by_identifier(&identifier, ConfigFile::Configuration, "") {
            Some(idc) => match read(&idc).map(|c| keyboard::property_map(&c)) {
                Some(Ok(config)) => config,
                Some(Err(error)) => {
                    log::error!("{idc}: {error}");
                    Vec::new()
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        if !keyboard::has_keys(&has_key) {
            continue;
        }
        let map = match KeyMap::load(&identifier, &config, &files, &read) {
            Ok(map) => map,
            Err(error) => {
                log::error!("{}: {error}", path.display());
                continue;
            }
        };
        let classes = Keyboard {
            map: &map,
            has_key: &has_key,
            has_msc_scan,
        }
        .classes();
        if !keyboard::eligible_built_in(&identifier, &config, &map) {
            log::info!(
                "{}: a keyboard other than the built-in one is not read (#1434)",
                identifier.name
            );
            continue;
        }
        // `isExternalDeviceLocked`.
        let is_external = match keyboard::property_bool(&config, "device.internal") {
            Some(internal) => !internal,
            None => matches!(identifier.bus, BUS_USB | BUS_BLUETOOTH),
        };
        let clock = libc::CLOCK_MONOTONIC as libc::c_int;
        // SAFETY: an int argument.
        if unsafe { libc::ioctl(fd, EVIOCSCLOCKID as _, &clock) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let key_character_map = map
            .characters
            .as_ref()
            .map(|kcm| kcm.load_file_name.clone())
            .unwrap_or_default();
        log::info!(
            "keyboard {} ({}): {}",
            path.display(),
            identifier.name,
            key_character_map
        );
        return Ok(Some(OpenKeyboard {
            info: KeyboardInfo {
                id: keyboard::BUILT_IN_KEYBOARD_ID,
                identifier,
                sources: classes.sources(),
                keyboard_type: classes.keyboard_type(),
                is_external,
                key_character_map,
            },
            device,
            map,
        }));
    }
    Ok(None)
}

/// `InputChannel::sendMessage`: one packet, retried when interrupted.
fn send(channel: &OwnedFd, message: &[u8]) {
    loop {
        // SAFETY: a packet from a live buffer on a socket this process owns.
        let n = unsafe {
            libc::send(
                channel.as_raw_fd(),
                message.as_ptr().cast(),
                message.len(),
                libc::MSG_NOSIGNAL,
            )
        };
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
        let n = unsafe {
            libc::recv(
                channel.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        if n == 0
            || (n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted)
        {
            return;
        }
    }
}
