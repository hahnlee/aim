//! `uimode` (IUiModeManager): the UI mode and night mode as a native
//! service (ADR 0013), in place of SystemServer's UiModeManagerService.
//!
//! This is `UiModeManagerService.java` at the pinned tag, with the Mac as
//! the owner of night mode (docs/system-services.md, "The uimode
//! service"):
//!
//! - Night mode follows the Mac's appearance ([`mac`]): at start and on
//!   each change of it, the night mode becomes yes (Dark) or no (Light),
//!   persisted and applied at once, as the system setting it would. Android
//!   never changes the Mac's appearance.
//! - Night mode is managed by the system, as on a device whose
//!   `config_lockDayNightMode` is set: `isNightModeLocked` is true, and a
//!   request to change it without `MODIFY_DAY_NIGHT_MODE` is refused as the
//!   original refuses it; a request with it (Settings, SystemUI, the shell)
//!   is answered as the original answers it, until the Mac changes.
//! - Everything else is the original's: car and desk mode with their
//!   broadcasts, notification, dock apps, dreams and wake lock, power save's
//!   night mode, the twilight and custom schedules, attention mode,
//!   projection, contrast and force invert, per-app night mode, the
//!   `persist.sys.theme` property, the client caches, the dump and
//!   `cmd uimode`.
//!
//! What the original does inside system_server goes through the bridge
//! ([`bridge`]), which is attached when system services are ready, the
//! original's `PHASE_SYSTEM_SERVICES_READY`: until then no configuration
//! is computed and `getCurrentModeType` is undefined, as in the original.
//! No state lock is held across a call to system_server that changes
//! something: its effects run after, in order.
//!
//! Not here: visible background users (`enforceCurrentUserIfVisibleBackgroundEnabled`
//! decides nothing without them, as on this image).

mod bridge;
mod mac;
mod time;

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    android_app_ionprojectionstatechangedlistener as projection_listener,
    android_app_iuimodemanager as ium, android_app_iuimodemanagercallback as callback,
    dev_aim_server_ibridge as ibridge,
};

use self::bridge::{Bridge, Config, Setting};
use crate::settings::Settings;
use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};
use crate::system::System;

/// `UiModeManager.MODE_NIGHT_*`.
const MODE_NIGHT_AUTO: i32 = 0;
const MODE_NIGHT_NO: i32 = 1;
const MODE_NIGHT_YES: i32 = 2;
const MODE_NIGHT_CUSTOM: i32 = 3;
/// `UiModeManager.MODE_NIGHT_CUSTOM_TYPE_*`.
const CUSTOM_TYPE_UNKNOWN: i32 = -1;
const CUSTOM_TYPE_SCHEDULE: i32 = 0;
const CUSTOM_TYPE_BEDTIME: i32 = 1;
/// `UiModeManager.MODE_ATTENTION_THEME_OVERLAY_*`.
const ATTENTION_OFF: i32 = 1000;
const ATTENTION_NIGHT: i32 = 1001;
const ATTENTION_DAY: i32 = 1002;
/// `UiModeManager` car mode flags and priority.
const ENABLE_CAR_MODE_GO_CAR_HOME: i32 = 1;
const ENABLE_CAR_MODE_ALLOW_SLEEP: i32 = 2;
const DISABLE_CAR_MODE_GO_HOME: i32 = 1;
const DISABLE_CAR_MODE_ALL_PRIORITIES: i32 = 2;
const DEFAULT_PRIORITY: i32 = 0;
/// `UiModeManager.PROJECTION_TYPE_*`.
const PROJECTION_TYPE_NONE: i32 = 0;
const PROJECTION_TYPE_AUTOMOTIVE: i32 = 1;
/// `UiModeManager.FORCE_INVERT_TYPE_*`.
const FORCE_INVERT_OFF: i32 = 0;
const FORCE_INVERT_DARK: i32 = 1;
/// `Configuration.UI_MODE_*`.
const UI_MODE_TYPE_MASK: i32 = 0x0f;
const UI_MODE_TYPE_DESK: i32 = 0x02;
const UI_MODE_TYPE_CAR: i32 = 0x03;
const UI_MODE_TYPE_TELEVISION: i32 = 0x04;
const UI_MODE_TYPE_WATCH: i32 = 0x06;
const UI_MODE_TYPE_VR_HEADSET: i32 = 0x07;
const UI_MODE_NIGHT_NO: i32 = 0x10;
const UI_MODE_NIGHT_YES: i32 = 0x20;
/// `Intent.EXTRA_DOCK_STATE_*`.
const DOCK_UNDOCKED: i32 = 0;
const DOCK_DESK: i32 = 1;
const DOCK_CAR: i32 = 2;
const DOCK_LE_DESK: i32 = 3;
const DOCK_HE_DESK: i32 = 4;
/// The dock mode broadcasts and the categories of their apps.
const ACTION_ENTER_CAR_MODE: &str = "android.app.action.ENTER_CAR_MODE";
const ACTION_EXIT_CAR_MODE: &str = "android.app.action.EXIT_CAR_MODE";
const ACTION_ENTER_DESK_MODE: &str = "android.app.action.ENTER_DESK_MODE";
const ACTION_EXIT_DESK_MODE: &str = "android.app.action.EXIT_DESK_MODE";
const CATEGORY_CAR_DOCK: &str = "android.intent.category.CAR_DOCK";
const CATEGORY_DESK_DOCK: &str = "android.intent.category.DESK_DOCK";
const CATEGORY_HOME: &str = "android.intent.category.HOME";
/// `Activity.RESULT_OK`.
const RESULT_OK: i32 = -1;
/// The settings the service reads and writes (`Settings.Secure`).
const UI_NIGHT_MODE: &str = "ui_night_mode";
const UI_NIGHT_MODE_CUSTOM_TYPE: &str = "ui_night_mode_custom_type";
const UI_NIGHT_MODE_OVERRIDE_ON: &str = "ui_night_mode_override_on";
const UI_NIGHT_MODE_OVERRIDE_OFF: &str = "ui_night_mode_override_off";
const UI_NIGHT_MODE_LAST_COMPUTED: &str = "ui_night_mode_last_computed";
const DARK_THEME_CUSTOM_START_TIME: &str = "dark_theme_custom_start_time";
const DARK_THEME_CUSTOM_END_TIME: &str = "dark_theme_custom_end_time";
const USER_SETUP_COMPLETE: &str = "user_setup_complete";
const CONTRAST_LEVEL: &str = "contrast_level";
const FORCE_INVERT_COLOR_ENABLED: &str = "accessibility_force_invert_color_enabled";
/// The URIs of the observed settings.
const OBSERVED: &[(&str, Setting, i32)] = &[
    (
        "content://settings/secure/ui_night_mode",
        Setting::NightMode,
        USER_SYSTEM,
    ),
    (
        "content://settings/secure/accessibility_force_invert_color_enabled",
        Setting::ForceInvert,
        USER_ALL,
    ),
    (
        "content://settings/secure/contrast_level",
        Setting::Contrast,
        USER_ALL,
    ),
];
const SETUP_COMPLETE_URI: &str = "content://settings/secure/user_setup_complete";
/// The permissions it checks.
const MODIFY_DAY_NIGHT_MODE: &str = "android.permission.MODIFY_DAY_NIGHT_MODE";
const ENTER_CAR_MODE_PRIORITIZED: &str = "android.permission.ENTER_CAR_MODE_PRIORITIZED";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const TOGGLE_AUTOMOTIVE_PROJECTION: &str = "android.permission.TOGGLE_AUTOMOTIVE_PROJECTION";
const READ_PROJECTION_STATE: &str = "android.permission.READ_PROJECTION_STATE";
const DUMP: &str = "android.permission.DUMP";
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");
/// `UserHandle.USER_SYSTEM`, `USER_ALL` and `PER_USER_RANGE`.
const USER_SYSTEM: i32 = 0;
const USER_ALL: i32 = -1;
const PER_USER_RANGE: i32 = 100_000;
/// `Process.SYSTEM_UID` and `SHELL_UID`.
const SYSTEM_UID: i32 = 1000;
const SHELL_UID: i32 = 2000;
/// The original's defaults of the custom schedule: 22:00 to 06:00.
const DEFAULT_CUSTOM_START: i64 = 22 * 3_600_000_000;
const DEFAULT_CUSTOM_END: i64 = 6 * 3_600_000_000;

type Result<T> = std::result::Result<T, Exception>;

fn bad_parcel(status: i32) -> Exception {
    Exception::illegal_argument(format!("bad parcel: status {status}"))
}

fn null_binder() -> Exception {
    Exception::new(
        EX_NULL_POINTER,
        "Attempt to invoke interface method 'android.os.IBinder \
         android.os.IInterface.asBinder()' on a null object reference",
    )
}

fn is_desk(dock: i32) -> bool {
    matches!(dock, DOCK_DESK | DOCK_LE_DESK | DOCK_HE_DESK)
}

/// `UiModeManagerService.Shell.nightModeToStr`.
fn night_mode_str(mode: i32, custom_type: i32) -> &'static str {
    match (mode, custom_type) {
        (MODE_NIGHT_YES, _) => "yes",
        (MODE_NIGHT_NO, _) => "no",
        (MODE_NIGHT_AUTO, _) => "auto",
        (MODE_NIGHT_CUSTOM, CUSTOM_TYPE_SCHEDULE) => "custom_schedule",
        (MODE_NIGHT_CUSTOM, CUSTOM_TYPE_BEDTIME) => "custom_bedtime",
        _ => "unknown",
    }
}

/// The caller of a method, as `Binder.getCallingUid()` and `Pid()`.
#[derive(Clone, Copy)]
struct Caller {
    uid: i32,
    pid: i32,
}

impl Caller {
    fn user(self) -> i32 {
        self.uid.div_euclid(PER_USER_RANGE)
    }
}

/// A registered callback (`RemoteCallbackList`).
struct Listener {
    strong: Strong,
    death: u64,
}

/// A package holding a projection type (`ProjectionHolder`).
struct Holder {
    package: String,
    strong: Strong,
    death: u64,
}

