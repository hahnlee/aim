//! `window`: WindowManager for the app host's windows, as
//! `WindowManagerService.addWindow` and `relayoutWindow` answer an app's
//! window at `android-16.0.0_r1` (`IWindowSession`'s replies). A window is
//! a layer of buffers with no system bars: the whole display's, at its
//! root, or in window mode its task's (`tasks`), a child of the task's
//! layer; its input channel is a guest socket pair the app host agent
//! makes.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_view_iwindowid as wid;
use aim_service_aidl::android_view_iwindowmanager as wm;
use aim_service_aidl::android_view_iwindowsession as ws;

use super::activity::{ActivityWindowInfo, Item};
use super::config::{Configuration, Rect};
use super::surface::{self, Layer};
use super::{AppHost, reply};

/// `WindowManagerGlobal.ADD_FLAG_IN_TOUCH_MODE | ADD_FLAG_APP_VISIBLE`.
const ADDED: i32 = 0x1 | 0x2;
/// `WindowManagerGlobal.RELAYOUT_RES_FIRST_TIME`, `RELAYOUT_RES_SURFACE_CHANGED`.
const RELAYOUT_RES_FIRST_TIME: i32 = 1;
const RELAYOUT_RES_SURFACE_CHANGED: i32 = 1 << 1;
/// `View.VISIBLE`; `WindowManager.LayoutParams.TYPE_BASE_APPLICATION`.
const VISIBLE: i32 = 0;
const TYPE_BASE_APPLICATION: i32 = 1;
/// `PixelFormat.TRANSLUCENT`.
const TRANSLUCENT: i32 = -3;
/// The first sequence numbers of `InsetsState` and `ClientWindowFrames`
/// (`Integer.MIN_VALUE + 1`).
const FIRST_SEQ: i32 = i32::MIN + 1;
/// `InsetsSource.ID_IME`, `WindowInsets.Type.ime()`.
const ID_IME: i32 = 3;
const TYPE_IME: i32 = 1 << 3;

/// A window added through a session, its `IWindow` held.
struct Window {
    _client: aim_binder_host::local::Strong,
    id: u32,
    name: String,
    uid: u32,
    pid: i32,
    /// Its task, in window mode.
    task: Option<i32>,
    /// Its input channel's index at the agent.
    channel: i32,
    layer: Option<Layer>,
    /// The sequence numbers of its frames and insets last sent.
    frames_seq: i32,
    insets_seq: i32,
    /// The inset types it asks to be visible (`updateRequestedVisibleTypes`).
    requested_visible_types: i32,
    /// Its `IWindowId`, made at the first `getWindowId`.
    window_id: Option<Binder>,
}

/// The app host's windows, by their `IWindow`.
#[derive(Default)]
pub struct Windows {
    windows: Mutex<HashMap<u32, Window>>,
    next: Mutex<u32>,
}

/// `window`.
pub struct WindowManager(pub Arc<AppHost>);

impl Service for WindowManager {
    fn descriptor(&self) -> &str {
        wm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            wm::OPEN_SESSION => {
                wm::OpenSession::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let session = self.0.process.add_service(Arc::new(Session {
                    host: self.0.clone(),
                    uid: call.sender_euid,
                    pid: call.sender_pid,
                }));
                wm::write_open_session_reply(&mut p, Some(session));
            }
            wm::GET_CURRENT_ANIMATOR_SCALE => {
                wm::GetCurrentAnimatorScale::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `Settings.Global.ANIMATOR_DURATION_SCALE`, 1 by default.
                let scale = self.0.inputs().ok()
                    .and_then(|i| i.settings.get(super::settings::Table::Global, "animator_duration_scale").and_then(|v| v.parse().ok()))
                    .unwrap_or(1.0);
                wm::write_get_current_animator_scale_reply(&mut p, scale);
            }
            wm::HAS_NAVIGATION_BAR => {
                wm::HasNavigationBar::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // The Mac's window has no navigation bar.
                wm::write_has_navigation_bar_reply(&mut p, false);
            }
            code => return reply(Err(self.0.unimplemented("window", code))),
        }
        Ok(p)
    }
}

/// An app's `IWindowSession`.
struct Session {
    host: Arc<AppHost>,
    uid: u32,
    pid: i32,
}

