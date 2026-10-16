//! Activities: ActivityTaskManager's side of an activity's start, as
//! `ActivityTaskSupervisor.realStartActivityLocked` makes it at
//! `android-16.0.0_r1` (one `ClientTransaction` of a `LaunchActivityItem`
//! and a `ResumeActivityItem`, then a `TopResumedActivityChangeItem`), and
//! `IActivityClientController`, where the activity reports back.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iactivityclientcontroller as acc;
use aim_service_aidl::android_app_iactivitytaskmanager as atm;

use super::config::Configuration;
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
}

impl AppHost {
    /// `realStartActivityLocked`: the transaction that launches and
    /// resumes `record`'s activity, then the one that makes it the top
    /// resumed activity.
    pub(super) fn launch_activity(
        &self,
        record: ActivityRecord,
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
        let mut override_config = launch.global.clone();
        override_config.window.activity_type = ACTIVITY_TYPE_STANDARD;
        let bounds = launch.global.window.bounds;
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

/// `IApplicationThread.scheduleTransaction` of `items`.
fn transaction(items: Vec<Box<dyn Item>>) -> Parcel {
    let mut p = Parcel::new();
    aim_service_aidl::android_app_iapplicationthread::ScheduleTransaction {
        transaction: Some(ClientTransaction(items)),
    }
    .write(&mut p);
    p
}

/// A `ClientTransactionItem`: its class, for `writeParcelable`, and its
/// fields.
trait Item {
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

struct ActivityWindowInfo {
    embedded: bool,
    task_bounds: super::config::Rect,
    task_fragment_bounds: super::config::Rect,
}

impl ActivityWindowInfo {
    fn write(&self, p: &mut Parcel) {
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
                empty()
            }
            acc::ACTIVITY_TOP_RESUMED_STATE_LOST => empty(),
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
            // The size buckets decide which configuration changes relaunch
            // the activity; a fixed display changes none.
            acc::REPORT_SIZE_CONFIGURATIONS => empty(),
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