/// What a change does in system_server, after the state lock.
enum Effect {
    /// `applyConfigurationExternallyLocked`'s update.
    Configuration(i32),
    /// `sendConfigurationAndStartDreamOrDockAppLocked` from its dock app
    /// on: the app (unless `shouldStartDockApp` refuses it), the
    /// configuration if it changed, then a dream if no app started.
    DockApp {
        category: Option<&'static str>,
        may_start_app: bool,
        ui_mode: i32,
        apply: bool,
        start_dream_immediately: bool,
        dreams_disabled: bool,
    },
    InvalidateNightMode,
    InvalidateCurrentModeType,
    DeviceTheme(String),
    PutSecure(&'static str, String, i32),
    TwilightListening(bool),
    CarModeBroadcast(bool, i32, Option<String>),
    ForegroundBroadcast(&'static str),
    DockBroadcast(&'static str, i32, i32),
    CarModeStatusBar(bool),
    KeepScreenOn(bool),
}

struct State {
    config: Config,
    system_ready: bool,
    /// The Mac's appearance, last read.
    mac_dark: bool,
    dock_state: i32,
    last_broadcast_state: i32,
    night_mode: i32,
    custom_type: i32,
    attention: i32,
    /// The custom schedule, in microseconds of the day.
    custom_start: i64,
    custom_end: i64,
    /// `mCarModePackagePriority`, by priority.
    car_mode_packages: BTreeMap<i32, Option<String>>,
    car_mode_enabled: bool,
    charging: bool,
    power_save: bool,
    wait_for_device_inactive: bool,
    vr_headset: bool,
    computed_night_mode: bool,
    last_bedtime_requested: bool,
    car_mode_enable_flags: i32,
    setup_complete: bool,
    cur_ui_mode: i32,
    set_ui_mode: i32,
    holding_configuration: bool,
    current_user: i32,
    /// `mConfiguration.uiMode`.
    configuration: i32,
    override_on: bool,
    override_off: bool,
    override_user: i32,
    /// Whether the full wake lock is held (`mWakeLock.isHeld()`).
    keeping_screen_on: bool,
    twilight_listening: bool,
    /// The time change receiver of the custom schedule is registered.
    time_changes: bool,
    /// The custom schedule's alarm (`mCustomTimeListener`), by generation.
    alarm: u64,
    callbacks: BTreeMap<i32, Vec<Listener>>,
    projection_holders: BTreeMap<i32, Vec<Holder>>,
    projection_listeners: BTreeMap<i32, Vec<Listener>>,
    contrasts: BTreeMap<i32, f32>,
    force_invert: BTreeMap<i32, i32>,
    /// Batches of effects handed out, to apply in order.
    tickets: u64,
}

/// The effects' turn: batches apply one at a time, in the order their
/// state changes were made.
#[derive(Default)]
struct Turn {
    serving: Mutex<u64>,
    next: Condvar,
}

pub struct UiModeManagerService {
    process: Arc<LocalProcess>,
    system: Arc<System>,
    settings: Arc<Settings>,
    this: Weak<Self>,
    state: Mutex<State>,
    turn: Turn,
    bridge: Mutex<Option<Arc<Bridge>>>,
    /// The service's `IUiModeHost`, handed to the bridge.
    host: Binder,
    /// Wakes the alarm thread when the custom schedule's alarm changes.
    alarm: Condvar,
    alarm_at: Mutex<Option<(Instant, u64)>>,
}

impl UiModeManagerService {
    pub fn new(
        process: Arc<LocalProcess>,
        system: Arc<System>,
        settings: Arc<Settings>,
    ) -> Arc<Self> {
        let mac_dark = mac::dark();
        let service = Arc::new_cyclic(|this: &Weak<Self>| {
            let host = process.add_service(Arc::new(bridge::Host {
                service: this.clone(),
            }));
            UiModeManagerService {
                process,
                system,
                settings,
                this: this.clone(),
                state: Mutex::new(State {
                    config: Config {
                        default_ui_mode_type: 1,
                        car_mode_keeps_screen_on: false,
                        desk_mode_keeps_screen_on: false,
                        start_dream_immediately_on_dock: true,
                        dreams_disabled_by_ambient_mode_suppression: false,
                        enable_car_dock_launch: true,
                        ui_mode_locked: false,
                        television: false,
                        car: false,
                        watch: false,
                        force_invert_color: false,
                    },
                    system_ready: false,
                    mac_dark,
                    dock_state: DOCK_UNDOCKED,
                    last_broadcast_state: DOCK_UNDOCKED,
                    night_mode: if mac_dark {
                        MODE_NIGHT_YES
                    } else {
                        MODE_NIGHT_NO
                    },
                    custom_type: CUSTOM_TYPE_UNKNOWN,
                    attention: ATTENTION_OFF,
                    custom_start: DEFAULT_CUSTOM_START,
                    custom_end: DEFAULT_CUSTOM_END,
                    car_mode_packages: BTreeMap::new(),
                    car_mode_enabled: false,
                    charging: false,
                    power_save: false,
                    wait_for_device_inactive: false,
                    vr_headset: false,
                    computed_night_mode: false,
                    last_bedtime_requested: false,
                    car_mode_enable_flags: 0,
                    setup_complete: false,
                    cur_ui_mode: 0,
                    set_ui_mode: 0,
                    holding_configuration: false,
                    current_user: USER_SYSTEM,
                    configuration: 0,
                    override_on: false,
                    override_off: false,
                    override_user: USER_SYSTEM,
                    keeping_screen_on: false,
                    twilight_listening: false,
                    time_changes: false,
                    alarm: 0,
                    callbacks: BTreeMap::new(),
                    projection_holders: BTreeMap::new(),
                    projection_listeners: BTreeMap::new(),
                    contrasts: BTreeMap::new(),
                    force_invert: BTreeMap::new(),
                    tickets: 0,
                }),
                turn: Turn::default(),
                bridge: Mutex::new(None),
                host,
                alarm: Condvar::new(),
                alarm_at: Mutex::new(None),
            }
        });
        let weak = Arc::downgrade(&service);
        service.system.add_bridge_listener(Box::new(move |handle| {
            if let Some(service) = weak.upgrade() {
                service.attach(handle);
            }
        }));
        let weak = Arc::downgrade(&service);
        mac::watch(Box::new(move |dark| {
            if let Some(service) = weak.upgrade() {
                service.mac_changed(dark);
            }
        }));
        let weak = Arc::downgrade(&service);
        std::thread::Builder::new()
            .name("uimode-alarm".into())
            .spawn(move || alarms(weak))
            .expect("spawn the uimode alarm thread");
        service
    }

    fn bridge(&self) -> Option<Arc<Bridge>> {
        self.bridge.lock().unwrap().clone()
    }

    /// Runs `f` under the state lock, then its effects, in their turn.
    fn with<T>(&self, f: impl FnOnce(&Self, &mut State, &mut Vec<Effect>) -> T) -> T {
        let mut fx = Vec::new();
        let (result, ticket) = {
            let mut s = self.state.lock().unwrap();
            let result = f(self, &mut s, &mut fx);
            let ticket = (!fx.is_empty()).then(|| {
                s.tickets += 1;
                s.tickets - 1
            });
            (result, ticket)
        };
        if let Some(ticket) = ticket {
            let mut serving = self.turn.serving.lock().unwrap();
            while *serving != ticket {
                serving = self.turn.next.wait(serving).unwrap();
            }
            drop(serving);
            self.apply(fx);
            *self.turn.serving.lock().unwrap() += 1;
            self.turn.next.notify_all();
        }
        result
    }

    fn apply(&self, fx: Vec<Effect>) {
        let bridge = self.bridge();
        let mut invalidated = (false, false);
        for effect in fx {
            match effect {
                Effect::PutSecure(name, value, user) => {
                    if let Err(e) = self.settings.put_secure(name, &value, user) {
                        eprintln!("uimode: put {name}: {}", e.message);
                    }
                    continue;
                }
                Effect::InvalidateNightMode if invalidated.0 => continue,
                Effect::InvalidateCurrentModeType if invalidated.1 => continue,
                _ => {}
            }
            let Some(b) = &bridge else { continue };
            match effect {
                Effect::Configuration(ui_mode) => b.update_configuration(ui_mode),
                Effect::DockApp {
                    category,
                    may_start_app,
                    ui_mode,
                    apply,
                    start_dream_immediately,
                    dreams_disabled,
                } => {
                    let started =
                        category.is_some_and(|c| may_start_app && b.start_dock_app(c, ui_mode));
                    if apply {
                        b.update_configuration(ui_mode);
                    }
                    if category.is_some() && !started {
                        b.start_dream_if_docked(start_dream_immediately, dreams_disabled);
                    }
                }
                Effect::InvalidateNightMode => {
                    invalidated.0 = true;
                    b.invalidate_night_mode_cache();
                }
                Effect::InvalidateCurrentModeType => {
                    invalidated.1 = true;
                    b.invalidate_current_mode_type_cache();
                }
                Effect::DeviceTheme(theme) => b.set_device_theme(&theme),
                Effect::TwilightListening(on) => b.set_twilight_listening(on),
                Effect::CarModeBroadcast(on, priority, package) => {
                    b.send_car_mode_broadcast(on, priority, package)
                }
                Effect::ForegroundBroadcast(action) => b.send_foreground_broadcast(action),
                Effect::DockBroadcast(action, enable, disable) => {
                    b.send_dock_broadcast(action, enable, disable)
                }
                Effect::CarModeStatusBar(car) => b.set_car_mode_status_bar(car),
                Effect::KeepScreenOn(on) => b.set_keep_screen_on(on),
                Effect::PutSecure(..) => {}
            }
        }
    }

    // Settings (`Settings.Secure.get*ForUser`).

    fn secure(&self, name: &str, user: i32) -> Option<String> {
        self.settings.secure(name, user).unwrap_or_else(|e| {
            eprintln!("uimode: read {name}: {}", e.message);
            None
        })
    }

    fn secure_int(&self, name: &str, default: i32, user: i32) -> i32 {
        self.secure(name, user)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn secure_long(&self, name: &str, default: i64, user: i32) -> i64 {
        self.secure(name, user)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn secure_float(&self, name: &str, default: f32, user: i32) -> f32 {
        self.secure(name, user)
            .and_then(|v| crate::thermal::java_float_parse(&v))
            .unwrap_or(default)
    }

    fn check(&self, permission: &str, caller: Caller) -> bool {
        self.system
            .check_permission(permission, caller.pid, caller.uid)
            .unwrap_or(false)
    }

    // The bridge, attached when system_server's services are ready.

    fn attach(&self, handle: u32) {
        let mut data = Parcel::new();
        ibridge::GetUiModeBridge {
            host: Some(self.host),
        }
        .write(&mut data);
        let reply = match self
            .process
            .transact(handle, ibridge::GET_UI_MODE_BRIDGE, &data, false)
        {
            Ok(reply) => reply,
            Err(s) => {
                eprintln!("uimode: getUiModeBridge: status {s}");
                return;
            }
        };
        let Ok(Ok(Some(Binder::Handle(h)))) =
            ibridge::read_get_ui_mode_bridge_reply(&mut reply.reader())
        else {
            eprintln!("uimode: system_server keeps its own UiModeManagerService");
            return;
        };
        let bridge = Arc::new(Bridge {
            strong: self.process.strong(h),
        });
        drop(reply);
        let this = self.this.clone();
        self.process.link_to_death(
            &bridge.strong,
            Box::new(move || {
                if let Some(service) = this.upgrade() {
                    service.bridge.lock().unwrap().take();
                    service.state.lock().unwrap().system_ready = false;
                }
            }),
        );
        let Some(config) = bridge.config() else {
            return;
        };
        *self.bridge.lock().unwrap() = Some(bridge.clone());
        let this = self.this.upgrade().expect("the service lives while called");
        for &(uri, setting, user) in OBSERVED {
            if setting == Setting::ForceInvert && !config.force_invert_color {
                continue;
            }
            let node = self.process.add_service(Arc::new(bridge::Observer {
                service: Arc::downgrade(&this),
                setting,
            }));
            bridge.register_content_observer(uri, node, user);
        }
        self.with(|this, s, fx| {
            s.config = config;
            // What system_server held is new: a new system_server has
            // none of it applied.
            s.set_ui_mode = 0;
            s.keeping_screen_on = false;
            s.twilight_listening = false;
            // `onStart`, with the Mac's appearance over what was persisted.
            s.current_user = USER_SYSTEM;
            this.verify_setup_wizard_completed(s, &bridge);
            this.update_night_mode_from_settings(s, USER_SYSTEM);
            this.take_mac(s, fx);
            this.update_system_properties(s, fx);
            // `onBootPhase(PHASE_SYSTEM_SERVICES_READY)`.
            s.system_ready = true;
            s.car_mode_enabled = s.dock_state == DOCK_CAR;
            this.update_configuration(s, fx);
            this.apply_configuration_externally(s, fx);
        });
    }

    /// `verifySetupWizardCompleted`: observes the setting until setup is
    /// complete.
    fn verify_setup_wizard_completed(&self, s: &mut State, bridge: &Bridge) {
        s.setup_complete = self.secure_int(USER_SETUP_COMPLETE, 0, USER_SYSTEM) == 1;
        if !s.setup_complete {
            let node = self.process.add_service(Arc::new(bridge::Observer {
                service: self.this.clone(),
                setting: Setting::SetupComplete,
            }));
            bridge.register_content_observer(SETUP_COMPLETE_URI, node, USER_SYSTEM);
        }
    }

    // The Mac.

    fn mac_changed(&self, dark: bool) {
        self.with(|this, s, fx| {
            s.mac_dark = dark;
            if s.system_ready && this.take_mac(s, fx) {
                this.update_locked(s, fx, 0, 0);
            }
        });
    }

    /// The Mac's appearance as the night mode, set as `setNightMode` sets
    /// one, by the system; whether it changed.
    fn take_mac(&self, s: &mut State, fx: &mut Vec<Effect>) -> bool {
        let mode = if s.mac_dark {
            MODE_NIGHT_YES
        } else {
            MODE_NIGHT_NO
        };
        if s.night_mode == mode && s.custom_type == CUSTOM_TYPE_UNKNOWN {
            return false;
        }
        if s.night_mode == MODE_NIGHT_AUTO || s.night_mode == MODE_NIGHT_CUSTOM {
            s.wait_for_device_inactive = false;
            self.cancel_alarm(s);
        }
        s.custom_type = CUSTOM_TYPE_UNKNOWN;
        set_night_mode(s, fx, mode);
        reset_override(s, fx);
        persist_night_mode(s, fx, s.current_user);
        true
    }

    // The original's state machine, under the state lock.

    /// `updateNightModeFromSettingsLocked`.
    fn update_night_mode_from_settings(&self, s: &mut State, user: i32) {
        if s.car_mode_enabled || s.config.car || !s.setup_complete {
            return;
        }
        let mode = self.secure_int(UI_NIGHT_MODE, s.night_mode, user);
        s.night_mode = mode;
        s.custom_type = self.secure_int(UI_NIGHT_MODE_CUSTOM_TYPE, CUSTOM_TYPE_UNKNOWN, user);
        s.override_on = self.secure_int(UI_NIGHT_MODE_OVERRIDE_ON, 0, user) != 0;
        s.override_off = self.secure_int(UI_NIGHT_MODE_OVERRIDE_OFF, 0, user) != 0;
        s.custom_start =
            valid_time(self.secure_long(DARK_THEME_CUSTOM_START_TIME, DEFAULT_CUSTOM_START, user))
                .unwrap_or(DEFAULT_CUSTOM_START);
        s.custom_end =
            valid_time(self.secure_long(DARK_THEME_CUSTOM_END_TIME, DEFAULT_CUSTOM_END, user))
                .unwrap_or(DEFAULT_CUSTOM_END);
        if s.night_mode == MODE_NIGHT_AUTO {
            s.computed_night_mode = self.secure_int(UI_NIGHT_MODE_LAST_COMPUTED, 0, user) != 0;
        }
    }

    /// `updateSystemProperties`: `persist.sys.theme`.
    fn update_system_properties(&self, s: &State, fx: &mut Vec<Effect>) {
        let mut mode = self.secure_int(UI_NIGHT_MODE, s.night_mode, USER_SYSTEM);
        if mode == MODE_NIGHT_AUTO || mode == MODE_NIGHT_CUSTOM {
            mode = MODE_NIGHT_YES;
        }
        fx.push(Effect::DeviceTheme(mode.to_string()));
    }

    /// `shouldApplyAutomaticChangesImmediately`.
    fn apply_immediately(&self, s: &State) -> bool {
        s.config.car
            || s.custom_type == CUSTOM_TYPE_BEDTIME
            || !self.bridge().is_none_or(|b| b.is_device_active())
    }

    /// `registerDeviceInactiveListenerLocked`.
    fn register_device_inactive(&self, s: &mut State) {
        if !s.power_save {
            s.wait_for_device_inactive = true;
        }
    }

    /// `updateConfigurationLocked`.
    fn update_configuration(&self, s: &mut State, fx: &mut Vec<Effect>) {
        let mut ui_mode = s.config.default_ui_mode_type;
        if s.config.ui_mode_locked {
        } else if s.config.television {
            ui_mode = UI_MODE_TYPE_TELEVISION;
        } else if s.config.watch {
            ui_mode = UI_MODE_TYPE_WATCH;
        } else if s.car_mode_enabled {
            ui_mode = UI_MODE_TYPE_CAR;
        } else if is_desk(s.dock_state) {
            ui_mode = UI_MODE_TYPE_DESK;
        } else if s.vr_headset {
            ui_mode = UI_MODE_TYPE_VR_HEADSET;
        }
        if s.night_mode == MODE_NIGHT_YES || s.night_mode == MODE_NIGHT_NO {
            update_computed_night_mode(s, fx, s.night_mode == MODE_NIGHT_YES);
        }
        if s.night_mode == MODE_NIGHT_AUTO {
            if !s.twilight_listening {
                s.twilight_listening = true;
                fx.push(Effect::TwilightListening(true));
            }
            let twilight = self.bridge().and_then(|b| b.twilight());
            let activate = twilight.unwrap_or(s.computed_night_mode);
            update_computed_night_mode(s, fx, activate);
        } else if s.twilight_listening {
            s.twilight_listening = false;
            fx.push(Effect::TwilightListening(false));
        }
        if s.night_mode == MODE_NIGHT_CUSTOM {
            if s.custom_type == CUSTOM_TYPE_BEDTIME {
                update_computed_night_mode(s, fx, s.last_bedtime_requested);
            } else {
                s.time_changes = true;
                let activate = compute_custom(s);
                update_computed_night_mode(s, fx, activate);
                self.schedule_next_custom_time(s);
            }
        } else {
            s.time_changes = false;
        }
        if s.power_save && !s.car_mode_enabled && !s.config.car {
            ui_mode &= !UI_MODE_NIGHT_NO;
            ui_mode |= UI_MODE_NIGHT_YES;
        } else {
            ui_mode = computed_ui_mode(ui_mode, s.computed_night_mode);
        }
        s.cur_ui_mode = ui_mode;
        fx.push(Effect::InvalidateCurrentModeType);
        if !s.holding_configuration && (!s.wait_for_device_inactive || s.power_save) {
            s.configuration = ui_mode;
        }
    }

    /// `applyConfigurationExternallyLocked`.
    fn apply_configuration_externally(&self, s: &mut State, fx: &mut Vec<Effect>) {
        if s.set_ui_mode != s.configuration {
            s.set_ui_mode = s.configuration;
            fx.push(Effect::Configuration(s.configuration));
        }
    }

    /// `scheduleNextCustomTimeListener`.
    fn schedule_next_custom_time(&self, s: &mut State) {
        self.cancel_alarm(s);
        let next = if compute_custom(s) {
            time::next(s.custom_end)
        } else {
            time::next(s.custom_start)
        };
        let (_, now) = time::now();
        let delay = Duration::from_micros((next - now).max(0) as u64);
        s.alarm += 1;
        *self.alarm_at.lock().unwrap() = Some((Instant::now() + delay, s.alarm));
        self.alarm.notify_all();
    }

    fn cancel_alarm(&self, s: &mut State) {
        s.alarm += 1;
        *self.alarm_at.lock().unwrap() = None;
    }

    /// `updateCustomTimeLocked`.
    fn update_custom_time(&self, s: &mut State, fx: &mut Vec<Effect>) {
        if s.night_mode != MODE_NIGHT_CUSTOM {
            return;
        }
        if self.apply_immediately(s) {
            self.update_locked(s, fx, 0, 0);
        } else {
            self.register_device_inactive(s);
        }
        self.schedule_next_custom_time(s);
    }

    /// `onCustomTimeUpdated`.
    fn custom_time_updated(&self, s: &mut State, fx: &mut Vec<Effect>, user: i32) {
        persist_night_mode(s, fx, user);
        if s.night_mode != MODE_NIGHT_CUSTOM {
            return;
        }
        if self.apply_immediately(s) {
            s.wait_for_device_inactive = false;
            self.update_locked(s, fx, 0, 0);
        } else {
            self.register_device_inactive(s);
        }
    }

    /// `updateLocked`.
    fn update_locked(&self, s: &mut State, fx: &mut Vec<Effect>, enable: i32, disable: i32) {
        let mut action = None;
        let mut old_action = None;
        if s.last_broadcast_state == DOCK_CAR {
            fx.push(Effect::CarModeStatusBar(s.car_mode_enabled));
            old_action = Some(ACTION_EXIT_CAR_MODE);
        } else if is_desk(s.last_broadcast_state) {
            old_action = Some(ACTION_EXIT_DESK_MODE);
        }
        if s.car_mode_enabled {
            if s.last_broadcast_state != DOCK_CAR {
                fx.push(Effect::CarModeStatusBar(s.car_mode_enabled));
                if let Some(old) = old_action {
                    fx.push(Effect::ForegroundBroadcast(old));
                }
                s.last_broadcast_state = DOCK_CAR;
                action = Some(ACTION_ENTER_CAR_MODE);
            }
        } else if is_desk(s.dock_state) {
            if !is_desk(s.last_broadcast_state) {
                if let Some(old) = old_action {
                    fx.push(Effect::ForegroundBroadcast(old));
                }
                s.last_broadcast_state = s.dock_state;
                action = Some(ACTION_ENTER_DESK_MODE);
            }
        } else {
            s.last_broadcast_state = DOCK_UNDOCKED;
            action = old_action;
        }
        if let Some(action) = action {
            // The ordered broadcast; its result starts the dock app, and
            // the configuration waits for it.
            fx.push(Effect::DockBroadcast(action, enable, disable));
            s.holding_configuration = true;
            self.update_configuration(s, fx);
        } else {
            let category = if s.car_mode_enabled {
                (s.config.enable_car_dock_launch && enable & ENABLE_CAR_MODE_GO_CAR_HOME != 0)
                    .then_some(CATEGORY_CAR_DOCK)
            } else if is_desk(s.dock_state) {
                (enable & ENABLE_CAR_MODE_GO_CAR_HOME != 0).then_some(CATEGORY_DESK_DOCK)
            } else {
                (disable & DISABLE_CAR_MODE_GO_HOME != 0).then_some(CATEGORY_HOME)
            };
            self.send_configuration_and_start_dream_or_dock_app(s, fx, category);
        }
        // Keeps the screen on while charging in car mode (or desk mode, as
        // the original compares the whole UI mode with the desk type).
        let keep = s.charging
            && ((s.car_mode_enabled
                && s.config.car_mode_keeps_screen_on
                && s.car_mode_enable_flags & ENABLE_CAR_MODE_ALLOW_SLEEP == 0)
                || (s.cur_ui_mode == UI_MODE_TYPE_DESK && s.config.desk_mode_keeps_screen_on));
        if keep != s.keeping_screen_on {
            s.keeping_screen_on = keep;
            fx.push(Effect::KeepScreenOn(keep));
        }
    }

    /// `updateAfterBroadcastLocked`.
    fn update_after_broadcast(
        &self,
        s: &mut State,
        fx: &mut Vec<Effect>,
        action: &str,
        enable: i32,
        disable: i32,
    ) {
        let category = match action {
            ACTION_ENTER_CAR_MODE => (s.config.enable_car_dock_launch
                && enable & ENABLE_CAR_MODE_GO_CAR_HOME != 0)
                .then_some(CATEGORY_CAR_DOCK),
            ACTION_ENTER_DESK_MODE => {
                (enable & ENABLE_CAR_MODE_GO_CAR_HOME != 0).then_some(CATEGORY_DESK_DOCK)
            }
            _ => (disable & DISABLE_CAR_MODE_GO_HOME != 0).then_some(CATEGORY_HOME),
        };
        self.send_configuration_and_start_dream_or_dock_app(s, fx, category);
    }

    /// `sendConfigurationAndStartDreamOrDockAppLocked`.
    fn send_configuration_and_start_dream_or_dock_app(
        &self,
        s: &mut State,
        fx: &mut Vec<Effect>,
        category: Option<&'static str>,
    ) {
        s.holding_configuration = false;
        self.update_configuration(s, fx);
        let apply = s.set_ui_mode != s.configuration;
        s.set_ui_mode = s.configuration;
        fx.push(Effect::DockApp {
            category,
            // `shouldStartDockApp`: never on a watch before setup completes.
            may_start_app: !s.config.watch || s.setup_complete,
            ui_mode: s.configuration,
            apply,
            start_dream_immediately: s.config.start_dream_immediately_on_dock,
            dreams_disabled: s.config.dreams_disabled_by_ambient_mode_suppression,
        });
    }

    /// `setCarModeLocked`.
    fn set_car_mode(
        &self,
        s: &mut State,
        fx: &mut Vec<Effect>,
        enabled: bool,
        flags: i32,
        priority: i32,
        package: Option<String>,
    ) {
        if enabled {
            // `enableCarMode`.
            let present = s.car_mode_packages.values().any(|p| *p == package);
            if !s.car_mode_packages.contains_key(&priority) && !present {
                eprintln!(
                    "uimode: enableCarMode: enabled at priority={priority}, packageName={}",
                    package.as_deref().unwrap_or("null")
                );
                s.car_mode_packages.insert(priority, package.clone());
                fx.push(Effect::CarModeBroadcast(true, priority, package));
            }
        } else {
            // `disableCarMode`.
            let all = flags & DISABLE_CAR_MODE_ALL_PRIORITIES != 0;
            let allowed = priority == DEFAULT_PRIORITY
                || s.car_mode_packages
                    .get(&priority)
                    .is_some_and(|p| *p == package)
                || all;
            if allowed {
                if all {
                    for (priority, package) in std::mem::take(&mut s.car_mode_packages) {
                        fx.push(Effect::CarModeBroadcast(false, priority, package));
                    }
                } else {
                    s.car_mode_packages.remove(&priority);
                    fx.push(Effect::CarModeBroadcast(false, priority, package));
                }
            }
        }
        let now_enabled = !s.car_mode_packages.is_empty();
        if s.car_mode_enabled != now_enabled {
            s.car_mode_enabled = now_enabled;
            // Out of car mode, the night mode is the setting's again.
            if !now_enabled {
                self.update_night_mode_from_settings(s, USER_SYSTEM);
            }
        }
        s.car_mode_enable_flags = flags;
    }

    /// `updateContrastLocked`: whether the current user's contrast changed.
    fn update_contrast(&self, s: &mut State) -> bool {
        let contrast = self.secure_float(CONTRAST_LEVEL, 0.0, s.current_user);
        let old = s
            .contrasts
            .get(&s.current_user)
            .copied()
            .unwrap_or(f32::MAX);
        if (old - contrast).abs() >= 1e-10 {
            s.contrasts.insert(s.current_user, contrast);
            return true;
        }
        false
    }

    /// `getContrastLocked`.
    fn contrast(&self, s: &mut State) -> f32 {
        if !s.contrasts.contains_key(&s.current_user) {
            self.update_contrast(s);
        }
        s.contrasts.get(&s.current_user).copied().unwrap_or(0.0)
    }

    /// `updateForceInvertStateLocked`.
    fn update_force_invert(&self, s: &mut State) -> bool {
        let state = if !s.config.force_invert_color
            || !self
                .bridge()
                .is_some_and(|b| b.is_system_ui_in_dark_theme())
            || self.secure_int(FORCE_INVERT_COLOR_ENABLED, 0, s.current_user) != 1
        {
            FORCE_INVERT_OFF
        } else {
            FORCE_INVERT_DARK
        };
        if s.force_invert.get(&s.current_user) != Some(&state) {
            s.force_invert.insert(s.current_user, state);
            return true;
        }
        false
    }

    /// `getForceInvertStateLocked`.
    fn force_invert(&self, s: &mut State) -> i32 {
        if !s.force_invert.contains_key(&s.current_user) && s.system_ready {
            self.update_force_invert(s);
        }
        s.force_invert
            .get(&s.current_user)
            .copied()
            .unwrap_or(FORCE_INVERT_OFF)
    }

    /// Tells the current user's callbacks, one-way.
    fn notify_callbacks(&self, s: &State, code: u32, write: impl Fn(&mut Parcel)) {
        let mut data = Parcel::new();
        write(&mut data);
        for l in s.callbacks.get(&s.current_user).into_iter().flatten() {
            let _ = l.strong.transact(code, &data, true);
        }
    }

    // Changes from system_server and the settings.

    fn on_setting_changed(&self, setting: Setting, self_change: bool) {
        match setting {
            Setting::NightMode => {
                self.with(|this, s, fx| this.update_system_properties(s, fx));
                self.update_force_invert_states();
            }
            Setting::ForceInvert => self.update_force_invert_states(),
            Setting::Contrast => self.with(|this, s, _| {
                if this.update_contrast(s) {
                    let contrast = this.contrast(s);
                    this.notify_callbacks(s, callback::NOTIFY_CONTRAST_CHANGED, |p| {
                        callback::NotifyContrastChanged { contrast }.write(p)
                    });
                }
            }),
            Setting::SetupComplete => self.with(|this, s, fx| {
                if s.setup_complete
                    || self_change
                    || this.secure_int(USER_SETUP_COMPLETE, 0, USER_SYSTEM) != 1
                {
                    return;
                }
                s.setup_complete = true;
                this.update_night_mode_from_settings(s, USER_SYSTEM);
                this.update_locked(s, fx, 0, 0);
            }),
        }
    }

    /// `updateForceInvertStates`.
    fn update_force_invert_states(&self) {
        self.with(|this, s, _| {
            if s.config.force_invert_color && this.update_force_invert(s) {
                let state = this.force_invert(s);
                this.notify_callbacks(s, callback::NOTIFY_FORCE_INVERT_STATE_CHANGED, |p| {
                    callback::NotifyForceInvertStateChanged {
                        force_invert_state: state,
                    }
                    .write(p)
                });
            }
        });
    }

    fn on_dock_event(&self, state: i32) {
        self.with(|this, s, fx| {
            if state != s.dock_state {
                s.dock_state = state;
                this.set_car_mode(
                    s,
                    fx,
                    state == DOCK_CAR,
                    0,
                    DEFAULT_PRIORITY,
                    Some(String::new()),
                );
                if s.system_ready {
                    this.update_locked(s, fx, ENABLE_CAR_MODE_GO_CAR_HOME, 0);
                }
            }
        });
    }

    fn on_battery_changed(&self, charging: bool) {
        self.with(|this, s, fx| {
            s.charging = charging;
            if s.system_ready {
                this.update_locked(s, fx, 0, 0);
            }
        });
    }

    fn on_power_save_changed(&self, power_save: bool) {
        self.with(|this, s, fx| {
            if s.power_save == power_save {
                return;
            }
            s.power_save = power_save;
            if s.system_ready {
                this.update_locked(s, fx, 0, 0);
            }
        });
    }

    fn on_twilight_state_changed(&self) {
        self.with(|this, s, fx| {
            if s.night_mode == MODE_NIGHT_AUTO && s.system_ready {
                if this.apply_immediately(s) {
                    this.update_locked(s, fx, 0, 0);
                } else {
                    this.register_device_inactive(s);
                }
            }
        });
    }

    fn on_device_inactive(&self) {
        self.with(|this, s, fx| {
            if s.wait_for_device_inactive {
                s.wait_for_device_inactive = false;
                this.update_locked(s, fx, 0, 0);
            }
        });
    }

    fn on_time_changed(&self) {
        self.with(|this, s, fx| {
            if s.time_changes {
                this.update_custom_time(s, fx);
            }
        });
    }

    fn on_setting_restored(&self, name: Option<&str>) {
        let restored = [
            UI_NIGHT_MODE,
            DARK_THEME_CUSTOM_START_TIME,
            DARK_THEME_CUSTOM_END_TIME,
        ];
        if !name.is_some_and(|n| restored.contains(&n)) {
            return;
        }
        self.with(|this, s, fx| {
            this.update_night_mode_from_settings(s, USER_SYSTEM);
            this.update_configuration(s, fx);
        });
    }

    fn on_shutdown(&self) {
        self.with(|_, s, fx| {
            if s.night_mode == MODE_NIGHT_AUTO {
                persist_computed_night_mode(s, fx, s.current_user);
            }
        });
    }

    fn on_vr_state_changed(&self, enabled: bool) {
        self.with(|this, s, fx| {
            s.vr_headset = enabled;
            if s.system_ready {
                this.update_locked(s, fx, 0, 0);
            }
        });
    }

    fn on_user_switching(&self, from: i32, to: i32) {
        self.with(|this, s, fx| {
            s.current_user = to;
            if s.night_mode == MODE_NIGHT_AUTO {
                persist_computed_night_mode(s, fx, from);
            }
            s.setup_complete = this.secure_int(USER_SETUP_COMPLETE, 0, USER_SYSTEM) == 1;
            this.update_night_mode_from_settings(s, to);
            this.update_locked(s, fx, 0, 0);
        });
    }

    fn on_dock_broadcast_result(&self, action: &str, enable: i32, disable: i32, code: i32) {
        if code != RESULT_OK {
            return;
        }
        let action = [ACTION_ENTER_CAR_MODE, ACTION_ENTER_DESK_MODE]
            .into_iter()
            .find(|a| *a == action)
            .unwrap_or(ACTION_EXIT_CAR_MODE);
        self.with(|this, s, fx| this.update_after_broadcast(s, fx, action, enable, disable));
    }

    /// The custom schedule's alarm, if it is still the one set.
    fn alarm_fired(&self, generation: u64) {
        self.with(|this, s, fx| {
            if s.alarm == generation {
                this.update_custom_time(s, fx);
            }
        });
    }

    // IUiModeManager.

    fn add_callback(&self, caller: Caller, binder: Option<Binder>) -> Result<()> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(null_binder());
        };
        let user = caller.user();
        let listener = self.listener(handle, move |s| {
            if let Some(list) = s.callbacks.get_mut(&user) {
                list.retain(|l| l.strong.handle != handle);
            }
        });
        let mut s = self.state.lock().unwrap();
        let list = s.callbacks.entry(user).or_default();
        self.unregister(list, handle);
        list.push(listener);
        Ok(())
    }

    /// A listener of `handle`, dropped from where `forget` says when its
    /// process dies.
    fn listener(&self, handle: u32, forget: impl FnOnce(&mut State) + Send + 'static) -> Listener {
        let strong = self.process.strong(handle);
        let this = self.this.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(service) = this.upgrade() {
                    forget(&mut service.state.lock().unwrap());
                }
            }),
        );
        Listener { strong, death }
    }