impl Session {
    /// The window of a call whose first argument is its `IWindow`, and the
    /// call's trailing `int`s (after the layout parameters, which the app
    /// host does not read).
    fn window_and_tail(call: &mut Call<'_>, tail: usize) -> Result<(u32, Vec<i32>), i32> {
        call.data.enforce_interface(ws::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let Some(Binder::Handle(window)) = call.data.read_binder().map_err(|_| BAD_VALUE)? else {
            return Err(BAD_VALUE);
        };
        let mut r = call.data.clone();
        let end = r.position() + r.remaining();
        let start = end.checked_sub(4 * tail).ok_or(BAD_VALUE)?;
        r.set_position(start);
        let ints = (0..tail).map(|_| r.read_i32()).collect::<Result<Vec<_>, _>>().map_err(|_| BAD_VALUE)?;
        Ok((window, ints))
    }

    /// `addToDisplayAsUser`: the window's input channel, its insets, no
    /// insets controls, no attached frame, no size compatibility scale.
    fn add(&self, window: u32) -> Result<Parcel, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let name = self.host.activities.top_component(self.uid).ok_or_else(|| state("no activity of the window's app".into()))?;
        let id = {
            let mut next = self.host.windows.next.lock().unwrap();
            *next += 1;
            *next
        };
        let channel_name = format!("{id:07x} {name}");
        let display = self.host.display()?.0.clone();
        let task = self.host.tasks.of_uid(self.uid);
        let frame = task.map_or(Rect { left: 0, top: 0, right: display.width, bottom: display.height }, |(_, bounds)| bounds);
        let (index, channel) = self.host.open_input_channel(&channel_name, (display.width, display.height), frame)?;
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(ADDED);
        // The input channel: present, holding one, an `InputChannelCore`
        // (its name, the client's socket, its token).
        p.write_i32(1);
        p.write_i32(1);
        let size_at = p.position();
        p.write_i32(-1);
        p.write_string16(Some(&channel_name));
        channel.append_from(super::AGENT_CHANNEL_FD, &mut p).map_err(|s| state(format!("input channel: {s}")))?;
        p.write_binder(Some(self.host.process.add_service(Arc::new(super::activity::Token))));
        p.set_i32_at(size_at, (p.position() - size_at) as i32);
        p.write_i32(1);
        write_insets_state(&mut p, display.width, display.height, FIRST_SEQ);
        p.write_i32(1);
        p.write_i32(-1); // no insets source controls
        p.write_i32(FIRST_SEQ);
        p.write_i32(1);
        for v in [0, 0, -1, -1] {
            p.write_i32(v); // no attached frame
        }
        p.write_i32(1);
        p.write_f32(1.0); // the size compatibility scale
        self.host.windows.windows.lock().unwrap().insert(
            window,
            Window {
                _client: self.host.process.strong(window),
                id,
                name: name.clone(),
                uid: self.uid,
                pid: self.pid,
                task: task.map(|(id, _)| id),
                channel: index,
                layer: None,
                frames_seq: FIRST_SEQ,
                insets_seq: FIRST_SEQ,
                requested_visible_types: 0,
                window_id: None,
            },
        );
        eprintln!("app host: window {channel_name} added");
        Ok(p)
    }

