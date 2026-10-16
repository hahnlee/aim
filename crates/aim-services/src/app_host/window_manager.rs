//! `window`: WindowManager for the app host's windows, as
//! `WindowManagerService.addWindow` and `relayoutWindow` answer an app's
//! window at `android-16.0.0_r1` (`IWindowSession`'s replies). A window is
//! a layer of buffers at the display's root, the whole display, with no
//! system bars; its input channel is a guest socket pair the app host
//! agent makes.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_view_iwindowid as wid;
use aim_service_aidl::android_view_iwindowmanager as wm;
use aim_service_aidl::android_view_iwindowsession as ws;

use super::config::Configuration;
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
    layer: Option<Layer>,
    insets_seq: i32,
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
        let channel = self.host.open_input_channel(&channel_name)?;
        let display = self.host.display()?.0.clone();
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
                layer: None,
                insets_seq: FIRST_SEQ,
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
        if view_visibility == VISIBLE && w.layer.is_none() {
            let composer = self.host.composer()?;
            let layer = composer
                .create_surface(&self.host.process, &w.name, surface::BUFFER_STATE, &[
                    (surface::METADATA_WINDOW_TYPE, TYPE_BASE_APPLICATION),
                    (surface::METADATA_OWNER_PID, w.pid),
                    (surface::METADATA_OWNER_UID, w.uid as i32),
                ])
                .map_err(state)?;
            eprintln!("app host: window {} has layer {}", w.name, layer.name);
            w.layer = Some(layer);
            result |= RELAYOUT_RES_FIRST_TIME | RELAYOUT_RES_SURFACE_CHANGED;
        }
        w.insets_seq += 1;
        let (width, height) = (display.width, display.height);
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(result);
        p.write_i32(1);
        // `ClientWindowFrames`: the frame and the display's, no parent or
        // attached frame.
        for r in [[0, 0, width, height], [0, 0, width, height], [0, 0, 0, 0]] {
            r.iter().for_each(|v| p.write_i32(*v));
        }
        p.write_i32(0);
        p.write_bool(false);
        p.write_f32(1.0);
        p.write_i32(FIRST_SEQ);
        // `MergedConfiguration`: global, override, merged.
        let mut overrides: Configuration = global.clone();
        overrides.window.activity_type = 1;
        global.write_to(&mut p);
        overrides.write_to(&mut p);
        overrides.write_to(&mut p);
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
        p.write_bool(false); // `ActivityWindowInfo`: not embedded, the task's bounds
        for _ in 0..2 {
            [0, 0, width, height].iter().for_each(|v| p.write_i32(*v));
        }
        Ok(p)
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
            code => reply(Err(self.host.unimplemented("window session", code))),
        }
    }
}