    /// `RemoteCallbackList.unregister`.
    fn unregister(&self, list: &mut Vec<Listener>, handle: u32) -> bool {
        match list.iter().position(|l| l.strong.handle == handle) {
            Some(at) => {
                let l = list.remove(at);
                self.process.clear_death(&l.strong, l.death);
                true
            }
            None => false,
        }
    }

    /// `assertLegit`: the package must be the caller's.
    fn assert_legit(&self, caller: Caller, package: Option<&str>) -> Result<()> {
        let legit = package.is_some_and(|p| self.system.check_package(caller.uid, p).is_ok());
        if legit {
            Ok(())
        } else {
            Err(Exception::security(format!(
                "Caller claimed bogus packageName: {}.",
                package.unwrap_or("null")
            )))
        }
    }

    fn enable_car_mode(
        &self,
        caller: Caller,
        flags: i32,
        priority: i32,
        package: Option<String>,
    ) -> Result<()> {
        if self.state.lock().unwrap().config.ui_mode_locked {
            eprintln!("uimode: enableCarMode while UI mode is locked");
            return Ok(());
        }
        if priority != DEFAULT_PRIORITY && !self.check(ENTER_CAR_MODE_PRIORITIZED, caller) {
            return Err(Exception::security(
                "Enabling car mode with a priority requires permission ENTER_CAR_MODE_PRIORITIZED",
            ));
        }
        // The shell may enable car mode (`cmd uimode car yes`).
        if caller.uid != SHELL_UID {
            self.assert_legit(caller, package.as_deref())?;
        }
        self.with(|this, s, fx| {
            this.set_car_mode(s, fx, true, flags, priority, package);
            if s.system_ready {
                this.update_locked(s, fx, flags, 0);
            }
        });
        Ok(())
    }