    /// `relayout`: the window's frames, configuration, surface and
    /// insets; its layer is made the first time it is visible.
    fn relayout(&self, window: u32, view_visibility: i32) -> Result<Parcel, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let display = self.host.display()?.0.clone();
        let global = self.host.global_config(self.uid).ok_or_else(|| state("relayout of an unbound app".into()))?;
        let mut windows = self.host.windows.windows.lock().unwrap();
        let w = windows.get_mut(&window).ok_or_else(|| Exception::illegal_argument("relayout of an unknown window"))?;
        let mut result = 0;
        let task = w.task.and_then(|id| self.host.tasks.bounds(id).map(|bounds| (id, bounds)));
        if view_visibility == VISIBLE && w.layer.is_none() {
            let composer = self.host.composer()?;
            let metadata = [
                (surface::METADATA_WINDOW_TYPE, TYPE_BASE_APPLICATION),
                (surface::METADATA_OWNER_PID, w.pid),
                (surface::METADATA_OWNER_UID, w.uid as i32),
            ];
            let create = |parent: Option<&Layer>| composer.create_surface(&self.host.process, &w.name, surface::BUFFER_STATE, parent, &metadata);
            let layer = match task {
                Some((id, _)) => self.host.task_layer(id, |parent| create(Some(parent))),
                None => create(None),
            }
            .map_err(state)?;
            eprintln!("app host: window {} has layer {}", w.name, layer.name);
            w.layer = Some(layer);
            result |= RELAYOUT_RES_FIRST_TIME | RELAYOUT_RES_SURFACE_CHANGED;
        }
        w.insets_seq += 1;
        let (width, height) = (display.width, display.height);
        let frame = task.map_or(Rect { left: 0, top: 0, right: width, bottom: height }, |(_, bounds)| bounds);
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(result);
        p.write_i32(1);
        write_frames(&mut p, frame, (width, height), w.frames_seq);
        let mut overrides: Configuration = match task {
            Some((_, bounds)) => global.of_freeform_task(bounds),
            None => global.clone(),
        };
        overrides.window.activity_type = 1;
        write_merged(&mut p, &global, &overrides);
        // `SurfaceControl`.
        match &w.layer {
            Some(layer) => {
                p.write_string8(Some(&w.name));
                p.write_i32(0);
                p.write_i32(0);
                p.write_i32(1);
                p.write_binder(Some(self.host.composer()?.binder()));
                p.write_binder(Some(layer.handle));
                p.write_i32(layer.id);
                p.write_string16(Some(&layer.name));
                p.write_i32(layer.transform_hint);
                p.write_i32(0);
                p.write_i32(0);
                p.write_i32(TRANSLUCENT);
            }
            None => {
                p.write_string8(None);
                p.write_i32(0);
                p.write_i32(0);
                p.write_i32(0);
            }
        }
        write_insets_state(&mut p, width, height, w.insets_seq);
        p.write_i32(-1); // no insets source controls
        p.write_i32(w.insets_seq);
        p.write_i32(-1); // no sync requested
        p.write_i32(1);
        // `ActivityWindowInfo`: not embedded, the task's bounds.
        ActivityWindowInfo { embedded: false, task_bounds: frame, task_fragment_bounds: frame }.write(&mut p);
        Ok(p)
    }
}

/// `ClientWindowFrames`: the frame and the display's, no parent or
/// attached frame, unscaled, of sequence number `seq`.
fn write_frames(p: &mut Parcel, frame: Rect, (width, height): (i32, i32), seq: i32) {
    for r in [[frame.left, frame.top, frame.right, frame.bottom], [0, 0, width, height], [0, 0, 0, 0]] {
        r.iter().for_each(|v| p.write_i32(*v));
    }
    p.write_i32(0);
    p.write_bool(false);
    p.write_f32(1.0);
    p.write_i32(seq);
}

/// `MergedConfiguration`: global, override, merged (the override is
/// whole).
fn write_merged(p: &mut Parcel, global: &Configuration, overrides: &Configuration) {
    global.write_to(p);
    overrides.write_to(p);
    overrides.write_to(p);
}

/// `WindowStateResizeItem`: a window's new frames, configuration and
/// insets (`WindowState.reportResized`).
struct WindowStateResizeItem {
    window: Binder,
    frame: Rect,
    display: (i32, i32),
    frames_seq: i32,
    insets_seq: i32,
    global: Configuration,
    config: Configuration,
}

impl Item for WindowStateResizeItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.WindowStateResizeItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.window));
        p.write_i32(1);
        write_frames(p, self.frame, self.display, self.frames_seq);
        p.write_bool(false); // no draw to report
        p.write_i32(1);
        write_merged(p, &self.global, &self.config);
        p.write_i32(1);
        write_insets_state(p, self.display.0, self.display.1, self.insets_seq);
        p.write_bool(false); // no forced layout
        p.write_bool(false); // system bars not always consumed
        p.write_i32(0); // the default display
        p.write_i32(-1); // no sync
        p.write_bool(false); // not drag resizing
        p.write_i32(1);
        ActivityWindowInfo { embedded: false, task_bounds: self.frame, task_fragment_bounds: self.frame }.write(p);
    }
}

