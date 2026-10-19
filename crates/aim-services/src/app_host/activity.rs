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
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iactivityclientcontroller as acc;
use aim_service_aidl::android_app_iactivitytaskmanager as atm;

use super::activity_stack::{FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS, FLAG_ACTIVITY_NEW_TASK};
use super::activity_start::{Component, FLAG_EXCLUDE_FROM_RECENTS, Report};
use super::config::{self, Configuration, SizeBuckets};
use super::kept::{RawBundle, RawIntent};
use super::packages::WindowStyle;
use super::{AppHost, reply};
use crate::package::info::ActivityInfo;

/// `ActivityManager.LOCK_TASK_MODE_NONE`.
const LOCK_TASK_MODE_NONE: i32 = 0;
/// `ActivityManager.PROCESS_STATE_UNKNOWN` and `PROCESS_STATE_CACHED_EMPTY`:
/// the resume's process state, and the launch's before the process state
/// is reported.
const PROCESS_STATE_UNKNOWN: i32 = -1;
const PROCESS_STATE_CACHED_EMPTY: i32 = 19;
/// `WindowConfiguration.ACTIVITY_TYPE_STANDARD`.
pub(super) const ACTIVITY_TYPE_STANDARD: i32 = 1;
/// `Display.DEFAULT_DISPLAY`, `Display.INVALID_DISPLAY`.
pub(super) const DEFAULT_DISPLAY: i32 = 0;
pub(super) const INVALID_DISPLAY: i32 = -1;
/// `Intent.EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN`, which a start
/// without a creator token gets; `UserHandle.USER_CURRENT`.
pub(super) const EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN: i32 = 1 << 2;
pub(super) const USER_CURRENT: i32 = -2;
/// `ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED`.
const SCREEN_ORIENTATION_UNSPECIFIED: i32 = -1;

/// `IRequestFinishCallback.requestFinish`, one-way.
const REQUEST_FINISH_DESCRIPTOR: &str = "android.app.IRequestFinishCallback";
const REQUEST_FINISH: u32 = 1;

/// A `ProfilerInfo`, read to be refused: its file, its fd, its sampling
/// interval, whether it stops, streams, its agent, whether the agent
/// attaches at bind, its clock and output version.
pub struct ProfilerInfo;

impl aim_service_aidl::ReadParcelable for ProfilerInfo {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        r.read_string16()?;
        if r.read_i32()? != 0 {
            r.read_fd()?;
        }
        r.read_i32()?;
        r.read_bool()?;
        r.read_bool()?;
        r.read_string16()?;
        r.read_bool()?;
        r.read_i32()?;
        r.read_i32()?;
        Ok(ProfilerInfo)
    }
}

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
    /// Whether it reported itself fully drawn (`reportFullyDrawn`).
    pub fully_drawn: bool,
    pub requested_orientation: i32,
    /// `ActivityInfo.getRealConfigChanged`: the changes it handles itself.
    pub config_changes: i32,
    /// The size buckets it reported (`reportSizeConfigurations`).
    pub size_buckets: Option<SizeBuckets>,
    /// The configuration it was last sent (`mLastReportedConfiguration`'s
    /// merged one).
    pub reported: Option<Configuration>,
    /// What it launches with and as, and its theme's window style.
    pub info: ActivityInfo,
    pub style: WindowStyle,
    pub intent: super::activity_start::LaunchIntent,
    pub referrer: String,
    /// Its task (the stack's key).
    pub task: i32,
    /// Who asked for its result, and how (`resultTo`, `resultWho`,
    /// `requestCode`).
    pub result_to: Option<u64>,
    pub result_who: Option<String>,
    pub request_code: i32,
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

/// The activities, by their token, and their tasks' stack.
#[derive(Default)]
pub struct Activities {
    pub(super) records: Mutex<HashMap<u64, ActivityRecord>>,
    next_ident: Mutex<i32>,
    pub(super) stack: Mutex<super::activity_stack::Stack>,
    /// The results and new intents the stack holds for delivery.
    pub(super) payloads: Mutex<HashMap<u64, super::activity_start::Payload>>,
    pub(super) next_payload: std::sync::atomic::AtomicU64,
    /// The window styles read, by base APK and theme (`AttributeCache`).
    styles: Mutex<HashMap<(String, u32), WindowStyle>>,
    /// The tasks' `WindowContainerToken`s, by task.
    pub(super) task_tokens: Mutex<HashMap<i32, Binder>>,
}

