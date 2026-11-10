//! Input for the app host's windows, as the original's InputReader and
//! InputDispatcher deliver it: the direct touchscreen's events, cooked
//! into motions (`aim_input_transport`), published on the input channel
//! of the front window whose touchable region holds a gesture's start, in
//! the window's coordinates; the windows in front of it that watch
//! outside touches are told of the start (`ACTION_OUTSIDE`, its position
//! only for the target's own app). Windows stack in the order they were added or focused;
//! the front one has the focus and is told so (`publishFocusEvent`). What
//! a window sends back (finished and timeline messages) is read and
//! correlated for injected foreground completion (ordinary input has no ANR). A display's touch mode changes
//! as `setInTouchMode` allows it, and its windows are told. The built-in keyboard's keys go
//! to the focused window as `KeyboardInputMapper` maps them through the
//! device's key layout and key character map, which EventHub finds by its
//! name; the dispatcher repeats a held key (`synthesizeKeyRepeatLocked`)
//! as the app host's input settings configure it. The window manager
//! policy's interception (`interceptKeyBeforeQueueing`, system keys) is
//! not modelled; the mouse and other keyboards are not read (#1434).
//! Motions and keys the app host injects (`input tap`, `input swipe`,
//! `input keyevent`, `input text`) are dispatched as the devices' are.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Condvar, Mutex};

pub use aim_input_transport::Region;
use aim_input_transport::keyboard::{
    self, ConfigFile, ConfigFiles, Identifier, KeyEvent, KeyMap, Keyboard, Mapper, Repeat,
    RepeatConfiguration,
};
use aim_input_transport::keylayout::POLICY_FLAG_WAKE;
use aim_input_transport::{
    ABS_MT_POSITION_X, ABS_MT_POSITION_Y, Motion, Touch, focus, touch_mode, touched,
};

#[path = "injection.rs"]
mod injection;

/// The display the windows are on.
const DEFAULT_DISPLAY: i32 = 0;

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
    channel: Arc<injection::Channel>,
    region: Region,
}

impl Drop for Target {
    fn drop(&mut self) {
        self.channel.close();
    }
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
    /// The window of the gesture under way, and the last motion it was
    /// sent of a device's gesture (`InputState`'s motion memento).
    gesture: Option<i32>,
    gesture_motion: Option<Motion>,
    /// The keys down in each window, as it was sent them (`InputState`'s
    /// key mementos).
    keys: Vec<(i32, KeyEvent)>,
    /// The windows the last key or motion went to, but for its outside
    /// targets (`mInteractionConnectionTokens`).
    interactions: Vec<i32>,
    /// Each display's touch mode, once set (`mTouchModePerDisplay`).
    touch_mode: HashMap<i32, bool>,
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
    /// Whether its downs wake the device by default.
    pub wakes: bool,
    pub key_character_map: String,
    pub supported_keys: Option<Vec<i32>>,
}

/// The built-in keyboard: what it is, and its device and key map.
struct OpenKeyboard {
    info: KeyboardInfo,
    device: File,
    map: KeyMap,
}

pub type InputFilter = Arc<dyn Fn(&[u8], i32) + Send + Sync>;
/// The window manager policy's key interception before queueing: whether
/// it consumes a keyboard key instead of passing it to the user.
pub type KeyPolicy = fn(&KeyEvent) -> bool;

/// The window policy's interception of what the devices give while the
/// device is not interactive (`interceptKeyBeforeQueueing`,
/// `interceptMotionBeforeQueueingNonInteractive`): whether it is a motion,
/// the key code, whether a down, its time and policy flags; its result.
pub type InputPolicy = Box<dyn Fn(bool, i32, bool, i64, u32) -> i32 + Send + Sync>;

/// `WindowManagerPolicy.ACTION_PASS_TO_USER`, and the app host's mark of a
/// wake key's down (`mPendingWakeKey`).
const ACTION_PASS_TO_USER: i32 = 1;
const PENDING_WAKE_KEY: i32 = 2;

