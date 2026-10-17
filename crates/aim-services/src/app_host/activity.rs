//! Activities: ActivityTaskManager's side of an activity's start, as
//! `ActivityTaskSupervisor.realStartActivityLocked` makes it at
//! `android-16.0.0_r1` (one `ClientTransaction` of a `LaunchActivityItem`
//! and a `ResumeActivityItem`, then a `TopResumedActivityChangeItem`), and
//! `IActivityClientController`, where the activity reports back; and
//! its configuration's changes when its task's bounds change
//! (`ActivityRecord.updateReportedConfigurationAndSend`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iactivityclientcontroller as acc;
use aim_service_aidl::android_app_iactivitytaskmanager as atm;

use super::config::{self, Configuration, SizeBuckets};
use super::{AppHost, reply};
use crate::package::info::ActivityInfo;

/// `ActivityManager.PROCESS_STATE_UNKNOWN` and `PROCESS_STATE_CACHED_EMPTY`:
/// the resume's process state, and the launch's before the process state
/// is reported.
const PROCESS_STATE_UNKNOWN: i32 = -1;
const PROCESS_STATE_CACHED_EMPTY: i32 = 19;
/// `WindowConfiguration.ACTIVITY_TYPE_STANDARD`.
const ACTIVITY_TYPE_STANDARD: i32 = 1;
/// `Display.DEFAULT_DISPLAY`, `Display.INVALID_DISPLAY`.
const DEFAULT_DISPLAY: i32 = 0;
const INVALID_DISPLAY: i32 = -1;
/// `Intent.FLAG_ACTIVITY_NEW_TASK`; `Intent.EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN`,
/// which a start without a creator token gets; `UserHandle.USER_CURRENT`.
const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
const EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN: i32 = 1 << 2;
const USER_CURRENT: i32 = -2;
/// `ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED`.
const SCREEN_ORIENTATION_UNSPECIFIED: i32 = -1;

/// An activity's tokens: plain binders the app holds and returns.
pub struct Token;

impl Service for Token {
    fn descriptor(&self) -> &str {
        ""
    }
    fn has_descriptor(&self) -> bool {
        false
    }
    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Err(UNKNOWN_TRANSACTION)
    }
}

/// An activity started in an app's process.
pub struct ActivityRecord {
    pub package: String,
    pub class: String,
    pub uid: u32,
    pub start_seq: i64,
    pub started: Instant,
    pub resumed: bool,
    pub requested_orientation: i32,
    /// `ActivityInfo.getRealConfigChanged`: the changes it handles itself.
    pub config_changes: i32,
    /// The size buckets it reported (`reportSizeConfigurations`).
    pub size_buckets: Option<SizeBuckets>,
    /// The configuration it was last sent (`mLastReportedConfiguration`'s
    /// merged one).
    pub reported: Option<Configuration>,
}

/// `Build.VERSION_CODES.HONEYCOMB_MR2`.
const HONEYCOMB_MR2: i32 = 13;

impl ActivityRecord {
    /// `ActivityInfo.getRealConfigChanged` of `info`.
    pub fn real_config_changes(info: &ActivityInfo) -> i32 {
        if info.info.application_info.target_sdk_version < HONEYCOMB_MR2 {
            info.config_changes | config::CONFIG_SCREEN_SIZE | config::CONFIG_SMALLEST_SCREEN_SIZE
        } else {
            info.config_changes
        }
    }
}

/// The activities, by their token.
#[derive(Default)]
pub struct Activities {
    records: Mutex<HashMap<u64, ActivityRecord>>,
    next_ident: Mutex<i32>,
}

impl Activities {
    /// Whether `uid` has the resumed activity: the app host's one task is
    /// focused (`WindowManagerInternal.isUidFocused`).
    pub fn uid_focused(&self, uid: u32) -> bool {
        self.records.lock().unwrap().values().any(|r| r.uid == uid && r.resumed)
    }

    /// The component (`package/class`) of `uid`'s latest activity, a
    /// window's name.
    pub fn top_component(&self, uid: u32) -> Option<String> {
        let records = self.records.lock().unwrap();
        records.values().filter(|r| r.uid == uid).max_by_key(|r| r.started).map(|r| format!("{}/{}", r.package, r.class))
    }