impl Activities {
    /// Whether `uid` has the focused task's top activity
    /// (`WindowManagerInternal.isUidFocused`).
    pub fn uid_focused(&self, uid: u32) -> bool {
        let top = self.stack.lock().unwrap().focused_top().map(|a| a.token);
        top.is_some_and(|t| {
            self.records
                .lock()
                .unwrap()
                .get(&t)
                .is_some_and(|r| r.uid == uid)
        })
    }

    /// The start of the process of the focused task's top activity
    /// (`getTopApp`).
    pub fn front_start(&self) -> Option<i64> {
        self.stack
            .lock()
            .unwrap()
            .focused_top()
            .map(|a| a.start_seq)
    }

    /// The component (`package/class`) of `uid`'s latest activity, a
    /// window's name.
    pub fn top_component(&self, uid: u32) -> Option<String> {
        let records = self.records.lock().unwrap();
        records
            .values()
            .filter(|r| r.uid == uid)
            .max_by_key(|r| r.started)
            .map(|r| format!("{}/{}", r.package, r.class))
    }

    /// Whether the process of `start_seq` has activities.
    pub fn has_start(&self, start_seq: i64) -> bool {
        self.records
            .lock()
            .unwrap()
            .values()
            .any(|r| r.start_seq == start_seq)
    }

    /// The activities of the process of `start_seq` are gone with it:
    /// their tokens, and what the stack does then; their results never
    /// sent go too.
    pub fn remove_start(&self, start_seq: i64) -> (Vec<u64>, Vec<super::activity_stack::Command>) {
        let gone = self
            .records
            .lock()
            .unwrap()
            .extract_if(|_, r| r.start_seq == start_seq)
            .map(|(token, _)| token)
            .collect();
        let mut stack = self.stack.lock().unwrap();
        let mut payloads = self.payloads.lock().unwrap();
        for result in stack
            .activities
            .iter()
            .filter(|a| a.start_seq == start_seq)
            .filter_map(|a| a.result)
        {
            payloads.remove(&result);
        }
        drop(payloads);
        (gone, stack.process_died(start_seq))
    }