impl super::AppHost {
    /// The input channels of `uid`'s windows, latest last.
    fn channels_of(&self, uid: u32) -> Vec<i32> {
        let windows = self.windows.windows.lock().unwrap();
        let mut of_uid: Vec<_> = windows.values().filter(|w| w.uid == uid).map(|w| (w.id, w.channel)).collect();
        of_uid.sort();
        of_uid.into_iter().map(|(_, channel)| channel).collect()
    }

    /// `uid`'s latest window takes the input focus.
    pub(super) fn focus_window_of(&self, uid: u32) -> Result<(), String> {
        match self.channels_of(uid).last() {
            Some(channel) => self.focus_input_channel(*channel).map_err(|e| format!("{e:?}")),
            None => Ok(()),
        }
    }

    /// `uid`'s windows moved to `frame`.
    pub(super) fn move_windows_of(&self, uid: u32, frame: Rect) -> Result<(), String> {
        for channel in self.channels_of(uid) {
            self.set_input_frame(channel, frame).map_err(|e| format!("{e:?}"))?;
        }
        Ok(())
    }

    /// The items that tell `uid`'s windows their task took `frame`, on a
    /// display of the global configuration `global`.
    pub(super) fn resize_windows(&self, uid: u32, global: &Configuration, frame: Rect) -> Result<Vec<Box<dyn Item>>, String> {
        let display = self.display().map_err(|e| format!("{e:?}"))?.0.clone();
        let mut config = global.of_freeform_task(frame);
        config.window.activity_type = 1;
        let mut windows = self.windows.windows.lock().unwrap();
        Ok(windows
            .iter_mut()
            .filter(|(_, w)| w.uid == uid)
            .map(|(&window, w)| {
                w.frames_seq += 1;
                w.insets_seq += 1;
                Box::new(WindowStateResizeItem {
                    window: Binder::Handle(window),
                    frame,
                    display: (display.width, display.height),
                    frames_seq: w.frames_seq,
                    insets_seq: w.insets_seq,
                    global: global.clone(),
                    config: config.clone(),
                }) as Box<dyn Item>
            })
            .collect())
    }

    /// The windows of the process `pid` are gone with it.
    pub(super) fn windows_died(&self, pid: i32) {
        self.windows.windows.lock().unwrap().retain(|_, w| w.pid != pid);
    }
}

impl Session {
    /// `getWindowId`: the window's `IWindowId`, the same each time.
    fn window_id(&self, window: u32) -> Result<Binder, Exception> {
        let mut windows = self.host.windows.windows.lock().unwrap();
        let w = windows.get_mut(&window).ok_or_else(|| Exception::illegal_argument("getWindowId of an unknown window"))?;
        Ok(*w.window_id.get_or_insert_with(|| {
            self.host.process.add_service(Arc::new(WindowId { host: self.host.clone(), window, observers: Mutex::new(Vec::new()) }))
        }))
    }
}

/// A window's `IWindowId`. Focus moves only when an activity of the
/// window's app pauses, which no observer is told of yet (#1422).
struct WindowId {
    host: Arc<AppHost>,
    window: u32,
    observers: Mutex<Vec<Binder>>,
}

impl WindowId {
    /// `WindowState.isFocused`: the latest window of the focused app.
    fn focused(&self) -> bool {
        let windows = self.host.windows.windows.lock().unwrap();
        let Some(w) = windows.get(&self.window) else { return false };
        self.host.activities.uid_focused(w.uid) && windows.values().filter(|o| o.uid == w.uid).all(|o| o.id <= w.id)
    }
}

impl Service for WindowId {
    fn descriptor(&self) -> &str {
        wid::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            wid::REGISTER_FOCUS_OBSERVER => {
                let args = wid::RegisterFocusObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.observers.lock().unwrap().extend(args.observer);
                wid::write_register_focus_observer_reply(&mut p);
            }
            wid::UNREGISTER_FOCUS_OBSERVER => {
                let args = wid::UnregisterFocusObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.observers.lock().unwrap().retain(|o| Some(*o) != args.observer);
                wid::write_unregister_focus_observer_reply(&mut p);
            }
            wid::IS_FOCUSED => {
                wid::IsFocused::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                wid::write_is_focused_reply(&mut p, self.focused());
            }
            code => return reply(Err(self.host.unimplemented("window id", code))),
        }
        Ok(p)
    }
}