    /// The activities of the process of `start_seq` are gone with it.
    pub fn remove_start(&self, start_seq: i64) {
        self.records.lock().unwrap().retain(|_, r| r.start_seq != start_seq);
    }

    fn with<T>(
        &self,
        token: Option<Binder>,
        f: impl FnOnce(&mut ActivityRecord) -> T,
    ) -> Option<T> {
        let Some(Binder::Local(ptr)) = token else {
            return None;
        };
        self.records.lock().unwrap().get_mut(&ptr).map(f)
    }
}

/// What a launch needs besides its record.
pub struct Launch<'a> {
    pub info: ActivityInfo,
    pub global: &'a Configuration,
    /// The bounds of its task in window mode, else the display's.
    pub task: Option<super::config::Rect>,
}

impl AppHost {
    /// `realStartActivityLocked`: the transaction that launches and
    /// resumes `record`'s activity, then the one that makes it the top
    /// resumed activity.
    pub(super) fn launch_activity(
        &self,
        mut record: ActivityRecord,
        launch: Launch<'_>,
    ) -> (Parcel, Parcel) {
        let token = self.process.add_service(Arc::new(Token));
        let assist = self.process.add_service(Arc::new(Token));
        let shareable = self.process.add_service(Arc::new(Token));
        let caller_info = self.process.add_service(Arc::new(Token));
        let ident = {
            let mut next = self.activities.next_ident.lock().unwrap();
            *next += 1;
            *next
        };
        let mut override_config = match launch.task {
            Some(bounds) => launch.global.of_freeform_task(bounds),
            None => launch.global.clone(),
        };
        override_config.window.activity_type = ACTIVITY_TYPE_STANDARD;
        record.reported = Some(override_config.clone());
        let bounds = override_config.window.bounds;
        let intent = Intent {
            component: (record.package.clone(), record.class.clone()),
            flags: FLAG_ACTIVITY_NEW_TASK,
            extended_flags: EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN,
            content_user_hint: USER_CURRENT,
        };
        let items: Vec<Box<dyn Item>> = vec![
            Box::new(LaunchActivityItem {
                token,
                ident,
                current: launch.global.clone(),
                overrides: override_config,
                referrer: "com.android.shell".into(),
                assist,
                shareable,
                caller_info,
                window: ActivityWindowInfo {
                    embedded: false,
                    task_bounds: bounds,
                    task_fragment_bounds: bounds,
                },
                intent,
                info: launch.info,
            }),
            Box::new(ResumeActivityItem {
                token,
                proc_state: PROCESS_STATE_UNKNOWN,
                forward: true,
            }),
        ];
        let launch = transaction(items);
        let top = transaction(vec![Box::new(TopResumedActivityChangeItem {
            token,
            on_top: true,
        })]);
        if let Binder::Local(ptr) = token {
            self.activities.records.lock().unwrap().insert(ptr, record);
        }
        (launch, top)
    }
}