    pub(super) fn with<T>(
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

impl AppHost {
    /// The window style of an activity of `info` (`getWindowStyle`): read
    /// from its package's resources once per APK and theme. One whose
    /// theme cannot be read is opaque, as the original's is when
    /// `AttributeCache` has no entry for it.
    pub(super) fn window_style(&self, info: &ActivityInfo) -> WindowStyle {
        let app = &info.info.application_info;
        let theme = if info.theme != 0 {
            info.theme
        } else {
            app.theme
        } as u32;
        let read = || -> Result<(String, Result<WindowStyle, String>), Exception> {
            let (state, _) = self.packages()?;
            let package = app.item.package_name.as_deref().unwrap_or_default();
            let pkg = state
                .packages
                .get(package)
                .and_then(|ps| ps.pkg.clone())
                .ok_or_else(|| Exception::illegal_argument(format!("no package {package}")))?;
            let apk = pkg.base_apk_path.clone().unwrap_or_default();
            if let Some(style) = self
                .activities
                .styles
                .lock()
                .unwrap()
                .get(&(apk.clone(), theme))
            {
                return Ok((apk, Ok(*style)));
            }
            let inputs = self.inputs()?;
            Ok((
                apk,
                inputs.packages.window_style(
                    self.root()?,
                    &inputs.data,
                    &pkg,
                    theme,
                    app.target_sdk_version,
                ),
            ))
        };
        match read() {
            Ok((apk, Ok(style))) => {
                self.activities
                    .styles
                    .lock()
                    .unwrap()
                    .insert((apk, theme), style);
                style
            }
            Ok((_, Err(e))) => {
                eprintln!("app host: the window style of theme {theme:#x}: {e}");
                WindowStyle::default()
            }
            Err(e) => {
                eprintln!(
                    "app host: the window style of theme {theme:#x}: {}",
                    e.message
                );
                WindowStyle::default()
            }
        }
    }
}

/// What a launch needs besides its record.
pub struct Launch<'a> {
    pub global: &'a Configuration,
    /// The bounds of its task in window mode, else the display's.
    pub task: Option<super::config::Rect>,
}

impl AppHost {
    /// `realStartActivityLocked`: the items that launch and resume the
    /// activity of `token`.
    pub(super) fn launch_items(
        &self,
        token: Binder,
        launch: Launch<'_>,
    ) -> Option<Vec<Box<dyn Item>>> {
        let Binder::Local(ptr) = token else {
            return None;
        };
        let mut records = self.activities.records.lock().unwrap();
        let record = records.get_mut(&ptr)?;
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
        let intent: Box<dyn WriteParcelable> = match &record.intent {
            super::activity_start::LaunchIntent::Component => Box::new(Intent {
                component: (record.package.clone(), record.class.clone()),
                flags: FLAG_ACTIVITY_NEW_TASK
                    | if record.info.flags & FLAG_EXCLUDE_FROM_RECENTS != 0 {
                        FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS
                    } else {
                        0
                    },
                extended_flags: EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN,
                content_user_hint: USER_CURRENT,
            }),
            super::activity_start::LaunchIntent::Raw(kept) => Box::new(kept.intent.clone()),
        };
        let items: Vec<Box<dyn Item>> = vec![
            Box::new(LaunchActivityItem {
                token,
                ident,
                current: launch.global.clone(),
                overrides: override_config,
                referrer: record.referrer.clone(),
                assist,
                shareable,
                caller_info,
                window: ActivityWindowInfo {
                    embedded: false,
                    task_bounds: bounds,
                    task_fragment_bounds: bounds,
                },
                intent,
                info: record.info.clone(),
            }),
            Box::new(ResumeActivityItem {
                token,
                proc_state: PROCESS_STATE_UNKNOWN,
                forward: true,
            }),
        ];
        Some(items)
    }
}

impl AppHost {
    /// `updateReportedConfigurationAndSend` for the activities of task
    /// `task` once it took `bounds`, each on a display of its uid's global
    /// configuration in `globals`: the items that tell them, each with its
    /// process's start, and whether one relaunches (its windows are not
    /// told they resized).
    pub(super) fn reconfigure_activities(
        &self,
        task: i32,
        globals: &HashMap<u32, Configuration>,
        bounds: config::Rect,
    ) -> (ProcessItems, bool) {
        self.reconfigure(
            |r| r.task == task,
            |uid| globals.get(&uid),
            |_| Some(bounds),
        )
    }

    /// `ensureActivityConfiguration` of every activity once the global
    /// configuration became `global` (`RootWindowContainer
    /// .onConfigurationChanged`): each in its freeform task's bounds
    /// (`freeform`), else the display's.
    pub(super) fn reconfigure_all_activities(
        &self,
        global: &Configuration,
        freeform: impl Fn(i32) -> Option<config::Rect>,
    ) -> ProcessItems {
        self.reconfigure(|_| true, |_| Some(global), freeform).0
    }