    fn disable_car_mode(&self, caller: Caller, flags: i32, package: Option<String>) -> Result<()> {
        if self.state.lock().unwrap().config.ui_mode_locked {
            eprintln!("uimode: disableCarMode while UI mode is locked");
            return Ok(());
        }
        // The system may disable every priority (the car mode
        // notification), and the shell may disable car mode.
        let system = caller.uid == SYSTEM_UID;
        if !system && caller.uid != SHELL_UID {
            self.assert_legit(caller, package.as_deref())?;
        }
        let car_flags = if system {
            flags
        } else {
            flags & !DISABLE_CAR_MODE_ALL_PRIORITIES
        };
        self.with(|this, s, fx| {
            let priority = s
                .car_mode_packages
                .iter()
                .find(|(_, p)| package.is_some() && **p == package)
                .map_or(DEFAULT_PRIORITY, |(priority, _)| *priority);
            this.set_car_mode(s, fx, false, car_flags, priority, package);
            if s.system_ready {
                this.update_locked(s, fx, 0, flags);
            }
        });
        Ok(())
    }

    /// `setNightModeInternal`.
    fn set_night_mode_internal(&self, caller: Caller, mode: i32, custom_type: i32) -> Result<()> {
        if !self.check(MODIFY_DAY_NIGHT_MODE, caller) {
            // Night mode is locked: the system manages it.
            eprintln!("uimode: Night mode locked, requires MODIFY_DAY_NIGHT_MODE permission");
            return Ok(());
        }
        match mode {
            MODE_NIGHT_NO | MODE_NIGHT_YES | MODE_NIGHT_AUTO => {}
            MODE_NIGHT_CUSTOM
                if custom_type == CUSTOM_TYPE_SCHEDULE || custom_type == CUSTOM_TYPE_BEDTIME => {}
            MODE_NIGHT_CUSTOM => {
                return Err(Exception::illegal_argument(format!(
                    "Can't set the custom type to {custom_type}"
                )));
            }
            _ => return Err(Exception::illegal_argument(format!("Unknown mode: {mode}"))),
        }
        let user = caller.user();
        self.with(|this, s, fx| {
            if s.night_mode == mode && s.custom_type == custom_type {
                return;
            }
            if s.night_mode == MODE_NIGHT_AUTO || s.night_mode == MODE_NIGHT_CUSTOM {
                s.wait_for_device_inactive = false;
                this.cancel_alarm(s);
            }
            s.custom_type = if mode == MODE_NIGHT_CUSTOM {
                custom_type
            } else {
                CUSTOM_TYPE_UNKNOWN
            };
            set_night_mode(s, fx, mode);
            reset_override(s, fx);
            persist_night_mode(s, fx, user);
            // On screen off otherwise.
            if (mode != MODE_NIGHT_AUTO && mode != MODE_NIGHT_CUSTOM) || this.apply_immediately(s) {
                s.wait_for_device_inactive = false;
                this.update_locked(s, fx, 0, 0);
            } else {
                this.register_device_inactive(s);
            }
        });
        Ok(())
    }

