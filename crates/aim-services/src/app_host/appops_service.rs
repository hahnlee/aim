//! `appops`: AppOps for the app host, for the ops its services decide by
//! (the clipboard's, the stream volumes', the accounts' and the network
//! requests' WRITE_SETTINGS): an op's
//! mode is ignored while a user restriction it has is set (but for root
//! and system), else the uid's, else the package's, as the permission
//! service last persisted them (`access.abx`), else the op's default
//! (`AppOpsManager.sAppOpInfos` at `android-16.0.0_r1`). A foreground mode
//! (TAKE_AUDIO_FOCUS's, which audio focus notes) allows a uid while it has
//! a process, as each of the app host's processes is top
//! (`AppOpsUidStateTracker.evalMode`). The modes do not change while the
//! app host runs, so watchers are kept and never called; foreground
//! changes are not watched (#1529).

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::com_android_internal_app_iappopsservice as appops;

use super::{AppHost, reply};

/// `AppOpsManager.MODE_ALLOWED`, `MODE_IGNORED`, `MODE_ERRORED`.
const MODE_ALLOWED: i32 = 0;
const MODE_IGNORED: i32 = 1;
const MODE_ERRORED: i32 = 2;
pub(super) const MODE_DEFAULT: i32 = 3;
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
    (OP_GET_ACCOUNTS, "android:get_accounts", MODE_ALLOWED, None),
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

pub struct AppOps {
    host: Arc<AppHost>,
    watchers: Mutex<Vec<(i32, Binder)>>,
}

impl AppOps {
    pub fn new(host: Arc<AppHost>) -> Self {
        AppOps { host, watchers: Mutex::new(Vec::new()) }
    }
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
            code => return reply(Err(self.host.unimplemented("appops", code))),
        }
        Ok(p)
    }
}