    /// `updateReportedConfigurationAndSend` of the activities `pick`
    /// picks, each on its uid's global configuration, in its task's
    /// freeform bounds if it has them.
    fn reconfigure<'g>(
        &self,
        pick: impl Fn(&ActivityRecord) -> bool,
        global_of: impl Fn(u32) -> Option<&'g Configuration>,
        freeform: impl Fn(i32) -> Option<config::Rect>,
    ) -> (ProcessItems, bool) {
        let mut records = self.activities.records.lock().unwrap();
        let mut items: ProcessItems = Vec::new();
        let mut relaunching = false;
        for (&ptr, record) in records.iter_mut().filter(|(_, r)| pick(r)) {
            let token = Binder::Local(ptr);
            let Some(global) = global_of(record.uid) else {
                continue;
            };
            let mut new = match freeform(record.task) {
                Some(bounds) => global.of_freeform_task(bounds),
                None => global.clone(),
            };
            new.window.activity_type = ACTIVITY_TYPE_STANDARD;
            let Some(old) = record.reported.replace(new.clone()) else {
                continue;
            };
            if old == new {
                continue;
            }
            let bounds = new.window.bounds;
            let window = ActivityWindowInfo {
                embedded: false,
                task_bounds: bounds,
                task_fragment_bounds: bounds,
            };
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
                items.push((
                    record.start_seq,
                    Box::new(ActivityConfigurationChangeItem {
                        token,
                        config: new,
                        window,
                    }),
                ));
                continue;
            }
            // `relaunchActivityLocked`: a resize alone keeps the window.
            let resize = config::CONFIG_SCREEN_SIZE
                | config::CONFIG_SMALLEST_SCREEN_SIZE
                | config::CONFIG_ORIENTATION
                | config::CONFIG_SCREEN_LAYOUT;
            relaunching = true;
            items.push((
                record.start_seq,
                Box::new(ActivityRelaunchItem {
                    token,
                    global: global.clone(),
                    config: new,
                    window,
                    changes,
                    preserve_window: changes & !resize == 0,
                }),
            ));
            items.push((
                record.start_seq,
                Box::new(if record.resumed {
                    Lifecycle::Resume(ResumeActivityItem {
                        token,
                        proc_state: PROCESS_STATE_UNKNOWN,
                        forward: false,
                    })
                } else {
                    Lifecycle::Pause(token)
                }),
            ));
            eprintln!(
                "app host: relaunching {}/{} for changes {changes:#x}",
                record.package, record.class
            );
        }
        (items, relaunching)
    }
}

/// `ResumeActivityItem` of a resumed activity coming back.
pub(super) fn resume_item(token: Binder) -> Box<dyn Item> {
    Box::new(ResumeActivityItem {
        token,
        proc_state: PROCESS_STATE_UNKNOWN,
        forward: false,
    })
}