    /// `setNightModeActivatedForModeInternal`.
    fn set_night_mode_activated(
        &self,
        caller: Caller,
        custom_type: Option<i32>,
        active: bool,
        user_interaction: bool,
    ) -> bool {
        if !self.check(MODIFY_DAY_NIGHT_MODE, caller) {
            eprintln!("uimode: Night mode locked, requires MODIFY_DAY_NIGHT_MODE permission");
            return false;
        }
        let current = self.state.lock().unwrap().current_user;
        if caller.user() != current && !self.check(INTERACT_ACROSS_USERS, caller) {
            eprintln!(
                "uimode: Target user is not current user, INTERACT_ACROSS_USERS permission is required"
            );
            return false;
        }
        self.with(|this, s, fx| {
            let custom_type = custom_type.unwrap_or(s.custom_type);
            // Kept, so a later switch to bedtime needs no notice.
            if custom_type == CUSTOM_TYPE_BEDTIME {
                s.last_bedtime_requested = active;
            }
            if custom_type != s.custom_type {
                return false;
            }
            if s.night_mode == MODE_NIGHT_AUTO || s.night_mode == MODE_NIGHT_CUSTOM {
                s.wait_for_device_inactive = false;
                s.override_off = !active;
                s.override_on = active;
                s.override_user = s.current_user;
                persist_night_mode_overrides(s, fx, s.current_user);
            } else if s.night_mode == MODE_NIGHT_NO && active {
                set_night_mode(s, fx, MODE_NIGHT_YES);
            } else if s.night_mode == MODE_NIGHT_YES && !active {
                set_night_mode(s, fx, MODE_NIGHT_NO);
            }
            if user_interaction {
                // Toggling dark theme ends attention mode.
                s.attention = ATTENTION_OFF;
            }
            this.update_configuration(s, fx);
            this.apply_configuration_externally(s, fx);
            persist_night_mode(s, fx, s.current_user);
            true
        })
    }

