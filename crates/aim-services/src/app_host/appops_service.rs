//! `appops`: AppOps for the app host, for the ops its services decide by
//! (the clipboard's, the stream volumes', the accounts', usage stats',
//! running in the background's (and any in it) and the network requests'
//! WRITE_SETTINGS): an op's mode is ignored while a user restriction it
//! has is set (but for root and system), else the uid's, else the
//! package's, as the permission service last persisted them
//! (`access.abx`), else the op's default
//! (`AppOpsManager.sAppOpInfos` at `android-16.0.0_r1`). A foreground mode
//! (TAKE_AUDIO_FOCUS's, which audio focus notes) allows a uid while it has
//! a process, as each of the app host's processes is top
//! (`AppOpsUidStateTracker.evalMode`). The modes do not change while the
//! app host runs, so mode watchers are kept and never called, as are
//! active watchers, no op being started; foreground
//! changes are not watched (#1529). An op noted for an app by another uid
//! with `shouldCollectAsyncNotedOp` goes to the app's async-noted watchers
//! (`collectAsyncNotedOp`), or waits for its `extractAsyncOps` when none
//! took it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::com_android_internal_app_iappopsservice as appops;

use super::{AppHost, reply};

/// `AppOpsManager.MODE_ALLOWED`, `MODE_IGNORED`, `MODE_ERRORED`.
pub(super) const MODE_ALLOWED: i32 = 0;
const MODE_IGNORED: i32 = 1;
const MODE_ERRORED: i32 = 2;
pub(super) const MODE_DEFAULT: i32 = 3;
/// `AppOpsManager._NUM_OP`.
const NUM_OP: i32 = 163;
const MODE_FOREGROUND: i32 = 4;
/// `AppOpsManager.OP_SCHEDULE_EXACT_ALARM`, `OP_USE_FULL_SCREEN_INTENT`,
/// `OP_READ_CONTACTS`, `OP_WRITE_CONTACTS`, `OP_GET_ACCOUNTS`.
pub(super) const OP_SCHEDULE_EXACT_ALARM: i32 = 107;
pub(super) const OP_USE_FULL_SCREEN_INTENT: i32 = 133;
pub(super) const OP_READ_CONTACTS: i32 = 4;
pub(super) const OP_WRITE_CONTACTS: i32 = 5;
pub(super) const OP_GET_ACCOUNTS: i32 = 62;
/// `AppOpsManager.OP_WRITE_SETTINGS`, `OP_TAKE_AUDIO_FOCUS`.
pub(super) const OP_WRITE_SETTINGS: i32 = 23;
pub(super) const OP_TAKE_AUDIO_FOCUS: i32 = 32;
/// `UserManager.DISALLOW_ADJUST_VOLUME`.
const DISALLOW_ADJUST_VOLUME: &str = "no_adjust_volume";
/// The ops it decides: their code, name, default mode and user
/// restriction. Each is its own switch, and none has a permission but
/// SCHEDULE_EXACT_ALARM's, USE_FULL_SCREEN_INTENT's, the accounts'
/// (READ_CONTACTS, WRITE_CONTACTS, GET_ACCOUNTS) and WRITE_SETTINGS',
/// which AlarmManager, NotificationManager, AccountManager and
/// ConnectivityService only check or note themselves, so their notes are
/// not collected (`AppOpsService.shouldCollectNotes`).
const OPS: &[(i32, &str, i32, Option<&str>)] = &[
    (OP_READ_CONTACTS, "android:read_contacts", MODE_ALLOWED, None),
    (OP_WRITE_CONTACTS, "android:write_contacts", MODE_ALLOWED, None),
    (OP_WRITE_SETTINGS, "android:write_settings", MODE_DEFAULT, None),
    (29, "android:read_clipboard", MODE_ALLOWED, None),
    (30, "android:write_clipboard", MODE_ALLOWED, None),
    (OP_TAKE_AUDIO_FOCUS, "android:take_audio_focus", MODE_FOREGROUND, None),
    (34, "android:audio_voice_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (35, "android:audio_ring_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (36, "android:audio_media_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (37, "android:audio_alarm_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (38, "android:audio_notification_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (39, "android:audio_bluetooth_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (43, "android:get_usage_stats", MODE_DEFAULT, None),
    (OP_GET_ACCOUNTS, "android:get_accounts", MODE_ALLOWED, None),
    (63, "android:run_in_background", MODE_ALLOWED, None),
    (70, "android:run_any_in_background", MODE_ALLOWED, None),
    (64, "android:audio_accessibility_volume", MODE_ALLOWED, Some(DISALLOW_ADJUST_VOLUME)),
    (OP_SCHEDULE_EXACT_ALARM, "android:schedule_exact_alarm", MODE_DEFAULT, None),
    (OP_USE_FULL_SCREEN_INTENT, "android:use_full_screen_intent", MODE_DEFAULT, None),
];

/// `noteOperation`'s mode: an op of a package not `uid`'s errs.
pub(super) fn note(host: &AppHost, code: i32, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
    match package_mode(host, uid, package)? {
        MODE_ALLOWED => mode(host, code, uid, package),
        _ => Ok(MODE_ERRORED),
    }
}

fn op(code: i32) -> Result<&'static (i32, &'static str, i32, Option<&'static str>), Exception> {
    OPS.iter()
        .find(|(c, ..)| *c == code)
        .ok_or_else(|| Exception::new(EX_UNSUPPORTED_OPERATION, format!("app op {code} is not decided by the app host")))
}

/// `checkOperation`: ignored under its restriction, else the uid's mode,
/// else the package's, else the op's default, a foreground mode evaluated.
pub(super) fn mode(host: &AppHost, code: i32, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
    let (_, name, default, restriction) = *op(code)?;
    let app_id = uid.rem_euclid(100_000);
    if let Some(restriction) = restriction
        && !matches!(app_id, 0 | 1000)
        && host.user_restrictions()?.contains(restriction)
    {
        return Ok(MODE_IGNORED);
    }
    let access = host.inputs()?.packages.access();
    let named = |ops: &[(String, i32)]| ops.iter().find(|(n, _)| n == name).map(|(_, m)| *m);
    let uid_mode = access.and_then(|a| a.app_id_app_ops.iter().find(|(id, _)| *id == app_id)).and_then(|(_, ops)| named(ops));
    let package_mode = package
        .and_then(|p| access.and_then(|a| a.package_app_ops.iter().find(|(name, _)| name == p)))
        .and_then(|(_, ops)| named(ops));
    Ok(match uid_mode.or(package_mode).unwrap_or(default) {
        MODE_FOREGROUND if host.uid_running(uid) => MODE_ALLOWED,
        MODE_FOREGROUND => MODE_IGNORED,
        mode => mode,
    })
}

/// `checkPackage`: whether `package` is `uid`'s.
fn package_mode(host: &AppHost, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
    let (state, _) = host.packages()?;
    let owns = package.and_then(|p| state.packages.get(p)).is_some_and(|ps| ps.app_id == uid.rem_euclid(100_000));
    Ok(if owns { MODE_ALLOWED } else { MODE_ERRORED })
}

/// `IAppOpsAsyncNotedCallback`'s descriptor and `opNoted`.
const ASYNC_NOTED_DESCRIPTOR: &str = "com.android.internal.app.IAppOpsAsyncNotedCallback";
const OP_NOTED: u32 = 1;
/// `AppOpsService.MAX_UNFORWARDED_OPS`.
const MAX_UNFORWARDED_OPS: usize = 10;

/// `AsyncNotedAppOp`.
struct AsyncNoted {
    op: i32,
    noting_uid: i32,
    attribution_tag: Option<String>,
    message: String,
    time: i64,
}

/// Its parcel: which nullable fields follow, then the fields.
impl WriteParcelable for AsyncNoted {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(if self.attribution_tag.is_some() { 0x4 } else { 0 });
        p.write_i32(self.op);
        p.write_i32(self.noting_uid);
        if let Some(tag) = &self.attribution_tag {
            p.write_string16(Some(tag));
        }
        p.write_string16(Some(&self.message));
        p.write_i64(self.time);
    }
}

/// An app's async-noted watchers and the ops none took, by package and
/// uid (`mAsyncOpWatchers`, `mUnforwardedAsyncNotedOps`).
#[derive(Default)]
struct Async {
    watchers: HashMap<(String, i32), Vec<(Strong, u64)>>,
    unforwarded: HashMap<(String, i32), Vec<AsyncNoted>>,
}

/// The active watchers by their binder (`mActiveWatchers`): each op's
/// watched uid (`Process.INVALID_UID` for all). No op is started in the
/// app host, so none is called.
type ActiveWatchers = HashMap<u32, (Strong, u64, Vec<(i32, i32)>)>;

pub struct AppOps {
    host: Arc<AppHost>,
    watchers: Mutex<Vec<(i32, Binder)>>,
    active_watchers: Arc<Mutex<ActiveWatchers>>,
    async_noted: Arc<Mutex<Async>>,
}

impl AppOps {
    pub fn new(host: Arc<AppHost>) -> Self {
        AppOps { host, watchers: Mutex::new(Vec::new()), active_watchers: Default::default(), async_noted: Default::default() }
    }

    /// `startWatchingActive`: the caller's own uid's ops watched, unless it
    /// holds WATCH_APPOPS.
    fn start_watching_active(&self, uid: i32, ops: Option<Vec<i32>>, callback: Option<Binder>) -> Result<(), Exception> {
        const INVALID_UID: i32 = -1;
        let watched = if self.host.check_permission(Some("android.permission.WATCH_APPOPS"), uid)? == 0 { INVALID_UID } else { uid };
        // `Preconditions.checkArrayElementsInRange`, of the ops given.
        let given = ops.as_deref().unwrap_or_default();
        let name = format!("Invalid op code in: [{}]", given.iter().map(i32::to_string).collect::<Vec<_>>().join(", "));
        for (i, &op) in given.iter().enumerate() {
            let side = if op < 0 { "too low" } else if op > NUM_OP - 1 { "too high" } else { continue };
            return Err(Exception::illegal_argument(format!("{name}[{i}] is out of range of [0, {}] ({side})", NUM_OP - 1)));
        }
        let Some(callback) = callback else { return Ok(()) };
        let ops = ops.ok_or_else(|| Exception::new(EX_NULL_POINTER, "ops"))?;
        let Binder::Handle(handle) = callback else {
            return Err(Exception::illegal_argument("startWatchingActive: a local callback"));
        };
        let mut watchers = self.active_watchers.lock().unwrap();
        let entry = watchers.entry(handle).or_insert_with(|| {
            // `ActiveCallback` links to its binder's death.
            let strong = self.host.process.strong(handle);
            let state = Arc::downgrade(&self.active_watchers);
            let death = self.host.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(state) = state.upgrade() {
                        state.lock().unwrap().remove(&handle);
                    }
                }),
            );
            (strong, death, Vec::new())
        });
        for op in ops {
            entry.2.retain(|(o, _)| *o != op);
            entry.2.push((op, watched));
        }
        Ok(())
    }

    /// `stopWatchingActive`.
    fn stop_watching_active(&self, callback: Option<Binder>) {
        if let Some(Binder::Handle(handle)) = callback
            && let Some((strong, death, _)) = self.active_watchers.lock().unwrap().remove(&handle)
        {
            self.host.process.clear_death(&strong, death);
        }
    }

    /// `verifyAndGetBypass`'s package check.
    fn verify(&self, uid: i32, package: &str) -> Result<(), Exception> {
        match package_mode(&self.host, uid, Some(package))? {
            MODE_ALLOWED => Ok(()),
            _ => Err(Exception::security(format!("Specified package \"{package}\" under uid {uid} but it is not"))),
        }
    }

    /// `collectAsyncNotedOp`: the op noted for `package` of `uid` by
    /// `noting_uid`, to its watchers, else kept for `extractAsyncOps`.
    fn collect(&self, uid: i32, package: String, op: i32, attribution_tag: Option<String>, message: String, noting_uid: i32) {
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let noted = AsyncNoted { op, noting_uid, attribution_tag, message, time };
        let key = (package, uid);
        let mut state = self.async_noted.lock().unwrap();
        let Some(watchers) = state.watchers.get(&key) else { return };
        let mut p = Parcel::new();
        p.write_interface_token(ASYNC_NOTED_DESCRIPTOR);
        p.write_i32(1);
        noted.write_to(&mut p);
        let forwarded = watchers.iter().fold(false, |sent, (w, _)| w.transact(OP_NOTED, &p, true).is_ok() || sent);
        if !forwarded {
            let unforwarded = state.unforwarded.entry(key).or_default();
            unforwarded.push(noted);
            if unforwarded.len() > MAX_UNFORWARDED_OPS {
                unforwarded.remove(0);
            }
        }
    }

    /// `startWatchingAsyncNoted`: a watcher of the caller's `package`,
    /// gone at its death.
    fn start_watching_async(&self, uid: i32, package: String, callback: Binder) -> Result<(), Exception> {
        self.verify(uid, &package)?;
        let Binder::Handle(handle) = callback else {
            return Err(Exception::illegal_argument("startWatchingAsyncNoted: a local callback"));
        };
        let strong = self.host.process.strong(handle);
        let state = Arc::downgrade(&self.async_noted);
        let key = (package, uid);
        let dead = key.clone();
        let death = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(state) = state.upgrade() {
                    remove_watcher(&mut state.lock().unwrap(), &dead, handle);
                }
            }),
        );
        self.async_noted.lock().unwrap().watchers.entry(key).or_default().push((strong, death));
        Ok(())
    }
}