/// `InsetsState` of a display without bars or cutout: the display's
/// frame, shape and an invisible IME source.
fn write_insets_state(p: &mut Parcel, width: i32, height: i32, seq: i32) {
    [0, 0, width, height].iter().for_each(|v| p.write_i32(*v));
    p.write_i32(0); // `DisplayCutout.NO_CUTOUT`
    p.write_i32(1);
    p.write_i32(0); // `RoundedCorners.NO_ROUNDED_CORNERS`
    [0, 0, width, height].iter().for_each(|v| p.write_i32(*v));
    p.write_i32(1); // `PrivacyIndicatorBounds`: four empty bounds, rotation 0
    p.write_i32(4);
    (0..4).for_each(|_| p.write_i32(0));
    p.write_i32(0);
    p.write_i32(1); // the display's shape
    p.write_string8(Some(&format!("M0,0 L{width},0 L{width},{height} L0,{height} Z")));
    p.write_i32(width);
    p.write_i32(height);
    p.write_f32(1.0);
    (0..3).for_each(|_| p.write_i32(0));
    p.write_f32(1.0);
    p.write_i32(seq);
    p.write_i32(1);
    p.write_i32(1);
    p.write_i32(ID_IME);
    p.write_i32(TYPE_IME);
    (0..4).for_each(|_| p.write_i32(0));
    p.write_i32(0); // no visible frame
    p.write_bool(false);
    p.write_i32(0); // flags
    p.write_i32(0); // side hint
    p.write_i32(-1); // no bounding rects
}

impl Service for Session {
    fn descriptor(&self) -> &str {
        ws::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // A finished draw's transaction may carry fences.
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            ws::ADD_TO_DISPLAY_AS_USER => {
                // ..., viewVisibility, layerStackId, userId, requestedVisibleTypes.
                let (window, _) = Self::window_and_tail(call, 4)?;
                reply(self.add(window))
            }
            ws::RELAYOUT => {
                // ..., requestedWidth, requestedHeight, viewVisibility, flags,
                // seq, lastSyncSeqId.
                let (window, tail) = Self::window_and_tail(call, 6)?;
                reply(self.relayout(window, tail[2]))
            }
            // A relayout whose results the window does not wait for
            // (oneway): its layer is made and its sequence numbers go on
            // as with `relayout`.
            ws::RELAYOUT_ASYNC => {
                let (window, tail) = Self::window_and_tail(call, 6)?;
                if let Err(e) = self.relayout(window, tail[2]) {
                    eprintln!("app host: relayoutAsync: {e:?}");
                }
                Ok(Parcel::new())
            }
            // Oneway: kept as `WindowState.setRequestedVisibleTypes` keeps
            // it; no inset source (the input method's) has a window here.
            ws::UPDATE_REQUESTED_VISIBLE_TYPES => {
                call.data.enforce_interface(ws::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
                let window = call.data.read_binder().map_err(|_| BAD_VALUE)?;
                let types = call.data.read_i32().map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(window)) = window
                    && let Some(w) = self.host.windows.windows.lock().unwrap().get_mut(&window)
                {
                    w.requested_visible_types = types;
                }
                Ok(Parcel::new())
            }
            ws::GET_WINDOW_ID => {
                let args = ws::GetWindowId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(Binder::Handle(window)) = args.window else { return Err(BAD_VALUE) };
                reply(self.window_id(window).map(|id| {
                    let mut p = Parcel::new();
                    ws::write_get_window_id_reply(&mut p, Some(id));
                    p
                }))
            }
            // The window's buffers reach SurfaceFlinger through the app's
            // own transactions; its draw transaction is not applied (#1431).
            ws::FINISH_DRAWING | ws::SET_ON_BACK_INVOKED_CALLBACK_INFO => Ok(Parcel::new()),
            // Oneway: WindowManager passes it to accessibility's
            // magnification (`AccessibilityController`), which no service
            // runs here.
            ws::ON_RECTANGLE_ON_SCREEN_REQUESTED => Ok(Parcel::new()),
            code => reply(Err(self.host.unimplemented("window session", code))),
        }
    }
}