impl AppHost {
    /// `updateReportedConfigurationAndSend` for the activities of the
    /// process `start_seq` once their task took `bounds`, on a display of
    /// the global configuration `global`: the items that tell them, and
    /// whether one relaunches (its windows are not told they resized).
    pub(super) fn reconfigure_activities(&self, start_seq: i64, global: &Configuration, bounds: config::Rect) -> (Vec<Box<dyn Item>>, bool) {
        let mut records = self.activities.records.lock().unwrap();
        let mut items: Vec<Box<dyn Item>> = Vec::new();
        let mut relaunching = false;
        for (&ptr, record) in records.iter_mut().filter(|(_, r)| r.start_seq == start_seq) {
            let token = Binder::Local(ptr);
            let mut new = global.of_freeform_task(bounds);
            new.window.activity_type = ACTIVITY_TYPE_STANDARD;
            let Some(old) = record.reported.replace(new.clone()) else { continue };
            if old == new {
                continue;
            }
            let window = ActivityWindowInfo { embedded: false, task_bounds: bounds, task_fragment_bounds: bounds };
            // `getConfigurationChanges`: the size changes its buckets
            // matter for; a window configuration change alone is none.
            let mut changes = old.diff(&new);
            if let Some(buckets) = &record.size_buckets {
                changes = buckets.filter_diff(changes, &old, &new);
            }
            changes &= !config::CONFIG_WINDOW_CONFIGURATION;
            // `shouldRelaunchLocked`, with `handleAllConfigChanges` (on in
            // the image): an activity whose resources are unused handles
            // every change.
            let handled = record.config_changes;
            if changes & !handled == 0 || handled & config::CONFIG_RESOURCES_UNUSED != 0 {
                items.push(Box::new(ActivityConfigurationChangeItem { token, config: new, window }));
                continue;
            }
            // `relaunchActivityLocked`: a resize alone keeps the window.
            let resize = config::CONFIG_SCREEN_SIZE | config::CONFIG_SMALLEST_SCREEN_SIZE | config::CONFIG_ORIENTATION | config::CONFIG_SCREEN_LAYOUT;
            relaunching = true;
            items.push(Box::new(ActivityRelaunchItem {
                token,
                global: global.clone(),
                config: new,
                window,
                changes,
                preserve_window: changes & !resize == 0,
            }));
            items.push(Box::new(if record.resumed {
                Lifecycle::Resume(ResumeActivityItem { token, proc_state: PROCESS_STATE_UNKNOWN, forward: false })
            } else {
                Lifecycle::Pause(token)
            }));
            eprintln!("app host: relaunching {}/{} for changes {changes:#x}", record.package, record.class);
        }
        (items, relaunching)
    }
}

/// `IApplicationThread.scheduleTransaction` of `items`.
pub(super) fn transaction(items: Vec<Box<dyn Item>>) -> Parcel {
    let mut p = Parcel::new();
    aim_service_aidl::android_app_iapplicationthread::ScheduleTransaction {
        transaction: Some(ClientTransaction(items)),
    }
    .write(&mut p);
    p
}

/// A `ClientTransactionItem`: its class, for `writeParcelable`, and its
/// fields.
pub(super) trait Item {
    fn class(&self) -> &'static str;
    fn write_fields(&self, p: &mut Parcel);
}

struct ClientTransaction(Vec<Box<dyn Item>>);

impl WriteParcelable for ClientTransaction {
    /// `writeParcelableList`: each item's class name, then its fields.
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0.len() as i32);
        for item in &self.0 {
            p.write_string16(Some(item.class()));
            item.write_fields(p);
        }
    }
}

struct LaunchActivityItem {
    token: Binder,
    ident: i32,
    current: Configuration,
    overrides: Configuration,
    referrer: String,
    assist: Binder,
    shareable: Binder,
    caller_info: Binder,
    window: ActivityWindowInfo,
    intent: Intent,
    info: ActivityInfo,
}

impl Item for LaunchActivityItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.LaunchActivityItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        p.write_i32(self.ident);
        p.write_i32(1);
        self.current.write_to(p);
        p.write_i32(1);
        self.overrides.write_to(p);
        p.write_i32(DEFAULT_DISPLAY); // the device id: the default device's
        p.write_string16(Some(&self.referrer));
        p.write_binder(None); // no voice interactor
        p.write_i32(PROCESS_STATE_CACHED_EMPTY);
        p.write_i32(-1); // no saved state (`writeBundle`)
        p.write_i32(-1); // no persistent state (`writePersistableBundle`)
        p.write_i32(-1); // no pending results
        p.write_i32(-1); // no pending new intents
        p.write_i32(0); // no scene transition
        p.write_bool(true); // forward
        p.write_i32(0); // no profiler
        p.write_binder(Some(self.assist));
        p.write_binder(None); // the activity client controller is asked for
        p.write_binder(Some(self.shareable));
        p.write_bool(false); // not launched from a bubble
        p.write_binder(None); // no task fragment
        p.write_binder(Some(self.caller_info));
        p.write_i32(1);
        self.window.write(p);
        p.write_i32(1);
        self.intent.write(p);
        p.write_i32(1);
        self.info.write_to(p);
    }
}

struct ResumeActivityItem {
    token: Binder,
    proc_state: i32,
    forward: bool,
}

impl Item for ResumeActivityItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.ResumeActivityItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        p.write_i32(self.proc_state);
        p.write_bool(self.forward);
        p.write_bool(false); // no compat fake focus
    }
}