    /// `setCustomNightModeStart` and `End`.
    fn set_custom_time(&self, caller: Caller, start: bool, time: i64) {
        if !self.check(MODIFY_DAY_NIGHT_MODE, caller) {
            let which = if start { "start" } else { "end" };
            eprintln!("uimode: Set custom time {which}, requires MODIFY_DAY_NIGHT_MODE permission");
            return;
        }
        let user = caller.user();
        self.with(|this, s, fx| {
            // `LocalTime.ofNanoOfDay(time * 1000)` throws out of the day.
            let Some(time) = valid_time(time) else {
                s.wait_for_device_inactive = false;
                return;
            };
            if start {
                s.custom_start = time;
                persist_night_mode(s, fx, user);
            } else {
                s.custom_end = time;
            }
            this.custom_time_updated(s, fx, user);
        });
    }

    fn enforce_projection_permission(&self, caller: Caller, projection_type: i32) -> Result<()> {
        if projection_type & PROJECTION_TYPE_AUTOMOTIVE != 0
            && !self.check(TOGGLE_AUTOMOTIVE_PROJECTION, caller)
        {
            return Err(Exception::security(format!(
                "toggleProjection: uid {} does not have {TOGGLE_AUTOMOTIVE_PROJECTION}.",
                caller.uid
            )));
        }
        Ok(())
    }