/// `TopResumedActivityChangeItem`.
pub(super) fn top_resumed_item(token: Binder, on_top: bool) -> Box<dyn Item> {
    Box::new(TopResumedActivityChangeItem { token, on_top })
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

/// Items, each with the start of the process it goes to.
pub(super) type ProcessItems = Vec<(i64, Box<dyn Item>)>;

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
    intent: Box<dyn WriteParcelable>,
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
        self.intent.write_to(p);
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

/// `ConfigurationChangeItem`: a process's new global configuration, of
/// the default device.
pub(super) struct ConfigurationChangeItem {
    pub config: Configuration,
}

impl Item for ConfigurationChangeItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.ConfigurationChangeItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_i32(1);
        self.config.write_to(p);
        p.write_i32(0); // `Context.DEVICE_ID_DEFAULT`
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
                [false, false, true, false]
                    .iter()
                    .for_each(|v| p.write_bool(*v));
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
pub(super) struct Intent {
    pub component: (String, String),
    pub flags: i32,
    pub extended_flags: i32,
    pub content_user_hint: i32,
}

impl WriteParcelable for Intent {
    fn write_to(&self, p: &mut Parcel) {
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
            atm::START_ACTIVITY => {
                let a =
                    atm::StartActivity::<RawIntent, ProfilerInfo, RawBundle>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                if a.profiler_info.is_some() {
                    return reply(Err(self
                        .0
                        .unimplemented_named("activity_task", "a start with a profiler")));
                }
                let args = super::activity_start::StartArgs {
                    caller: a.caller,
                    intent: a.intent,
                    resolved_type: a.resolved_type,
                    result_to: a.result_to,
                    result_who: a.result_who,
                    request_code: a.request_code,
                };
                reply(self.0.start_activity(call, args).map(|code| {
                    let mut p = Parcel::new();
                    atm::write_start_activity_reply(&mut p, code);
                    p
                }))
            }
            // `startActivityIntentSender`: the record sent as `sendInner`
            // sends it, with the caller's result target and flags. Its
            // options are not applied (#1493).
            atm::START_ACTIVITY_INTENT_SENDER => {
                let a =
                    atm::StartActivityIntentSender::<RawIntent, RawBundle>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                reply((|| {
                    if super::is_isolated(uid) {
                        return Err(Exception::security(
                            "Isolated process not allowed to call startActivityIntentSender",
                        ));
                    }
                    if a.fill_in_intent.as_ref().is_some_and(RawIntent::has_fds) {
                        return Err(Exception::illegal_argument(
                            "File descriptors passed in Intent",
                        ));
                    }
                    let Some(target @ Binder::Local(_)) = a.target else {
                        return Err(Exception::illegal_argument("Bad PendingIntent object"));
                    };
                    let sent = super::pending_intents::Sent {
                        result_to: a.result_to,
                        result_who: a.result_who,
                        request_code: a.request_code,
                        flags_mask: a.flags_mask,
                        flags_values: a.flags_values,
                    };
                    let code = self.0.send_record(
                        a.caller,
                        target,
                        0,
                        a.fill_in_intent,
                        a.resolved_type,
                        None,
                        None,
                        sent,
                    )?;
                    let mut p = Parcel::new();
                    atm::write_start_activity_intent_sender_reply(&mut p, code);
                    Ok(p)
                })())
            }
            // `filterOnlyVisibleRecents` filters none of the app host's
            // tasks (`task_info::running_tasks`).
            atm::GET_TASKS => {
                let a = atm::GetTasks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .get_tasks(
                            call.sender_euid as i32,
                            a.max_num,
                            a.keep_intent_extra,
                            a.display_id,
                        )
                        .map(|list| {
                            let mut p = Parcel::new();
                            atm::write_get_tasks_reply(&mut p, Some(&list));
                            p
                        }),
                )
            }
            // `List<IBinder> getAppTasks(String callingPackage)`, read and
            // written by hand (`writeBinderList`).
            atm::GET_APP_TASKS => {
                call.data
                    .enforce_interface(atm::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let package = call.data.read_string16().map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .app_tasks(call.sender_euid as i32, package.as_deref())
                        .map(|tasks| {
                            let mut p = Parcel::new();
                            p.write_no_exception();
                            p.write_i32(tasks.len() as i32);
                            for task in tasks {
                                p.write_binder(Some(task));
                            }
                            p
                        }),
                )
            }
            atm::GET_DEVICE_CONFIGURATION_INFO => {
                atm::GetDeviceConfigurationInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .device_configuration_info(call.sender_pid)
                        .map(|info| {
                            let mut p = Parcel::new();
                            atm::write_get_device_configuration_info_reply(&mut p, Some(&info));
                            p
                        }),
                )
            }
            // `LockTaskController.getLockTaskModeState`: the app host starts
            // no lock task or screen pinning, so none is on.
            atm::GET_LOCK_TASK_MODE_STATE => {
                atm::GetLockTaskModeState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                atm::write_get_lock_task_mode_state_reply(&mut p, LOCK_TASK_MODE_NONE);
                Ok(p)
            }
            atm::GET_ACTIVITY_CLIENT_CONTROLLER => {
                atm::GetActivityClientController::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                atm::write_get_activity_client_controller_reply(&mut p, Some(self.1));
                Ok(p)
            }
            atm::REGISTER_BACKGROUND_ACTIVITY_START_CALLBACK => {
                let a = atm::RegisterBackgroundActivityStartCallback::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let added = self
                    .0
                    .add_bal_strict_mode_callback(call.sender_euid as i32, a.binder);
                let mut p = Parcel::new();
                atm::write_register_background_activity_start_callback_reply(&mut p, added);
                Ok(p)
            }
            atm::UNREGISTER_BACKGROUND_ACTIVITY_START_CALLBACK => {
                let a = atm::UnregisterBackgroundActivityStartCallback::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.0
                    .remove_bal_strict_mode_callback(call.sender_euid as i32, a.binder);
                let mut p = Parcel::new();
                atm::write_unregister_background_activity_start_callback_reply(&mut p);
                Ok(p)
            }
            code => reply(Err(self.0.unimplemented("activity_task", code))),
        }
    }
}