struct ActivityConfigurationChangeItem {
    token: Binder,
    config: Configuration,
    window: ActivityWindowInfo,
}

impl Item for ActivityConfigurationChangeItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.ActivityConfigurationChangeItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        p.write_i32(1);
        self.config.write_to(p);
        p.write_i32(1);
        self.window.write(p);
    }
}

struct ActivityRelaunchItem {
    token: Binder,
    global: Configuration,
    config: Configuration,
    window: ActivityWindowInfo,
    changes: i32,
    preserve_window: bool,
}

impl Item for ActivityRelaunchItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.ActivityRelaunchItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        p.write_i32(-1); // no pending results
        p.write_i32(-1); // no pending new intents
        // `MergedConfiguration`: global, override, merged (the override is
        // whole).
        p.write_i32(1);
        self.global.write_to(p);
        self.config.write_to(p);
        self.config.write_to(p);
        p.write_i32(1);
        self.window.write(p);
        p.write_i32(self.changes);
        p.write_bool(self.preserve_window);
    }
}

/// The lifecycle a relaunch ends in: resumed, or paused.
enum Lifecycle {
    Resume(ResumeActivityItem),
    Pause(Binder),
}

impl Item for Lifecycle {
    fn class(&self) -> &'static str {
        match self {
            Lifecycle::Resume(item) => item.class(),
            Lifecycle::Pause(_) => "android.app.servertransaction.PauseActivityItem",
        }
    }

    fn write_fields(&self, p: &mut Parcel) {
        match self {
            Lifecycle::Resume(item) => item.write_fields(p),
            // `PauseActivityItem(token)`: not finishing, not user leaving,
            // not reported, not entering PiP.
            Lifecycle::Pause(token) => {
                p.write_binder(Some(*token));
                [false, false, true, false].iter().for_each(|v| p.write_bool(*v));
            }
        }
    }
}

struct TopResumedActivityChangeItem {
    token: Binder,
    on_top: bool,
}

impl Item for TopResumedActivityChangeItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.TopResumedActivityChangeItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        p.write_bool(self.on_top);
    }
}

pub(super) struct ActivityWindowInfo {
    pub embedded: bool,
    pub task_bounds: super::config::Rect,
    pub task_fragment_bounds: super::config::Rect,
}

impl ActivityWindowInfo {
    pub(super) fn write(&self, p: &mut Parcel) {
        p.write_bool(self.embedded);
        for r in [self.task_bounds, self.task_fragment_bounds] {
            for v in [r.left, r.top, r.right, r.bottom] {
                p.write_i32(v);
            }
        }
    }
}

/// An explicit `Intent` to a component, as `Intent.writeToParcel` writes
/// it with intent redirect prevention (`preventIntentRedirect`) on.
struct Intent {
    component: (String, String),
    flags: i32,
    extended_flags: i32,
    content_user_hint: i32,
}

impl Intent {
    fn write(&self, p: &mut Parcel) {
        p.write_string8(None); // action
        p.write_i32(0); // no data (`Uri.writeToParcel`)
        p.write_string8(None); // type
        p.write_string8(None); // identifier
        p.write_i32(self.flags);
        p.write_i32(self.extended_flags);
        p.write_string8(None); // package
        p.write_string16(Some(&self.component.0));
        p.write_string16(Some(&self.component.1));
        p.write_i32(0); // no source bounds
        p.write_i32(0); // no categories
        p.write_i32(0); // no selector
        p.write_i32(0); // no clip data
        p.write_i32(self.content_user_hint);
        p.write_i32(-1); // no extras (`writeBundle`)
        p.write_i32(0); // no original intent
        p.write_i32(0); // no creator token
    }
}

/// `activity_task`: what an activity's process asks ActivityTaskManager.
pub struct ActivityTaskManager(pub Arc<AppHost>, pub Binder);

impl Service for ActivityTaskManager {
    fn descriptor(&self) -> &str {
        atm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            atm::GET_ACTIVITY_CLIENT_CONTROLLER => {
                atm::GetActivityClientController::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                atm::write_get_activity_client_controller_reply(&mut p, Some(self.1));
                Ok(p)
            }
            code => reply(Err(self.0.unimplemented("activity_task", code))),
        }
    }
}