    fn request_projection(
        &self,
        caller: Caller,
        binder: Option<Binder>,
        projection_type: i32,
        package: Option<String>,
    ) -> Result<bool> {
        self.assert_legit(caller, package.as_deref())?;
        assert_single_projection_type(projection_type)?;
        self.enforce_projection_permission(caller, projection_type)?;
        let package = package.unwrap_or_default();
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke interface method 'void android.os.IBinder.linkToDeath\
                 (android.os.IBinder$DeathRecipient, int)' on a null object reference",
            ));
        };
        let mut s = self.state.lock().unwrap();
        let holders = s.projection_holders.entry(projection_type).or_default();
        if holders.iter().any(|h| h.package == package) {
            return Ok(true);
        }
        // Automotive projection is held by one package at a time.
        if projection_type == PROJECTION_TYPE_AUTOMOTIVE && !holders.is_empty() {
            return Ok(false);
        }
        let strong = self.process.strong(handle);
        let this = self.this.clone();
        let (died_type, died_package) = (projection_type, package.clone());
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(service) = this.upgrade() {
                    eprintln!(
                        "uimode: Projection holder {died_package} died. Releasing projection \
                         type {died_type}."
                    );
                    service.release_projection_unchecked(died_type, &died_package);
                }
            }),
        );
        s.projection_holders
            .entry(projection_type)
            .or_default()
            .push(Holder {
                package: package.clone(),
                strong,
                death,
            });
        eprintln!("uimode: Package {package} set projection type {projection_type}.");
        self.projection_state_changed(&s, projection_type);
        Ok(true)
    }

    fn release_projection_unchecked(&self, projection_type: i32, package: &str) -> bool {
        let mut s = self.state.lock().unwrap();
        let mut removed = false;
        if let Some(holders) = s.projection_holders.get_mut(&projection_type) {
            let mut i = holders.len();
            while i > 0 {
                i -= 1;
                if holders[i].package == package {
                    let h = holders.remove(i);
                    self.process.clear_death(&h.strong, h.death);
                    eprintln!("uimode: Projection type {projection_type} released by {package}.");
                    removed = true;
                }
            }
        }
        if removed {
            self.projection_state_changed(&s, projection_type);
        } else {
            eprintln!(
                "uimode: {package} tried to release projection type {projection_type} but was \
                 not set by that package."
            );
        }
        removed
    }

    /// `populateWithRelevantActivePackageNames`.
    fn projecting(s: &State, projection_type: i32) -> (i32, Vec<String>) {
        let mut types = PROJECTION_TYPE_NONE;
        let mut packages = Vec::new();
        for (&key, holders) in &s.projection_holders {
            if projection_type & key != 0 && !holders.is_empty() {
                packages.extend(holders.iter().map(|h| h.package.clone()));
                types |= key;
            }
        }
        (types, packages)
    }

    fn notify_projection(l: &Listener, types: i32, packages: &[String]) {
        let mut data = Parcel::new();
        projection_listener::OnProjectionStateChanged {
            active_projection_types: types,
            projecting_packages: Some(packages.iter().cloned().map(Some).collect()),
        }
        .write(&mut data);
        let _ = l.strong.transact(
            projection_listener::ON_PROJECTION_STATE_CHANGED,
            &data,
            true,
        );
    }

    /// `onProjectionStateChangedLocked`.
    fn projection_state_changed(&self, s: &State, changed: i32) {
        for (&listening, listeners) in &s.projection_listeners {
            if changed & listening != 0 {
                let (types, packages) = Self::projecting(s, listening);
                for l in listeners {
                    Self::notify_projection(l, types, &packages);
                }
            }
        }
    }

    fn add_projection_listener(&self, binder: Option<Binder>, projection_type: i32) -> Result<()> {
        if projection_type == PROJECTION_TYPE_NONE {
            return Ok(());
        }
        let Some(Binder::Handle(handle)) = binder else {
            return Err(null_binder());
        };
        let listener = self.listener(handle, move |s| {
            if let Some(list) = s.projection_listeners.get_mut(&projection_type) {
                list.retain(|l| l.strong.handle != handle);
            }
        });
        let mut s = self.state.lock().unwrap();
        let list = s.projection_listeners.entry(projection_type).or_default();
        self.unregister(list, handle);
        list.push(listener);
        // Told at once of what is active.
        let (types, packages) = Self::projecting(&s, projection_type);
        if !packages.is_empty() {
            let l = s.projection_listeners[&projection_type].last().unwrap();
            Self::notify_projection(l, types, &packages);
        }
        Ok(())
    }

    fn remove_projection_listener(&self, binder: Option<Binder>) -> Result<()> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(null_binder());
        };
        let mut s = self.state.lock().unwrap();
        let lists: Vec<i32> = s.projection_listeners.keys().copied().collect();
        for key in lists {
            let mut list = s.projection_listeners.remove(&key).unwrap_or_default();
            self.unregister(&mut list, handle);
            s.projection_listeners.insert(key, list);
        }
        Ok(())
    }

    fn dispatch(&self, call: &mut Call<'_>) -> Result<Option<Parcel>> {
        let caller = Caller {
            uid: call.sender_euid as i32,
            pid: call.sender_pid,
        };
        let r = &mut call.data;
        let mut reply = Parcel::new();
        match call.code {
            ium::ADD_CALLBACK => {
                let a = ium::AddCallback::read(r).map_err(bad_parcel)?;
                self.add_callback(caller, a.callback)?;
                ium::write_add_callback_reply(&mut reply);
            }
            ium::ENABLE_CAR_MODE => {
                let a = ium::EnableCarMode::read(r).map_err(bad_parcel)?;
                self.enable_car_mode(caller, a.flags, a.priority, a.calling_package)?;
                ium::write_enable_car_mode_reply(&mut reply);
            }
            ium::DISABLE_CAR_MODE => {
                let a = ium::DisableCarMode::read(r).map_err(bad_parcel)?;
                self.disable_car_mode(caller, a.flags, None)?;
                ium::write_disable_car_mode_reply(&mut reply);
            }
            ium::DISABLE_CAR_MODE_BY_CALLING_PACKAGE => {
                let a = ium::DisableCarModeByCallingPackage::read(r).map_err(bad_parcel)?;
                self.disable_car_mode(caller, a.flags, a.calling_package)?;
                ium::write_disable_car_mode_by_calling_package_reply(&mut reply);
            }
            ium::GET_CURRENT_MODE_TYPE => {
                ium::GetCurrentModeType::read(r).map_err(bad_parcel)?;
                let mode = self.state.lock().unwrap().cur_ui_mode & UI_MODE_TYPE_MASK;
                ium::write_get_current_mode_type_reply(&mut reply, mode);
            }
            ium::SET_NIGHT_MODE => {
                let mode = ium::SetNightMode::read(r).map_err(bad_parcel)?.mode;
                // MODE_NIGHT_CUSTOM is a schedule unless said otherwise.
                let custom = if mode == MODE_NIGHT_CUSTOM {
                    CUSTOM_TYPE_SCHEDULE
                } else {
                    CUSTOM_TYPE_UNKNOWN
                };
                self.set_night_mode_internal(caller, mode, custom)?;
                ium::write_set_night_mode_reply(&mut reply);
            }
            ium::GET_NIGHT_MODE => {
                ium::GetNightMode::read(r).map_err(bad_parcel)?;
                let mode = self.state.lock().unwrap().night_mode;
                ium::write_get_night_mode_reply(&mut reply, mode);
            }
            ium::SET_NIGHT_MODE_CUSTOM_TYPE => {
                let a = ium::SetNightModeCustomType::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(MODIFY_DAY_NIGHT_MODE, caller.pid, caller.uid)?;
                self.set_night_mode_internal(caller, MODE_NIGHT_CUSTOM, a.night_mode_custom_type)?;
                ium::write_set_night_mode_custom_type_reply(&mut reply);
            }
            ium::GET_NIGHT_MODE_CUSTOM_TYPE => {
                ium::GetNightModeCustomType::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(MODIFY_DAY_NIGHT_MODE, caller.pid, caller.uid)?;
                let t = self.state.lock().unwrap().custom_type;
                ium::write_get_night_mode_custom_type_reply(&mut reply, t);
            }
            ium::SET_ATTENTION_MODE_THEME_OVERLAY => {
                let a = ium::SetAttentionModeThemeOverlay::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(MODIFY_DAY_NIGHT_MODE, caller.pid, caller.uid)?;
                let overlay = a.attention_mode_theme_overlay_type;
                self.with(|this, s, fx| {
                    if s.attention != overlay {
                        s.attention = overlay;
                        this.update_locked(s, fx, 0, 0);
                    }
                });
                ium::write_set_attention_mode_theme_overlay_reply(&mut reply);
            }
            ium::GET_ATTENTION_MODE_THEME_OVERLAY => {
                ium::GetAttentionModeThemeOverlay::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(MODIFY_DAY_NIGHT_MODE, caller.pid, caller.uid)?;
                let overlay = self.state.lock().unwrap().attention;
                ium::write_get_attention_mode_theme_overlay_reply(&mut reply, overlay);
            }
            ium::SET_APPLICATION_NIGHT_MODE => {
                let mode = ium::SetApplicationNightMode::read(r)
                    .map_err(bad_parcel)?
                    .mode;
                let night = match mode {
                    MODE_NIGHT_YES => UI_MODE_NIGHT_YES,
                    MODE_NIGHT_NO => UI_MODE_NIGHT_NO,
                    MODE_NIGHT_AUTO | MODE_NIGHT_CUSTOM => 0,
                    _ => return Err(Exception::illegal_argument(format!("Unknown mode: {mode}"))),
                };
                if let Some(b) = self.bridge() {
                    b.set_application_night_mode(caller.pid, caller.uid, night);
                }
                ium::write_set_application_night_mode_reply(&mut reply);
            }
            ium::IS_UI_MODE_LOCKED => {
                ium::IsUiModeLocked::read(r).map_err(bad_parcel)?;
                let locked = self.state.lock().unwrap().config.ui_mode_locked;
                ium::write_is_ui_mode_locked_reply(&mut reply, locked);
            }
            ium::IS_NIGHT_MODE_LOCKED => {
                ium::IsNightModeLocked::read(r).map_err(bad_parcel)?;
                // The Mac manages night mode.
                ium::write_is_night_mode_locked_reply(&mut reply, true);
            }
            ium::SET_NIGHT_MODE_ACTIVATED_FOR_CUSTOM_MODE => {
                let a = ium::SetNightModeActivatedForCustomMode::read(r).map_err(bad_parcel)?;
                let done = self.set_night_mode_activated(
                    caller,
                    Some(a.night_mode_custom),
                    a.active,
                    false,
                );
                ium::write_set_night_mode_activated_for_custom_mode_reply(&mut reply, done);
            }
            ium::SET_NIGHT_MODE_ACTIVATED => {
                let a = ium::SetNightModeActivated::read(r).map_err(bad_parcel)?;
                let done = self.set_night_mode_activated(caller, None, a.active, true);
                ium::write_set_night_mode_activated_reply(&mut reply, done);
            }
            ium::GET_CUSTOM_NIGHT_MODE_START => {
                ium::GetCustomNightModeStart::read(r).map_err(bad_parcel)?;
                let t = self.state.lock().unwrap().custom_start;
                ium::write_get_custom_night_mode_start_reply(&mut reply, t);
            }
            ium::SET_CUSTOM_NIGHT_MODE_START => {
                let a = ium::SetCustomNightModeStart::read(r).map_err(bad_parcel)?;
                self.set_custom_time(caller, true, a.time);
                ium::write_set_custom_night_mode_start_reply(&mut reply);
            }
            ium::GET_CUSTOM_NIGHT_MODE_END => {
                ium::GetCustomNightModeEnd::read(r).map_err(bad_parcel)?;
                let t = self.state.lock().unwrap().custom_end;
                ium::write_get_custom_night_mode_end_reply(&mut reply, t);
            }
            ium::SET_CUSTOM_NIGHT_MODE_END => {
                let a = ium::SetCustomNightModeEnd::read(r).map_err(bad_parcel)?;
                self.set_custom_time(caller, false, a.time);
                ium::write_set_custom_night_mode_end_reply(&mut reply);
            }
            ium::REQUEST_PROJECTION => {
                let a = ium::RequestProjection::read(r).map_err(bad_parcel)?;
                let done = self.request_projection(
                    caller,
                    a.binder,
                    a.projection_type,
                    a.calling_package,
                )?;
                ium::write_request_projection_reply(&mut reply, done);
            }
            ium::RELEASE_PROJECTION => {
                let a = ium::ReleaseProjection::read(r).map_err(bad_parcel)?;
                self.assert_legit(caller, a.calling_package.as_deref())?;
                assert_single_projection_type(a.projection_type)?;
                self.enforce_projection_permission(caller, a.projection_type)?;
                let package = a.calling_package.unwrap_or_default();
                let done = self.release_projection_unchecked(a.projection_type, &package);
                ium::write_release_projection_reply(&mut reply, done);
            }
            ium::ADD_ON_PROJECTION_STATE_CHANGED_LISTENER => {
                let a = ium::AddOnProjectionStateChangedListener::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(READ_PROJECTION_STATE, caller.pid, caller.uid)?;
                self.add_projection_listener(a.listener, a.projection_type)?;
                ium::write_add_on_projection_state_changed_listener_reply(&mut reply);
            }
            ium::REMOVE_ON_PROJECTION_STATE_CHANGED_LISTENER => {
                let a = ium::RemoveOnProjectionStateChangedListener::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(READ_PROJECTION_STATE, caller.pid, caller.uid)?;
                self.remove_projection_listener(a.listener)?;
                ium::write_remove_on_projection_state_changed_listener_reply(&mut reply);
            }
            ium::GET_PROJECTING_PACKAGES => {
                let a = ium::GetProjectingPackages::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(READ_PROJECTION_STATE, caller.pid, caller.uid)?;
                let (_, packages) =
                    Self::projecting(&self.state.lock().unwrap(), a.projection_type);
                let packages = Some(packages.into_iter().map(Some).collect());
                ium::write_get_projecting_packages_reply(&mut reply, &packages);
            }
            ium::GET_ACTIVE_PROJECTION_TYPES => {
                ium::GetActiveProjectionTypes::read(r).map_err(bad_parcel)?;
                self.system
                    .enforce_permission(READ_PROJECTION_STATE, caller.pid, caller.uid)?;
                let (types, _) = Self::projecting(&self.state.lock().unwrap(), -1);
                ium::write_get_active_projection_types_reply(&mut reply, types);
            }
            ium::GET_CONTRAST => {
                ium::GetContrast::read(r).map_err(bad_parcel)?;
                let contrast = self.contrast(&mut self.state.lock().unwrap());
                ium::write_get_contrast_reply(&mut reply, contrast);
            }
            ium::GET_FORCE_INVERT_STATE => {
                ium::GetForceInvertState::read(r).map_err(bad_parcel)?;
                let state = self.force_invert(&mut self.state.lock().unwrap());
                ium::write_get_force_invert_state_reply(&mut reply, state);
            }
            DUMP_TRANSACTION => {
                self.dump(call);
                reply.write_no_exception();
            }
            SHELL_COMMAND_TRANSACTION => {
                let command = ShellCommand::read(&self.process, r).map_err(bad_parcel)?;
                if command.has_output() {
                    self.shell_command(caller, command);
                }
                reply.write_no_exception();
            }
            _ => return Ok(None),
        }
        Ok(Some(reply))
    }

    /// `dumpImpl`, after `DumpUtils.checkDumpPermission`.
    fn dump(&self, call: &mut Call<'_>) {
        use std::io::Write;
        let Some(mut out) = call
            .data
            .read_fd()
            .ok()
            .and_then(|fd| self.process.file(fd))
            .and_then(|f| aim_binder_host::server::file_fd(&f))
            .map(std::fs::File::from)
        else {
            return;
        };
        let (pid, uid) = (call.sender_pid, call.sender_euid as i32);
        if !self.check(DUMP, Caller { uid, pid }) {
            let _ = writeln!(
                out,
                "Permission Denial: can't dump UiModeManager from from pid={pid}, uid={uid} due \
                 to missing android.permission.DUMP permission"
            );
            return;
        }
        let twilight = self.bridge().map(|b| b.dump_twilight());
        let s = self.state.lock().unwrap();
        let mut t = String::from("Current UI Mode Service state:\n");
        t += &format!(
            "  mDockState={} mLastBroadcastState={}\n",
            s.dock_state, s.last_broadcast_state
        );
        t += &format!(
            " mStartDreamImmediatelyOnDock={}  mNightMode={} ({})  mOverrideOn/Off={}/{}  \
             mAttentionModeThemeOverlay={} mNightModeLocked=true\n",
            s.config.start_dream_immediately_on_dock,
            s.night_mode,
            night_mode_str(s.night_mode, s.custom_type),
            s.override_on,
            s.override_off,
            s.attention
        );
        t += &format!("  mCarModeEnabled={} (carModeApps=", s.car_mode_enabled);
        for (priority, package) in &s.car_mode_packages {
            t += &format!("{priority}:{} ", package.as_deref().unwrap_or("null"));
        }
        t += "\n";
        t += &format!(
            " mWaitForDeviceInactive={} mComputedNightMode={} customStart={} customEnd{} \
             mCarModeEnableFlags={} mEnableCarDockLaunch={}\n",
            s.wait_for_device_inactive,
            s.computed_night_mode,
            time::format(s.custom_start),
            time::format(s.custom_end),
            s.car_mode_enable_flags,
            s.config.enable_car_dock_launch
        );
        t += &format!(
            "  mCurUiMode=0x{:x} mUiModeLocked={} mSetUiMode=0x{:x}\n",
            s.cur_ui_mode, s.config.ui_mode_locked, s.set_ui_mode
        );
        t += &format!(
            "  mHoldingConfiguration={} mSystemReady={}\n",
            s.holding_configuration, s.system_ready
        );
        if let Some(twilight) = twilight {
            t += &format!(
                "  mTwilightService.getLastTwilightState()={}\n",
                twilight.as_deref().unwrap_or("null")
            );
        }
        t += &format!(
            "  the Mac's appearance: {}\n",
            if s.mac_dark { "Dark" } else { "Light" }
        );
        drop(s);
        let _ = out.write_all(t.as_bytes());
    }

    /// `UiModeManagerService.Shell.onCommand`: the commands call the
    /// service as the caller of the shell command.
    fn shell_command(&self, caller: Caller, mut c: ShellCommand) {
        let result = match c.command().map(str::to_string).as_deref() {
            Some("night") => self.shell_night(caller, &mut c),
            Some("car") => match c.next_arg().as_deref() {
                None => {
                    self.print_car_mode(&mut c);
                    0
                }
                Some(yes @ ("yes" | "no")) => {
                    let result = if yes == "yes" {
                        self.enable_car_mode(caller, 0, DEFAULT_PRIORITY, Some(String::new()))
                    } else {
                        self.disable_car_mode(caller, 0, None)
                    };
                    match result {
                        Ok(()) => {
                            self.print_car_mode(&mut c);
                            0
                        }
                        Err(e) => {
                            c.exception(&java_exception(&e));
                            -1
                        }
                    }
                }
                Some(_) => {
                    c.eprintln("Error: mode must be 'yes', or 'no'");
                    -1
                }
            },
            Some("time") => match c.next_arg().as_deref() {
                None => {
                    let s = self.state.lock().unwrap();
                    let (start, end) = (s.custom_start, s.custom_end);
                    drop(s);
                    c.println(&format!("start {}", time::format(start)));
                    c.println(&format!("end {}", time::format(end)));
                    0
                }
                Some(which @ ("start" | "end")) => {
                    let text = c.next_arg();
                    match time::parse(text.as_deref()) {
                        Ok(t) => {
                            self.set_custom_time(caller, which == "start", t);
                            0
                        }
                        Err(e) => {
                            c.exception(&e);
                            -1
                        }
                    }
                }
                Some(_) => {
                    c.eprintln("command must be in [start|end]");
                    -1
                }
            },
            None | Some("help" | "-h") => {
                help(&mut c);
                -1
            }
            Some(other) => {
                c.println(&format!("Unknown command: {other}"));
                -1
            }
        };
        c.finish(result);
    }

    /// `handleNightMode`.
    fn shell_night(&self, caller: Caller, c: &mut ShellCommand) -> i32 {
        let Some(mode) = c.next_arg() else {
            return self.print_night_mode(caller, c);
        };
        let (night, custom) = match mode.as_str() {
            "yes" => (MODE_NIGHT_YES, -1),
            "no" => (MODE_NIGHT_NO, -1),
            "auto" => (MODE_NIGHT_AUTO, -1),
            "custom_schedule" => (MODE_NIGHT_CUSTOM, CUSTOM_TYPE_SCHEDULE),
            "custom_bedtime" => (MODE_NIGHT_CUSTOM, CUSTOM_TYPE_BEDTIME),
            _ => {
                c.eprintln(
                    "Error: mode must be 'yes', 'no', or 'auto', or 'custom_schedule', or \
                     'custom_bedtime'",
                );
                return -1;
            }
        };
        let schedule = if night == MODE_NIGHT_CUSTOM {
            CUSTOM_TYPE_SCHEDULE
        } else {
            CUSTOM_TYPE_UNKNOWN
        };
        let mut result = self.set_night_mode_internal(caller, night, schedule);
        if result.is_ok() && night == MODE_NIGHT_CUSTOM {
            result = self
                .system
                .enforce_permission(MODIFY_DAY_NIGHT_MODE, caller.pid, caller.uid)
                .and_then(|()| self.set_night_mode_internal(caller, MODE_NIGHT_CUSTOM, custom));
        }
        if let Err(e) = result {
            c.exception(&java_exception(&e));
            return -1;
        }
        self.print_night_mode(caller, c)
    }

    /// `printCurrentNightMode`, which reads the custom type as the caller.
    fn print_night_mode(&self, caller: Caller, c: &mut ShellCommand) -> i32 {
        if let Err(e) =
            self.system
                .enforce_permission(MODIFY_DAY_NIGHT_MODE, caller.pid, caller.uid)
        {
            c.exception(&java_exception(&e));
            return -1;
        }
        let s = self.state.lock().unwrap();
        let line = format!(
            "Night mode: {}",
            night_mode_str(s.night_mode, s.custom_type)
        );
        drop(s);
        c.println(&line);
        0
    }

    fn print_car_mode(&self, c: &mut ShellCommand) {
        let car = self.state.lock().unwrap().cur_ui_mode & UI_MODE_TYPE_MASK == UI_MODE_TYPE_CAR;
        c.println(&format!("Car mode: {}", if car { "yes" } else { "no" }));
    }
}