/// `android::base::ParseInt` of an `int` (`SystemProperties.getInt`):
/// `strtoll` in base 0, after leading whitespace, with nothing after it.
pub(super) fn parse_int(s: &str) -> Option<i32> {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, s) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, hex)
    } else if s.len() > 1 && s.starts_with('0') {
        (8, &s[1..])
    } else {
        (10, s)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = i64::from_str_radix(digits, radix).ok()?;
    i32::try_from(if negative { -magnitude } else { magnitude }).ok()
}

/// `ConfigurationInfo`.
pub(super) struct ConfigurationInfo {
    touch_screen: i32,
    keyboard_type: i32,
    navigation: i32,
    input_features: i32,
    gl_es_version: i32,
}

impl WriteParcelable for ConfigurationInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.touch_screen);
        p.write_i32(self.keyboard_type);
        p.write_i32(self.navigation);
        p.write_i32(self.input_features);
        p.write_i32(self.gl_es_version);
    }
}

impl AppHost {
    /// `getDeviceConfigurationInfo`: of the calling process's configuration
    /// (`getGlobalConfigurationForCallingPid`), and the GLES version
    /// `ro.opengles.version` gives (`GL_ES_VERSION_UNDEFINED` without one).
    fn device_configuration_info(&self, pid: i32) -> Result<ConfigurationInfo, Exception> {
        /// `Configuration.KEYBOARD_*`, `NAVIGATION_*`;
        /// `ConfigurationInfo.INPUT_FEATURE_*`.
        const KEYBOARD_UNDEFINED: i32 = 0;
        const KEYBOARD_NOKEYS: i32 = 1;
        const NAVIGATION_DPAD: i32 = 2;
        const NAVIGATION_TRACKBALL: i32 = 3;
        const INPUT_FEATURE_HARD_KEYBOARD: i32 = 0x1;
        const INPUT_FEATURE_FIVE_WAY_NAV: i32 = 0x2;
        let bound = self
            .apps
            .lock()
            .unwrap()
            .values()
            .find(|a| a.pid == pid && a.thread.is_some())
            .and_then(|a| a.config.clone());
        let config = match bound {
            Some(config) => config,
            None => self.global_configuration()?,
        };
        let mut input_features = 0;
        if matches!(config.navigation, NAVIGATION_DPAD | NAVIGATION_TRACKBALL) {
            input_features |= INPUT_FEATURE_FIVE_WAY_NAV;
        }
        if !matches!(config.keyboard, KEYBOARD_UNDEFINED | KEYBOARD_NOKEYS) {
            input_features |= INPUT_FEATURE_HARD_KEYBOARD;
        }
        Ok(ConfigurationInfo {
            touch_screen: config.touchscreen,
            keyboard_type: config.keyboard,
            navigation: config.navigation,
            input_features,
            gl_es_version: self
                .property("ro.opengles.version")
                .and_then(|v| parse_int(&v))
                .unwrap_or(0),
        })
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
                if let Some(start_seq) = self.0.activities.with(token, |r| r.start_seq) {
                    self.0.activity_changed(start_seq);
                }
                empty()
            }
            acc::ACTIVITY_IDLE => {
                let token = Self::token(call)?;
                self.report(token, "idle", |_| {});
                if let Some(task) = self.0.activities.with(token, |r| r.task) {
                    self.0.task_idle(task);
                }
                self.0.activity_reported(token, Report::Idle);
                empty()
            }
            // `ActivityMetricsLogger.notifyFullyDrawn`: its launch's first
            // report is logged (one-way).
            acc::REPORT_ACTIVITY_FULLY_DRAWN => {
                let a =
                    acc::ReportActivityFullyDrawn::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if self
                    .0
                    .activities
                    .with(a.token, |r| !std::mem::replace(&mut r.fully_drawn, true))
                    == Some(true)
                {
                    self.report(a.token, "fully drawn", |_| {});
                }
                Ok(Parcel::new())
            }
            acc::ACTIVITY_TOP_RESUMED_STATE_LOST => {
                call.data
                    .enforce_interface(acc::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                self.0.top_resumed_lost();
                empty()
            }
            // `setShowWhenLocked`, `setInheritShowWhenLocked` (one-way) and
            // `setTurnScreenOn` matter to a locked or sleeping display; the
            // app host's is never locked and always on (`power`), so
            // nothing follows, as on the original's awake, unlocked one.
            acc::SET_SHOW_WHEN_LOCKED | acc::SET_INHERIT_SHOW_WHEN_LOCKED => Ok(Parcel::new()),
            // `setRecentsScreenshotEnabled` (one-way) chooses between a
            // screenshot and the app's theme for its task's snapshot in
            // recents (`shouldUseAppThemeSnapshot`); the app host takes no
            // task snapshots, so nothing follows.
            acc::SET_RECENTS_SCREENSHOT_ENABLED => {
                acc::SetRecentsScreenshotEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok(Parcel::new())
            }
            // `overridePendingTransition` gives the transition being
            // collected its animation (`TransitionController
            // .setOverrideAnimation`); the app host collects none (#1725), so
            // nothing follows, as on the original when none is collecting.
            acc::OVERRIDE_PENDING_TRANSITION => {
                acc::OverridePendingTransition::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                acc::write_override_pending_transition_reply(&mut p);
                Ok(p)
            }
            // `enableTaskLocaleOverride` (the system's alone) aligns a
            // task's other packages with the locales its root package has
            // of its own; no package has any in the app host (#1642), so
            // nothing follows, as on an original where none set them.
            acc::ENABLE_TASK_LOCALE_OVERRIDE => {
                acc::EnableTaskLocaleOverride::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                acc::write_enable_task_locale_override_reply(&mut p);
                Ok(p)
            }
            acc::SET_TURN_SCREEN_ON => empty(),
            acc::GET_TASK_FOR_ACTIVITY => {
                let token = Self::token(call)?;
                let only_root = call.data.read_bool().map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                p.write_no_exception();
                p.write_i32(self.0.task_of_activity(token, only_root));
                Ok(p)
            }
            // `finishRelaunching`: its relaunch is done (oneway).
            acc::ACTIVITY_RELAUNCHED => {
                let token = Self::token(call)?;
                self.report(token, "relaunched", |_| {});
                Ok(Parcel::new())
            }
            acc::ACTIVITY_PAUSED => {
                let token = Self::token(call)?;
                self.report(token, "paused", |r| r.resumed = false);
                if let Some(Binder::Local(t)) = token {
                    self.0
                        .activity_usage(t, super::app_standby::Event::ActivityPaused);
                }
                self.0.activity_reported(token, Report::Paused);
                empty()
            }
            acc::ACTIVITY_STOPPED | acc::ACTIVITY_DESTROYED => {
                let token = Self::token(call)?;
                let stopped = call.code == acc::ACTIVITY_STOPPED;
                self.report(token, if stopped { "stopped" } else { "destroyed" }, |_| {});
                if let Some(Binder::Local(t)) = token {
                    self.0.activity_usage_stats(
                        t,
                        if stopped {
                            super::usage_events::ACTIVITY_STOPPED
                        } else {
                            super::usage_events::ACTIVITY_DESTROYED
                        },
                    );
                }
                self.0.activity_reported(
                    token,
                    if stopped {
                        Report::Stopped
                    } else {
                        Report::Destroyed
                    },
                );
                empty()
            }
            acc::FINISH_ACTIVITY => {
                let a = acc::FinishActivity::<RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .finish_activity(
                            call.sender_euid as i32,
                            a.token,
                            a.code,
                            a.data,
                            a.finish_task,
                        )
                        .map(|finished| {
                            let mut p = Parcel::new();
                            acc::write_finish_activity_reply(&mut p, finished);
                            p
                        }),
                )
            }
            acc::GET_CALLING_ACTIVITY | acc::GET_CALLING_PACKAGE => {
                let token = Self::token(call)?;
                let calling = self.0.calling_activity(token);
                let mut p = Parcel::new();
                if call.code == acc::GET_CALLING_ACTIVITY {
                    let component = calling.as_ref().map(|(p, c)| Component(p, c));
                    acc::write_get_calling_activity_reply(&mut p, component.as_ref());
                } else {
                    acc::write_get_calling_package_reply(&mut p, &calling.map(|(p, _)| p));
                }
                Ok(p)
            }
            // `onBackPressed` (one-way): the root of a task the launcher
            // started goes to the back (#1486); another activity finishes
            // (`IRequestFinishCallback.requestFinish`).
            acc::ON_BACK_PRESSED => {
                let a = acc::OnBackPressed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if self.0.launcher_root(a.activity_token) {
                    let _ = self.0.unimplemented_named(
                        "activity client controller",
                        "a task to the back (#1486)",
                    );
                } else if let Some(Binder::Handle(callback)) = a.callback {
                    let mut p = Parcel::new();
                    p.write_interface_token(REQUEST_FINISH_DESCRIPTOR);
                    if let Err(e) = self.0.process.transact(callback, REQUEST_FINISH, &p, true) {
                        eprintln!("app host: requestFinish: {e}");
                    }
                }
                Ok(Parcel::new())
            }
            acc::MOVE_ACTIVITY_TASK_TO_BACK => reply(Err(self
                .0
                .unimplemented_named("activity client controller", "a task to the back (#1486)"))),
            // The size buckets decide which size changes relaunch it.
            acc::REPORT_SIZE_CONFIGURATIONS => {
                let args = acc::ReportSizeConfigurations::<SizeBuckets>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.0
                    .activities
                    .with(args.token, |r| r.size_buckets = args.size_configurations);
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
            acc::CONVERT_FROM_TRANSLUCENT => {
                let a = acc::ConvertFromTranslucent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                acc::write_convert_from_translucent_reply(
                    &mut p,
                    self.0.set_occludes_parent(a.token, true),
                );
                Ok(p)
            }
            // The options are a scene transition's to return to the
            // activity below, which the app host does not run (#1587).
            acc::CONVERT_TO_TRANSLUCENT => {
                let a = acc::ConvertToTranslucent::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if a.options.is_some() {
                    return reply(Err(self.0.unimplemented_named(
                        "activity client controller",
                        "convertToTranslucent with a scene transition (#1587)",
                    )));
                }
                let mut p = Parcel::new();
                acc::write_convert_to_translucent_reply(
                    &mut p,
                    self.0.set_occludes_parent(a.token, false),
                );
                Ok(p)
            }
            // The task's description is the Mac window's once activities
            // have windows (#1422).
            acc::SET_TASK_DESCRIPTION => empty(),
            acc::IS_TOP_OF_TASK => {
                let token = Self::token(call)?;
                let top = match token {
                    Some(Binder::Local(t)) => {
                        let stack = self.0.activities.stack.lock().unwrap();
                        stack
                            .get(t)
                            .and_then(|a| stack.top(a.task))
                            .is_some_and(|a| a.token == t)
                    }
                    _ => false,
                };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Integer properties parse as `ParseInt` does.
    #[test]
    fn int_properties_parse_as_parse_int() {
        assert_eq!(parse_int("196610"), Some(196610));
        assert_eq!(parse_int(" 0x30002"), Some(0x30002));
        assert_eq!(parse_int("010"), Some(8));
        assert_eq!(parse_int("-0"), Some(0));
        assert_eq!(parse_int("196610 "), None);
        assert_eq!(parse_int("3.2"), None);
        assert_eq!(parse_int("4294967296"), None);
    }

    #[test]
    fn a_configuration_change_is_the_configuration_then_the_default_device() {
        let config = Configuration::default();
        let mut expected = Parcel::new();
        expected.write_i32(1);
        config.write_to(&mut expected);
        expected.write_i32(0);
        let mut p = Parcel::new();
        ConfigurationChangeItem { config }.write_fields(&mut p);
        assert_eq!(p.data(), expected.data());
    }
}