/// `IActivityClientController`: the activity's reports and questions.
pub struct ActivityClientController(pub Arc<AppHost>);

impl ActivityClientController {
    /// The token of a call whose first argument is one.
    fn token(call: &mut Call<'_>) -> Result<Option<Binder>, i32> {
        call.data
            .enforce_interface(acc::DESCRIPTOR)
            .map_err(|_| BAD_VALUE)?;
        call.data.read_binder().map_err(|_| BAD_VALUE)
    }

    fn report(&self, token: Option<Binder>, what: &str, f: impl FnOnce(&mut ActivityRecord)) {
        let host = &self.0;
        let elapsed = host.activities.with(token, |r| {
            f(r);
            (format!("{}/{}", r.package, r.class), r.started.elapsed())
        });
        if let Some((name, elapsed)) = elapsed {
            eprintln!("app host: {name} {what} {elapsed:?} after its process's start");
        }
    }
}

impl Service for ActivityClientController {
    fn descriptor(&self) -> &str {
        acc::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let empty = || {
            let mut p = Parcel::new();
            p.write_no_exception();
            Ok(p)
        };
        match call.code {
            acc::ACTIVITY_RESUMED => {
                let token = Self::token(call)?;
                self.report(token, "resumed", |r| r.resumed = true);
                empty()
            }
            acc::ACTIVITY_IDLE => {
                let token = Self::token(call)?;
                self.report(token, "idle", |_| {});
                if let Some(uid) = self.0.activities.with(token, |r| r.uid) {
                    self.0.task_idle(uid);
                }
                empty()
            }
            acc::ACTIVITY_TOP_RESUMED_STATE_LOST => empty(),
            // `finishRelaunching`: its relaunch is done (oneway).
            acc::ACTIVITY_RELAUNCHED => {
                let token = Self::token(call)?;
                self.report(token, "relaunched", |_| {});
                Ok(Parcel::new())
            }
            acc::ACTIVITY_PAUSED => {
                let token = Self::token(call)?;
                self.report(token, "paused", |r| r.resumed = false);
                empty()
            }
            acc::ACTIVITY_STOPPED | acc::ACTIVITY_DESTROYED => {
                let token = Self::token(call)?;
                self.report(
                    token,
                    if call.code == acc::ACTIVITY_STOPPED {
                        "stopped"
                    } else {
                        "destroyed"
                    },
                    |_| {},
                );
                empty()
            }
            // The size buckets decide which size changes relaunch it.
            acc::REPORT_SIZE_CONFIGURATIONS => {
                let args = acc::ReportSizeConfigurations::<SizeBuckets>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.activities.with(args.token, |r| r.size_buckets = args.size_configurations);
                empty()
            }
            acc::GET_DISPLAY_ID => {
                let token = Self::token(call)?;
                let id = self
                    .0
                    .activities
                    .with(token, |_| DEFAULT_DISPLAY)
                    .unwrap_or(INVALID_DISPLAY);
                let mut p = Parcel::new();
                acc::write_get_display_id_reply(&mut p, id);
                Ok(p)
            }
            // The task's description is the Mac window's once activities
            // have windows (#1422).
            acc::SET_TASK_DESCRIPTION => empty(),
            acc::IS_TOP_OF_TASK => {
                let token = Self::token(call)?;
                let top = self.0.activities.with(token, |_| true).unwrap_or(false);
                let mut p = Parcel::new();
                acc::write_is_top_of_task_reply(&mut p, top);
                Ok(p)
            }
            acc::GET_REQUESTED_ORIENTATION => {
                let token = Self::token(call)?;
                let o = self
                    .0
                    .activities
                    .with(token, |r| r.requested_orientation)
                    .unwrap_or(SCREEN_ORIENTATION_UNSPECIFIED);
                let mut p = Parcel::new();
                acc::write_get_requested_orientation_reply(&mut p, o);
                Ok(p)
            }
            acc::SET_REQUESTED_ORIENTATION => {
                let token = Self::token(call)?;
                let orientation = call.data.read_i32().map_err(|_| BAD_VALUE)?;
                self.0
                    .activities
                    .with(token, |r| r.requested_orientation = orientation);
                empty()
            }
            code => reply(Err(self
                .0
                .unimplemented("activity client controller", code))),
        }
    }
}