/// Removes the watcher `handle` of `key`, and the key with its last.
fn remove_watcher(state: &mut Async, key: &(String, i32), handle: u32) -> Option<(Strong, u64)> {
    let watchers = state.watchers.get_mut(key)?;
    let i = watchers.iter().position(|(w, _)| w.handle == handle)?;
    let removed = watchers.remove(i);
    if watchers.is_empty() {
        state.watchers.remove(key);
    }
    Some(removed)
}

impl Service for AppOps {
    fn descriptor(&self) -> &str {
        appops::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            appops::CHECK_PACKAGE => {
                let args = appops::CheckPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match package_mode(&self.host, args.uid, args.package_name.as_deref()) {
                    Ok(mode) => appops::write_check_package_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::CHECK_OPERATION_FOR_DEVICE => {
                let args = appops::CheckOperationForDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match mode(&self.host, args.code, args.uid, args.package_name.as_deref()) {
                    Ok(mode) => appops::write_check_operation_for_device_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::NOTE_OPERATION => {
                let args = appops::NoteOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mode = match note(&self.host, args.code, args.uid, args.package_name.as_deref()) {
                    Ok(mode) => mode,
                    Err(e) => return reply(Err(e)),
                };
                if mode == MODE_ALLOWED
                    && args.should_collect_async_noted_op
                    && let Some(package) = args.package_name.clone()
                {
                    let message = args.message.clone().unwrap_or_default();
                    self.collect(args.uid, package, args.code, args.attribution_tag.clone(), message, call.sender_euid as i32);
                }
                // `SyncNotedAppOp`: which nullable fields follow, the mode,
                // the op, the attribution tag, the package.
                p.write_no_exception();
                p.write_i32(1);
                let tag = args.attribution_tag.as_deref();
                let package = args.package_name.as_deref();
                p.write_i32(if tag.is_some() { 0x4 } else { 0 } | if package.is_some() { 0x8 } else { 0 });
                p.write_i32(mode);
                p.write_i32(args.code);
                if let Some(tag) = tag {
                    p.write_string16(Some(tag));
                }
                if let Some(package) = package {
                    p.write_string16(Some(package));
                }
            }
            appops::SHOULD_COLLECT_NOTES => {
                let args = appops::ShouldCollectNotes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = op(args.op_code) {
                    return reply(Err(e));
                }
                appops::write_should_collect_notes_reply(&mut p, false);
            }
            appops::START_WATCHING_MODE_WITH_FLAGS => {
                let args = appops::StartWatchingModeWithFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(callback) = args.callback {
                    self.watchers.lock().unwrap().push((args.op, callback));
                }
                appops::write_start_watching_mode_with_flags_reply(&mut p);
            }
            appops::START_WATCHING_ACTIVE => {
                let args = appops::StartWatchingActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.start_watching_active(call.sender_euid as i32, args.ops, args.callback) {
                    return reply(Err(e));
                }
                appops::write_start_watching_active_reply(&mut p);
            }
            appops::STOP_WATCHING_ACTIVE => {
                let args = appops::StopWatchingActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.stop_watching_active(args.callback);
                appops::write_stop_watching_active_reply(&mut p);
            }
            appops::START_WATCHING_ASYNC_NOTED => {
                let args = appops::StartWatchingAsyncNoted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (Some(package), Some(callback)) = (args.package_name, args.callback) else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "packageName or callback")));
                };
                if let Err(e) = self.start_watching_async(call.sender_euid as i32, package, callback) {
                    return reply(Err(e));
                }
                appops::write_start_watching_async_noted_reply(&mut p);
            }
            appops::STOP_WATCHING_ASYNC_NOTED => {
                let args = appops::StopWatchingAsyncNoted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (Some(package), Some(callback)) = (args.package_name, args.callback) else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "packageName or callback")));
                };
                let uid = call.sender_euid as i32;
                if let Err(e) = self.verify(uid, &package) {
                    return reply(Err(e));
                }
                if let Binder::Handle(handle) = callback
                    && let Some((strong, death)) = remove_watcher(&mut self.async_noted.lock().unwrap(), &(package, uid), handle)
                {
                    self.host.process.clear_death(&strong, death);
                }
                appops::write_stop_watching_async_noted_reply(&mut p);
            }
            appops::EXTRACT_ASYNC_OPS => {
                let args = appops::ExtractAsyncOps::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(package) = args.package_name else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "packageName")));
                };
                let uid = call.sender_euid as i32;
                if let Err(e) = self.verify(uid, &package) {
                    return reply(Err(e));
                }
                // Null when none waits.
                let ops = self.async_noted.lock().unwrap().unforwarded.remove(&(package, uid));
                let ops: Option<Vec<Option<AsyncNoted>>> = ops.map(|ops| ops.into_iter().map(Some).collect());
                appops::write_extract_async_ops_reply(&mut p, ops.as_deref());
            }
            code => return reply(Err(self.host.unimplemented("appops", code))),
        }
        Ok(p)
    }
}