pub struct Dispatcher {
    filter: Mutex<Option<InputFilter>>,
    /// `NativeInputManager`'s `mInteractive`, and the window policy.
    interactive: std::sync::atomic::AtomicBool,
    policy: Mutex<Option<InputPolicy>>,
    /// `PhoneWindowManager.mPendingWakeKey`: the key whose down woke the
    /// device, its up not dispatched.
    pending_wake_key: Mutex<Option<i32>>,
    windows: Mutex<Windows>,
    next_seq: Mutex<(u32, i32)>,
    keyboard: Option<KeyboardInfo>,
    /// The dispatcher's key repeat and its configuration, and what wakes
    /// its timer.
    repeat: Mutex<(Repeat, RepeatConfiguration)>,
    repeat_changed: Condvar,
    injections: std::sync::mpsc::Sender<(
        aim_input_transport::injection::Event,
        i32,
        Arc<injection::Injection>,
    )>,
}

impl Dispatcher {
    pub fn set_input_filter(&self, filter: Option<InputFilter>) {
        *self.filter.lock().unwrap() = filter;
        self.repeat.lock().unwrap().0.reset();
        self.repeat_changed.notify_all();
    }

    /// notifyKey/notifyMotion call the filter before normal dispatch. Public
    /// injections bypass it, as InputDispatcher.injectInputEvent does.
    pub fn filter_input(&self, message: &[u8]) -> bool {
        let filter = self.filter.lock().unwrap().clone();
        if let Some(filter) = filter {
            filter(
                &aim_input_transport::injection::Event::from_message(message),
                0x02000000 | 0x04000000 | 0x40000000,
            );
            true
        } else {
            false
        }
    }

    /// Publish live membership under the same lock as geometry, focus and
    /// dispatch, including removals whose channel closure has not arrived.
    pub fn sync_windows(&self, live: &[i32]) {
        let mut windows = self.windows.lock().unwrap();
        let previous = windows.stack.last().map(|t| t.index);
        windows.stack.retain(|t| live.contains(&t.index));
        windows.interactions.retain(|i| live.contains(i));
        windows.keys.retain(|(i, _)| live.contains(i));
        if windows.gesture.is_some_and(|i| !live.contains(&i)) {
            windows.gesture = None;
            windows.gesture_motion = None;
        }
        self.refocus(&mut windows, previous);
    }

    pub fn new(policy: KeyPolicy) -> Arc<Dispatcher> {
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
        let (injections, jobs) = injection::queue();
        let dispatcher = Arc::new(Dispatcher {
            windows: Mutex::new(Windows::default()),
            next_seq: Mutex::new((0, 0)),
            keyboard: info,
            repeat: Mutex::new((Repeat::default(), RepeatConfiguration::default())),
            repeat_changed: Condvar::new(),
            injections,
            filter: Mutex::new(None),
            interactive: std::sync::atomic::AtomicBool::new(true),
            policy: Mutex::new(None),
            pending_wake_key: Mutex::new(None),
        });
        let injector = dispatcher.clone();
        std::thread::spawn(move || injector.inject_queue(jobs));
        let reader = dispatcher.clone();
        std::thread::spawn(move || reader.read_touchscreen());
        if let Some((device, map)) = device {
            let reader = dispatcher.clone();
            std::thread::spawn(move || reader.read_keyboard(device, map, policy));
            let repeater = dispatcher.clone();
            std::thread::spawn(move || repeater.repeat_keys());
        }
        dispatcher
    }

    /// `setInteractive`.
    pub fn set_interactive(&self, interactive: bool) {
        self.interactive
            .store(interactive, std::sync::atomic::Ordering::Relaxed);
    }

    /// The window policy, once the agent attached to the app host.
    pub fn set_policy(&self, policy: InputPolicy) {
        *self.policy.lock().unwrap() = Some(policy);
    }

    /// `interceptKeyBeforeQueueing` of a device's key of `policy_flags`:
    /// whether it is dispatched. Interactive, all are but the up of the key
    /// that woke the device; else the window policy decides.
    fn intercept_key(&self, event: &KeyEvent, policy_flags: u32) -> bool {
        let down = event.action != keyboard::ACTION_UP;
        let mut pending = self.pending_wake_key.lock().unwrap();
        if self.interactive.load(std::sync::atomic::Ordering::Relaxed) {
            let woke = pending.take() == Some(event.key_code) && !down;
            return !woke;
        }
        let Some(policy) = &*self.policy.lock().unwrap() else {
            return true;
        };
        let result = policy(false, event.key_code, down, event.event_time, policy_flags);
        *pending = (result & PENDING_WAKE_KEY != 0).then_some(event.key_code);
        result & ACTION_PASS_TO_USER != 0
    }

    /// A device's key of `policy_flags`: dispatched, unless the policy
    /// consumes it.
    fn key_from_device(&self, key: KeyEvent, policy_flags: u32) {
        if self.intercept_key(&key, policy_flags) {
            self.dispatch_key(key);
        } else {
            self.drop_key(&key);
        }
    }