/// `UiModeManagerService.Shell.onHelp`.
fn help(c: &mut ShellCommand) {
    for line in [
        "UiModeManager service (uimode) commands:",
        "  help",
        "    Print this help text.",
        "  night [yes|no|auto|custom_schedule|custom_bedtime]",
        "    Set or read night mode.",
        "  car [yes|no]",
        "    Set or read car mode.",
        "  time [start|end] <ISO time>",
        "    Set custom start/end schedule time (night mode must be set to custom to apply).",
    ] {
        c.println(line);
    }
}

/// An exception as its `toString` prints it.
fn java_exception(e: &Exception) -> String {
    let class = match e.code {
        aim_binder_host::parcel::EX_SECURITY => "java.lang.SecurityException",
        aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT => "java.lang.IllegalArgumentException",
        EX_NULL_POINTER => "java.lang.NullPointerException",
        _ => "java.lang.IllegalStateException",
    };
    format!("{class}: {}", e.message)
}

/// `assertSingleProjectionType`.
fn assert_single_projection_type(p: i32) -> Result<()> {
    if p == 0 || p & p.wrapping_sub(1) != 0 {
        return Err(Exception::illegal_argument(
            "Must specify exactly one projection type.",
        ));
    }
    Ok(())
}

/// A time of the day in microseconds, as `LocalTime.ofNanoOfDay(t * 1000)`
/// takes it.
fn valid_time(micros: i64) -> Option<i64> {
    let nanos = micros.wrapping_mul(1000);
    (0..time::MICROS_PER_DAY * 1000)
        .contains(&nanos)
        .then_some(nanos / 1000)
}

/// `mNightMode.set`, which invalidates the clients' cache.
fn set_night_mode(s: &mut State, fx: &mut Vec<Effect>, mode: i32) {
    s.night_mode = mode;
    fx.push(Effect::InvalidateNightMode);
}

/// `persistNightMode`: not in car mode.
fn persist_night_mode(s: &State, fx: &mut Vec<Effect>, user: i32) {
    if s.car_mode_enabled || s.config.car {
        return;
    }
    fx.push(Effect::PutSecure(
        UI_NIGHT_MODE,
        s.night_mode.to_string(),
        user,
    ));
    fx.push(Effect::PutSecure(
        UI_NIGHT_MODE_CUSTOM_TYPE,
        s.custom_type.to_string(),
        user,
    ));
    fx.push(Effect::PutSecure(
        DARK_THEME_CUSTOM_START_TIME,
        s.custom_start.to_string(),
        user,
    ));
    fx.push(Effect::PutSecure(
        DARK_THEME_CUSTOM_END_TIME,
        s.custom_end.to_string(),
        user,
    ));
}

/// `persistNightModeOverrides`: not in car mode.
fn persist_night_mode_overrides(s: &State, fx: &mut Vec<Effect>, user: i32) {
    if s.car_mode_enabled || s.config.car {
        return;
    }
    let flag = |b: bool| if b { "1" } else { "0" }.to_string();
    fx.push(Effect::PutSecure(
        UI_NIGHT_MODE_OVERRIDE_ON,
        flag(s.override_on),
        user,
    ));
    fx.push(Effect::PutSecure(
        UI_NIGHT_MODE_OVERRIDE_OFF,
        flag(s.override_off),
        user,
    ));
}

/// `persistComputedNightMode`.
fn persist_computed_night_mode(s: &State, fx: &mut Vec<Effect>, user: i32) {
    let computed = if s.computed_night_mode { "1" } else { "0" };
    fx.push(Effect::PutSecure(
        UI_NIGHT_MODE_LAST_COMPUTED,
        computed.into(),
        user,
    ));
}

/// `resetNightModeOverrideLocked`.
fn reset_override(s: &mut State, fx: &mut Vec<Effect>) {
    if s.override_off || s.override_on {
        s.override_off = false;
        s.override_on = false;
        persist_night_mode_overrides(s, fx, s.override_user);
        s.override_user = USER_SYSTEM;
    }
}

/// `updateComputedNightModeLocked`.
fn update_computed_night_mode(s: &mut State, fx: &mut Vec<Effect>, activate: bool) {
    let mut computed = activate;
    let automatic = s.night_mode != MODE_NIGHT_YES && s.night_mode != MODE_NIGHT_NO;
    if automatic {
        if s.override_on && !computed {
            computed = true;
        } else if s.override_off && computed {
            computed = false;
        }
    }
    s.computed_night_mode = match s.attention {
        ATTENTION_NIGHT => true,
        ATTENTION_DAY => false,
        _ => computed,
    };
    // Overrides last until the schedule agrees; a fixed mode has none.
    if !automatic {
        reset_override(s, fx);
    }
}

/// `getComputedUiModeConfiguration`.
fn computed_ui_mode(mut ui_mode: i32, night: bool) -> i32 {
    ui_mode |= if night {
        UI_MODE_NIGHT_YES
    } else {
        UI_MODE_NIGHT_NO
    };
    ui_mode &= if night {
        !UI_MODE_NIGHT_NO
    } else {
        !UI_MODE_NIGHT_YES
    };
    ui_mode
}

/// `computeCustomNightMode`.
fn compute_custom(s: &State) -> bool {
    time::is_between(time::now().0, s.custom_start, s.custom_end)
}

/// Runs the custom schedule's alarm while the service lives.
fn alarms(service: Weak<UiModeManagerService>) {
    loop {
        let Some(s) = service.upgrade() else { return };
        let mut at = s.alarm_at.lock().unwrap();
        let fired = match *at {
            Some((due, generation)) if due <= Instant::now() => {
                *at = None;
                Some(generation)
            }
            Some((due, _)) => {
                let wait = due - Instant::now();
                drop(
                    s.alarm
                        .wait_timeout(at, wait.min(Duration::from_secs(60)))
                        .unwrap(),
                );
                None
            }
            None => {
                drop(s.alarm.wait_timeout(at, Duration::from_secs(60)).unwrap());
                None
            }
        };
        if let Some(generation) = fired {
            s.alarm_fired(generation);
        }
    }
}

impl Service for UiModeManagerService {
    fn descriptor(&self) -> &str {
        ium::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match self.dispatch(call) {
            Ok(Some(reply)) => Ok(reply),
            Ok(None) => Err(UNKNOWN_TRANSACTION),
            Err(exception) => {
                let mut reply = Parcel::new();
                reply.write_exception(&exception);
                Ok(reply)
            }
        }
    }

    fn accepts_fds(&self) -> bool {
        // A dump's fd, a shell command's streams.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_modes_print_as_the_shell_does() {
        assert_eq!(night_mode_str(MODE_NIGHT_YES, -1), "yes");
        assert_eq!(
            night_mode_str(MODE_NIGHT_CUSTOM, CUSTOM_TYPE_BEDTIME),
            "custom_bedtime"
        );
        assert_eq!(night_mode_str(MODE_NIGHT_CUSTOM, -1), "unknown");
        assert_eq!(night_mode_str(7, -1), "unknown");
    }

    #[test]
    fn night_mode_sets_one_night_bit() {
        assert_eq!(computed_ui_mode(0x01, true), 0x21);
        assert_eq!(computed_ui_mode(0x21, false), 0x11);
        assert_eq!(computed_ui_mode(0x13, true), 0x23);
    }

    #[test]
    fn projection_types_are_single_bits() {
        assert!(assert_single_projection_type(1).is_ok());
        assert!(assert_single_projection_type(0).is_err());
        assert!(assert_single_projection_type(3).is_err());
        assert!(assert_single_projection_type(-1).is_err());
    }

    #[test]
    fn times_are_within_the_day() {
        assert_eq!(valid_time(0), Some(0));
        assert_eq!(
            valid_time(time::MICROS_PER_DAY - 1),
            Some(time::MICROS_PER_DAY - 1)
        );
        assert_eq!(valid_time(time::MICROS_PER_DAY), None);
        assert_eq!(valid_time(-1), None);
    }
}