    /// A touchscreen motion, waking if `wake`: dispatched unless the
    /// policy consumes it or the input filter takes it.
    fn motion_from_device(&self, motion: &Motion, wake: bool) {
        if !self.intercept_motion(motion, wake) {
            self.drop_motion(motion);
        } else if !self.filter_input(&motion.message(0, self.ids().1, (0., 0.))) {
            self.dispatch(&mut self.windows.lock().unwrap(), motion);
        }
    }

    /// `interceptMotionBeforeQueueing` of a touchscreen motion, waking if
    /// `wake` (`POLICY_FLAG_WAKE`): whether it is dispatched.
    fn intercept_motion(&self, motion: &Motion, wake: bool) -> bool {
        if self.interactive.load(std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        let Some(policy) = &*self.policy.lock().unwrap() else {
            return true;
        };
        let flags = if wake { POLICY_FLAG_WAKE } else { 0 };
        policy(true, 0, motion.initial_down(), motion.time(), flags) & ACTION_PASS_TO_USER != 0
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
        self.dispatch_unfiltered_key(event);
    }

    /// A key goes to the focused window (`dispatchKeyLocked`), its repeat
    /// state kept.
    fn dispatch_key(&self, event: KeyEvent) {
        if self.filter_input(&event.message(0, self.ids().1)) {
            return;
        }
        self.dispatch_unfiltered_key(event);
    }

    /// The repeat state is kept until the key is sent, so a repeat due
    /// meanwhile cannot follow its up or cancellation.
    fn dispatch_unfiltered_key(&self, event: KeyEvent) {
        let mut repeat = self.repeat.lock().unwrap();
        let config = repeat.1;
        let event = repeat.0.dispatch(config, event);
        self.repeat_changed.notify_all();
        self.send_key(&event);
    }

    /// Publishes `event` to the focused window, if any, which holds a
    /// key's down until its up (`InputState::trackKey`).
    fn send_key(&self, event: &KeyEvent) {
        let windows = &mut *self.windows.lock().unwrap();
        let Some(target) = windows.stack.last() else {
            return;
        };
        let index = target.index;
        let (seq, id) = self.ids();
        send(&target.channel, &event.message(seq, id));
        windows.keys.retain(|(i, k)| {
            !(*i == index
                && (k.device, k.source, k.display, k.key_code, k.scan_code)
                    == (
                        event.device,
                        event.source,
                        event.display,
                        event.key_code,
                        event.scan_code,
                    ))
        });
        if event.action != keyboard::ACTION_UP {
            windows.keys.push((index, event.clone()));
            windows.interactions = vec![index];
        }
    }

    /// `dropInboundEventLocked` of a device's key the policy consumed: the
    /// key repeat is reset, and the keys of its device and display down in
    /// a window are canceled (`CANCEL_NON_POINTER_EVENTS`).
    fn drop_key(&self, event: &KeyEvent) {
        let mut repeat = self.repeat.lock().unwrap();
        repeat.0.reset();
        self.repeat_changed.notify_all();
        let windows = &mut *self.windows.lock().unwrap();
        let (canceled, kept) = std::mem::take(&mut windows.keys)
            .into_iter()
            .partition(|(_, k)| (k.device, k.display) == (event.device, event.display));
        windows.keys = kept;
        let now = monotonic_ns();
        for (index, key) in canceled {
            if let Some(target) = windows.stack.iter().find(|t| t.index == index) {
                let (seq, id) = self.ids();
                send(&target.channel, &key.canceled(now).message(seq, id));
            }
        }
    }

    /// `dropInboundEventLocked` of a touchscreen motion the policy
    /// consumed: the gesture of its device under way is canceled
    /// (`CANCEL_POINTER_EVENTS`).
    fn drop_motion(&self, motion: &Motion) {
        let windows = &mut *self.windows.lock().unwrap();
        if windows
            .gesture_motion
            .as_ref()
            .is_none_or(|m| m.device() != motion.device())
        {
            return;
        }
        let last = windows.gesture_motion.take().unwrap();
        let gesture = windows.gesture.take();
        if let Some(target) = windows.stack.iter().find(|t| Some(t.index) == gesture) {
            let (seq, id) = self.ids();
            let cancel = last.canceled(monotonic_ns());
            send(&target.channel, &cancel.message(seq, id, target.origin()));
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
                self.send_key(&event);
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
    /// gives) and dispatched unless `policy` consumes it; autorepeats
    /// (value 2) are dropped.
    fn read_keyboard(&self, mut device: File, map: KeyMap, policy: KeyPolicy) {
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
                    if let Some((key, layout_flags)) =
                        mapper.process_key(&map, time, value != 0, code.into(), usage_code)
                        && !policy(&key)
                    {
                        // `KeyboardInputMapper::processKey`'s `POLICY_FLAG_WAKE`.
                        let down = value != 0;
                        let mut flags = layout_flags & POLICY_FLAG_WAKE;
                        if down
                            && info.wakes
                            && !(info.keyboard_type != keyboard::KEYBOARD_TYPE_ALPHABETIC
                                && keyboard::is_media_key(key.key_code))
                        {
                            flags |= POLICY_FLAG_WAKE;
                        }
                        self.key_from_device(key, flags);
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
        let channel = injection::Channel::new(channel);
        let drained = channel.clone();
        let dispatcher = self.clone();
        std::thread::spawn(move || {
            drain(&drained, &dispatcher, index);
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
        windows.interactions.retain(|&i| i != index);
        windows.keys.retain(|&(i, _)| i != index);
        if windows.gesture == Some(index) {
            windows.gesture = None;
            windows.gesture_motion = None;
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
            if motion.interacts() {
                windows.interactions = vec![target.index];
            }
        }
        windows.gesture_motion = windows.gesture.map(|_| motion.clone());
        if motion.ends_gesture() {
            windows.gesture = None;
            windows.gesture_motion = None;
        }
    }

    /// `setInTouchMode`: `display` takes touch mode `in_touch_mode`, unless
    /// it has it, or the caller (`pid`, `uid`) has no permission and owns
    /// neither the focused window nor one interacted with last; its
    /// windows are told. Whether the mode changed.
    pub fn set_in_touch_mode(
        &self,
        in_touch_mode: bool,
        (pid, uid): (i32, i32),
        has_permission: bool,
        display: i32,
    ) -> bool {
        let mut windows = self.windows.lock().unwrap();
        if windows.touch_mode.get(&display) == Some(&in_touch_mode) {
            return false;
        }
        let owned = |t: &Target| t.region.owner_pid == pid && t.region.owner == uid;
        if !has_permission
            && !windows.stack.last().is_some_and(owned)
            && !windows
                .stack
                .iter()
                .any(|t| windows.interactions.contains(&t.index) && owned(t))
        {
            log::debug!(
                "Touch mode switch rejected, caller (pid={pid}, uid={uid}) doesn't own the focused window nor none of the previously interacted window"
            );
            return false;
        }
        windows.touch_mode.insert(display, in_touch_mode);
        if display == DEFAULT_DISPLAY {
            for target in &windows.stack {
                let (seq, id) = self.ids();
                send(&target.channel, &touch_mode(seq, id, in_touch_mode));
            }
        }
        true
    }

    /// Reads the touchscreen, as long as the agent runs.
    fn read_touchscreen(&self) {
        let (mut device, x_axis, y_axis, wake) = match open_touchscreen() {
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
                        let display = self.windows.lock().unwrap().display;
                        let scale = (display.0 / x_axis.1 as f32, display.1 / y_axis.1 as f32);
                        for motion in touch.report(time, x_axis.0, y_axis.0, scale) {
                            // `TouchInputMapper`: an initial down wakes. The
                            // policy is asked without the windows' lock, as
                            // waking changes them.
                            self.motion_from_device(&motion, wake && motion.initial_down());
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
/// classifies a touchscreen), on the touchscreen's clock, its x and y
/// axes' minimum and extent, and whether its initial downs wake the
/// device (`TouchInputMapper`'s `touch.wake`, by default if external).
fn open_touchscreen() -> std::io::Result<Option<(File, (i32, i32), (i32, i32), bool)>> {
    let mut paths: Vec<_> = std::fs::read_dir("/dev/input")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("event"))
        })
        .collect();
    paths.sort();
    let readable = |path: &str| std::fs::metadata(path).is_ok_and(|m| m.is_file());
    let files = config_files(&readable);
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
        let (identifier, config) = identify(fd, &files);
        let wake = keyboard::property_bool(&config, "touch.wake")
            .unwrap_or_else(|| is_external(&identifier, &config));
        log::info!("touchscreen {}", path.display());
        return Ok(Some((
            device,
            axis(ABS_MT_POSITION_X)?,
            axis(ABS_MT_POSITION_Y)?,
            wake,
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
    unsafe extern "C" {
        fn __system_property_get(
            name: *const libc::c_char,
            value: *mut libc::c_char,
        ) -> libc::c_int;
    }
    let n = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr().cast()) };
    String::from_utf8_lossy(&value[..n.max(0) as usize]).into_owned()
}

/// The input device configuration files' places (`getInputDeviceConfigurationFilePath*`).
fn config_files(readable: &dyn Fn(&str) -> bool) -> ConfigFiles<'_> {
    let apex = property(c"input_device.config_file.apex");
    let root = std::env::var("ANDROID_ROOT").ok();
    let data = std::env::var("ANDROID_DATA").unwrap_or_default();
    ConfigFiles {
        prefixes: ConfigFiles::prefixes(Some(&apex), root.as_deref(), &data),
        readable,
    }
}

/// A device's identifier and its input device configuration (`.idc`),
/// as EventHub reads them (`openDeviceLocked`, `loadConfigurationLocked`).
fn identify(fd: libc::c_int, files: &ConfigFiles<'_>) -> (Identifier, Vec<(String, String)>) {
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
        Some(idc) => match std::fs::read(&idc).map(|c| keyboard::property_map(&c)) {
            Ok(Ok(config)) => config,
            Ok(Err(error)) => {
                log::error!("{idc}: {error}");
                Vec::new()
            }
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    };
    (identifier, config)
}

/// `isExternalDeviceLocked`: the configuration's `device.internal`, else
/// a USB or Bluetooth bus.
fn is_external(identifier: &Identifier, config: &[(String, String)]) -> bool {
    match keyboard::property_bool(config, "device.internal") {
        Some(internal) => !internal,
        None => matches!(identifier.bus, BUS_USB | BUS_BLUETOOTH),
    }
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
    let readable = |path: &str| std::fs::metadata(path).is_ok_and(|m| m.is_file());
    let files = config_files(&readable);
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
        let (identifier, config) = identify(fd, &files);
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
        let keyboard = Keyboard {
            map: &map,
            has_key: &has_key,
            has_msc_scan,
        };
        let classes = keyboard.classes();
        let supported_keys = keyboard.supported_key_codes();
        if !keyboard::eligible_built_in(&identifier, &config, &map) {
            log::info!(
                "{}: a keyboard other than the built-in one is not read (#1434)",
                identifier.name
            );
            continue;
        }
        let is_external = is_external(&identifier, &config);
        // `KeyboardInputMapper`: an external keyboard's downs wake the
        // device unless it says not to (`keyboard.doNotWakeByDefault`).
        let wakes = is_external
            && !keyboard::property_bool(&config, "keyboard.doNotWakeByDefault").unwrap_or(false);
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
                wakes,
                key_character_map,
                supported_keys,
            },
            device,
            map,
        }));
    }
    Ok(None)
}

fn send(channel: &injection::Channel, message: &[u8]) {
    channel.enqueue(message.to_vec(), None);
}

/// Reads what the window sends back until it closes its end.
fn drain(channel: &injection::Channel, dispatcher: &Dispatcher, index: i32) {
    let mut buffer = [0u8; 256];
    loop {
        let n = {
            let socket = channel.socket.read().unwrap();
            let Some(socket) = socket.as_ref() else {
                return;
            };
            // SAFETY: a receive into a local buffer; close waits for this borrow.
            unsafe {
                libc::recv(
                    socket.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                )
            }
        };
        if n == 24 && u32::from_ne_bytes(buffer[..4].try_into().unwrap()) == 2 {
            dispatcher.finished(index, u32::from_ne_bytes(buffer[4..8].try_into().unwrap()));
        }
        if n == 0
            || (n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted)
        {
            return;
        }
    }
}

// Forks in concurrent tests retain even CLOEXEC sockets until exec. On Darwin
// a datagram recv waiting at the last close can time out before ECONNRESET.
// Keep descriptor-lifetime fixtures alone, including when services compiles us.
#[cfg(test)]
pub(crate) fn isolated_test() -> bool {
    const MARKER: &str = "isolated-input-fixture";
    if std::env::args().any(|arg| arg == MARKER) {
        return false;
    }
    let thread = std::thread::current();
    let name = thread.name().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--skip", MARKER, "--nocapture"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains(&format!("test {name} ... ok")), "{stdout}");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_input_transport::{ABS_MT_SLOT, ABS_MT_TRACKING_ID};
    use std::os::unix::net::UnixDatagram;

    /// A dispatcher whose window policy consumes all input, and the client
    /// end of its one window.
    fn window() -> (Arc<Dispatcher>, UnixDatagram) {
        let dispatcher = Dispatcher::new(|_| false);
        dispatcher.set_policy(Box::new(|_, _, _, _, _| 0));
        let (server, client) = UnixDatagram::pair().unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let frame = [0, 0, 100, 100];
        let region = Region {
            frame,
            touchable: frame,
            watch_outside: false,
            owner: 10001,
            owner_pid: 42,
        };
        dispatcher.add_window(0, server.into(), (100, 100), region);
        recv(&client); // focus
        (dispatcher, client)
    }

    fn recv(client: &UnixDatagram) -> Vec<u8> {
        let mut bytes = [0; 512];
        let n = client.recv(&mut bytes).unwrap();
        bytes[..n].to_vec()
    }

    /// A message's sequence number, event id and event time.
    fn header(m: &[u8]) -> (u32, i32, i64) {
        (
            u32::from_ne_bytes(m[4..8].try_into().unwrap()),
            i32::from_ne_bytes(m[8..12].try_into().unwrap()),
            i64::from_ne_bytes(m[16..24].try_into().unwrap()),
        )
    }

    fn nothing(client: &UnixDatagram) {
        client.set_nonblocking(true).unwrap();
        let error = client.recv(&mut [0; 512]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    /// A key held while the device goes to sleep: its dropped up cancels
    /// it in the window that has it, and its repeat stops.
    #[test]
    fn a_dropped_key_cancels_the_keys_held_and_their_repeat() {
        let (dispatcher, client) = window();
        let key = |action, time| KeyEvent {
            device: keyboard::BUILT_IN_KEYBOARD_ID,
            source: keyboard::SOURCE_KEYBOARD,
            display: keyboard::DISPLAY_INVALID,
            action,
            flags: keyboard::FLAG_FROM_SYSTEM,
            key_code: 29,
            scan_code: 30,
            meta_state: 0,
            repeat_count: 0,
            down_time: 1,
            event_time: time,
        };
        let down = key(keyboard::ACTION_DOWN, 1);
        dispatcher.key_from_device(down.clone(), 0);
        let (seq, id, _) = header(&recv(&client));
        assert!(dispatcher.repeat.lock().unwrap().0.next().is_some());
        dispatcher.set_interactive(false);
        dispatcher.key_from_device(key(keyboard::ACTION_UP, 2), 0);
        let canceled = recv(&client);
        let (seq, id, time) = (seq + 1, id + 1, header(&canceled).2);
        assert_eq!(canceled, down.canceled(time).message(seq, id));
        assert!(dispatcher.repeat.lock().unwrap().0.next().is_none());
        dispatcher.key_from_device(key(keyboard::ACTION_DOWN, 3), 0);
        nothing(&client);
    }

    /// A touch under way while the device goes to sleep: its dropped move
    /// cancels the gesture, and the rest of it is dropped.
    #[test]
    fn a_dropped_motion_cancels_the_gesture_under_way() {
        let (dispatcher, client) = window();
        let mut touch = Touch::default();
        let mut report = |time, x| {
            for (code, value) in [
                (ABS_MT_SLOT, 0),
                (ABS_MT_TRACKING_ID, 1),
                (ABS_MT_POSITION_X, x),
                (ABS_MT_POSITION_Y, 10),
            ] {
                touch.abs(code, value);
            }
            touch.report(time, 0, 0, (1., 1.)).remove(0)
        };
        let down = report(1, 10);
        dispatcher.motion_from_device(&down, false);
        let (seq, id, _) = header(&recv(&client));
        dispatcher.set_interactive(false);
        dispatcher.motion_from_device(&report(2, 20), false);
        let canceled = recv(&client);
        let (seq, id, time) = (seq + 1, id + 1, header(&canceled).2);
        assert_eq!(canceled, down.canceled(time).message(seq, id, (0., 0.)));
        assert!(dispatcher.windows.lock().unwrap().gesture.is_none());
        touch.abs(ABS_MT_TRACKING_ID, -1);
        for up in touch.report(3, 0, 0, (1., 1.)) {
            dispatcher.motion_from_device(&up, false);
        }
        nothing(&client);
    }
}
