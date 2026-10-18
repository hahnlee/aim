//! `appops`: AppOps for the app host (`AppOpsService` at
//! `android-16.0.0_r1`), deciding every op of `sAppOpInfos`
//! (`app_op_infos`) as `checkOperationImpl` (by `getAppOpMode`) and
//! `noteOperationImpl` do. A package the caller may not see or that is not
//! the uid's is not decided by; an op is ignored while a sensor privacy
//! toggle restricts it, a user restriction it has is set and the app may
//! not bypass it, or (for checks of the ops suspension restricts) the
//! package is suspended; root and system have every op once
//! `runtime_permission_appops_mapping_enabled` is on; else the op's switch
//! decides: the uid's mode where it is not the switch's default, else the
//! package's, as the permission service last persisted them
//! (`access.abx`), else the switch's default. A foreground mode allows a
//! uid while it has a process, as each of the app host's processes is top
//! (`AppOpsUidStateTracker.evalMode`). Notes go through SystemServer's
//! `AppOpsPolicy`: without a hotword detection service RECORD_AUDIO_HOTWORD
//! is noted as RECORD_AUDIO; datasource attribution tags: #1561. The modes
//! do not change while the app host runs, so mode watchers are kept and
//! never called, as are active watchers, no op being started; foreground
//! changes are not watched (#1529). An op noted for an app by another uid
//! with `shouldCollectAsyncNotedOp` goes to the app's async-noted watchers
//! (`collectAsyncNotedOp`), or waits for its `extractAsyncOps` when none
//! took it. The app host's own services ask as the system server does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::com_android_internal_app_iappopsservice as appops;

use super::app_op_infos::{APP_OP_INFOS, AppOpInfo, Bypass, DefaultMode, NUM_OP, OP_NONE, UNRESTRICTED};
use super::permission_manager::PermissionManager;
use super::{AppHost, reply};

/// `AppOpsManager.MODE_ALLOWED`, `MODE_IGNORED`, `MODE_ERRORED`,
/// `MODE_DEFAULT`, `MODE_FOREGROUND`.
pub(super) const MODE_ALLOWED: i32 = 0;
pub(super) const MODE_IGNORED: i32 = 1;
pub(super) const MODE_ERRORED: i32 = 2;
pub(super) const MODE_DEFAULT: i32 = 3;
pub(super) const MODE_FOREGROUND: i32 = 4;
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
/// `AppOpsManager.OP_SYSTEM_ALERT_WINDOW`, `OP_ACTIVATE_VPN`,
/// `OP_START_FOREGROUND`, `OP_ACTIVATE_PLATFORM_VPN`,
/// `OP_SYSTEM_EXEMPT_FROM_POWER_RESTRICTIONS`.
pub(super) const OP_SYSTEM_ALERT_WINDOW: i32 = 24;
pub(super) const OP_ACTIVATE_VPN: i32 = 47;
pub(super) const OP_START_FOREGROUND: i32 = 76;
pub(super) const OP_ACTIVATE_PLATFORM_VPN: i32 = 94;
pub(super) const OP_SYSTEM_EXEMPT_FROM_POWER_RESTRICTIONS: i32 = 128;
/// `AppOpsManager.OP_CAMERA`, `OP_RECORD_AUDIO`,
/// `OP_RECORD_AUDIO_HOTWORD`.
const OP_CAMERA: i32 = 26;
const OP_RECORD_AUDIO: i32 = 27;
const OP_RECORD_AUDIO_HOTWORD: i32 = 102;
/// `AppOpsService.OPS_RESTRICTED_ON_SUSPEND`: PLAY_AUDIO, RECORD_AUDIO,
/// CAMERA, VIBRATE.
const OPS_RESTRICTED_ON_SUSPEND: [i32; 4] = [28, OP_RECORD_AUDIO, OP_CAMERA, 3];
/// The ops `AppOpsPolicy.resolveDatasourceOp` resolves by attribution tag:
/// COARSE_LOCATION, FINE_LOCATION, ACTIVITY_RECOGNITION.
const DATASOURCE_OPS: [i32; 3] = [0, 1, 79];
/// `Process.ROOT_UID`, `SYSTEM_UID`, `SHELL_UID`, `MEDIA_UID`,
/// `AUDIOSERVER_UID`, `CAMERASERVER_UID`, `FIRST_SDK_SANDBOX_UID`,
/// `LAST_SDK_SANDBOX_UID`; `UserHandle.PER_USER_RANGE`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const SHELL_UID: i32 = 2000;
const MEDIA_UID: i32 = 1013;
const AUDIOSERVER_UID: i32 = 1041;
const CAMERASERVER_UID: i32 = 1047;
const SDK_SANDBOX_UIDS: std::ops::RangeInclusive<i32> = 20000..=29999;
const PER_USER_RANGE: i32 = 100_000;
/// `Context.DEVICE_ID_DEFAULT`.
const DEVICE_ID_DEFAULT: i32 = 0;
/// `PermissionInfo.PROTECTION_MASK_BASE`, `PROTECTION_DANGEROUS`,
/// `PROTECTION_FLAG_APPOP`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_FLAG_APPOP: i32 = 0x40;
/// `PackageManager.FEATURE_AUTOMOTIVE`, `FEATURE_LEANBACK`.
const FEATURE_AUTOMOTIVE: &str = "android.hardware.type.automotive";
const FEATURE_LEANBACK: &str = "android.software.leanback";
const UPDATE_APP_OPS_STATS: &str = "android.permission.UPDATE_APP_OPS_STATS";
const EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS: &str = "android.permission.EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS";
/// The permissions one of which reads a restricted-read op.
const READ_RESTRICTED: [&str; 3] =
    ["android.permission.MANAGE_APPOPS", "android.permission.GET_APP_OPS_STATS", "android.permission.MANAGE_APP_OPS_MODES"];
const RUNTIME_PERMISSION_APPOPS_MAPPING: &str = "android.permission.flags.runtime_permission_appops_mapping_enabled";
const APPOP_MODE_CACHING: &str = "android.permission.flags.appop_mode_caching_enabled";
const CHECK_OP_VALIDATE_PACKAGE: &str = "android.permission.flags.check_op_validate_package";
const CAMERA_PRIVACY_ALLOWLIST: &str = "com.android.internal.camera.flags.camera_privacy_allowlist";

/// What the decisions read of the system server.
trait System {
    /// An aconfig flag of the image.
    fn flag(&self, name: &str) -> Result<bool, Exception>;
    /// A guest property.
    fn property(&self, name: &str) -> Option<String>;
    /// `PackageManager.hasSystemFeature(name, 0)`.
    fn feature(&self, name: &str) -> Result<bool, Exception>;
    /// Whether user 0's restriction `name` is set, as UserManager gives
    /// its restrictions to appops (`setUserRestrictions`).
    fn user_restriction(&self, name: &str) -> Result<bool, Exception>;
    /// Whether sensor privacy restricts `op` for every app (a global
    /// restriction).
    fn global_restriction(&self, op: i32) -> Result<bool, Exception>;
    /// The package `name` (`getPackageStateInternal`), in `user`.
    fn package(&self, name: &str, user: i32) -> Result<Option<Package>, Exception>;
    /// `PackageManagerInternal.filterAppAccess(package, caller, user)`.
    fn filtered(&self, package: &str, caller: i32, user: i32) -> Result<bool, Exception>;
    /// `checkPermission(permission, -1, uid)`.
    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception>;
    /// The mode persisted for the op named `op` of an app id, and of a
    /// package.
    fn uid_mode(&self, app_id: i32, op: &str) -> Result<Option<i32>, Exception>;
    fn package_mode(&self, package: &str, op: &str) -> Result<Option<i32>, Exception>;
    /// Whether `uid` has a process.
    fn running(&self, uid: i32) -> bool;
    fn unimplemented(&self, what: &str) -> Exception;
}

/// A package as appops reads it: its app id, whether it is parsed
/// (`getAndroidPackage`) and privileged, and suspended in the user.
#[derive(Clone, Copy, Debug)]
struct Package {
    app_id: i32,
    parsed: bool,
    privileged: bool,
    suspended: bool,
}

/// `verifyAndGetBypass`: the package's restriction bypass, or that it is
/// not the uid's (its SecurityException).
enum Verified {
    Bypass(Option<Bypass>),
    NotUids,
}

fn app_id(uid: i32) -> i32 {
    uid.rem_euclid(PER_USER_RANGE)
}

/// `verifyIncomingOp`: a known op, a restricted-read one only for the app
/// ops managers.
fn verify_incoming_op(sys: &dyn System, caller: i32, code: i32) -> Result<&'static AppOpInfo, Exception> {
    let info = usize::try_from(code)
        .ok()
        .and_then(|i| APP_OP_INFOS.get(i))
        .ok_or_else(|| Exception::illegal_argument(format!("Bad operation #{code}")))?;
    if info.restrict_read && !READ_RESTRICTED.iter().try_fold(false, |held, p| Ok::<_, Exception>(held || sys.holds(caller, p)?))? {
        return Err(Exception::security(format!(
            "verifyIncomingOp: uid {caller} does not have any of {{MANAGE_APPOPS, GET_APP_OPS_STATS, MANAGE_APP_OPS_MODES}}"
        )));
    }
    Ok(info)
}

/// `verifyIncomingUid`: the caller's own uid, else UPDATE_APP_OPS_STATS.
fn verify_incoming_uid(sys: &dyn System, caller: i32, uid: i32) -> Result<(), Exception> {
    if uid == caller || sys.holds(caller, UPDATE_APP_OPS_STATS)? {
        return Ok(());
    }
    Err(Exception::security(format!("uid {caller} does not have {UPDATE_APP_OPS_STATS}.")))
}

/// `opToDefaultMode`.
fn default_mode(sys: &dyn System, info: &AppOpInfo) -> Result<i32, Exception> {
    Ok(match info.default_mode {
        DefaultMode::Mode(mode) => mode,
        // `ActivityManager.isLowRamDeviceStatic`.
        DefaultMode::SystemAlertWindow => {
            let low_ram = super::system_boolean(sys.property("ro.config.low_ram").as_deref(), false)
                || (sys.property("ro.debuggable").as_deref() == Some("1")
                    && super::system_boolean(sys.property("debug.force_low_ram").as_deref(), false));
            if low_ram && !sys.feature(FEATURE_LEANBACK)? { MODE_IGNORED } else { MODE_DEFAULT }
        }
    })
}

/// `AppOpsManager.resolvePackageName`.
fn resolve_package_name(uid: i32, package: Option<&str>) -> Option<String> {
    let special = match uid {
        ROOT_UID => "root",
        SHELL_UID => "com.android.shell",
        MEDIA_UID => "media",
        AUDIOSERVER_UID => "audioserver",
        CAMERASERVER_UID => "cameraserver",
        SYSTEM_UID if package.is_none() => "android",
        _ => return package.map(str::to_string),
    };
    Some(special.to_string())
}

/// `resolveNonAppUid`.
fn non_app_uid(package: &str) -> Option<i32> {
    match package {
        "root" => Some(ROOT_UID),
        "shell" | "dumpstate" => Some(SHELL_UID),
        "media" => Some(MEDIA_UID),
        "audioserver" => Some(AUDIOSERVER_UID),
        "cameraserver" => Some(CAMERASERVER_UID),
        _ => None,
    }
}

/// `isIncomingPackageValid`: none, a special one of the caller, or one it
/// may see.
fn valid_package(sys: &dyn System, caller: i32, package: Option<&str>, user: i32) -> Result<bool, Exception> {
    let Some(package) = package else { return Ok(true) };
    if caller == SYSTEM_UID || resolve_package_name(caller, Some(package)).as_deref().and_then(non_app_uid).is_some() {
        return Ok(true);
    }
    Ok(sys.package(package, user)?.is_some() && !sys.filtered(package, caller, user)?)
}

/// `verifyAndGetBypass`: root has no bypass, a non-app uid's own name is
/// unrestricted, else the package must be the uid's.
fn verify_and_get_bypass(sys: &dyn System, uid: i32, package: &str) -> Result<Verified, Exception> {
    if uid == ROOT_UID {
        return Ok(Verified::Bypass(None));
    }
    if SDK_SANDBOX_UIDS.contains(&app_id(uid)) {
        return Err(sys.unimplemented("an SDK sandbox uid's ops (#1562)"));
    }
    let non_app = if package == "com.android.shell" { Some(SHELL_UID) } else { non_app_uid(package) };
    if let Some(non_app) = non_app {
        return Ok(if non_app == app_id(uid) { Verified::Bypass(Some(UNRESTRICTED)) } else { Verified::NotUids });
    }
    let user = uid.div_euclid(PER_USER_RANGE);
    let Some(ps) = sys.package(package, user)?.filter(|p| p.parsed) else { return Ok(Verified::NotUids) };
    if user * PER_USER_RANGE + ps.app_id != uid {
        return Ok(Verified::NotUids);
    }
    // `getBypassforPackage`.
    Ok(Verified::Bypass(Some(Bypass {
        system_uid: ps.app_id == SYSTEM_UID,
        privileged: ps.privileged,
        record_audio_exempt: sys.holds(ps.app_id, EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS)?,
    })))
}

/// `isOpRestrictedLocked` of the default device: a global restriction, or
/// the op's user restriction unless the app may bypass it.
fn restricted(sys: &dyn System, uid: i32, info: &AppOpInfo, bypass: Option<Bypass>) -> Result<bool, Exception> {
    if sys.global_restriction(info.code)? {
        return Ok(true);
    }
    if info.code == OP_CAMERA && sys.feature(FEATURE_AUTOMOTIVE)? && sys.flag(CAMERA_PRIVACY_ALLOWLIST)? {
        return Err(sys.unimplemented("the camera privacy allowlist (#1563)"));
    }
    let Some(restriction) = info.restriction else { return Ok(false) };
    if uid.div_euclid(PER_USER_RANGE) != 0 || !sys.user_restriction(restriction)? {
        return Ok(false);
    }
    // `opAllowSystemBypassRestriction`.
    if let (Some(op), Some(app)) = (info.allow_system_restriction_bypass, bypass)
        && ((op.system_uid && app.system_uid)
            || (op.privileged && app.privileged)
            || (op.record_audio_exempt && app.record_audio_exempt))
    {
        return Ok(false);
    }
    Ok(true)
}

/// `isOpAllowedForUid`: root and system, with the runtime permissions'
/// app ops mapping.
fn allowed_for_uid(sys: &dyn System, uid: i32) -> Result<bool, Exception> {
    Ok(matches!(app_id(uid), ROOT_UID | SYSTEM_UID) && sys.flag(RUNTIME_PERMISSION_APPOPS_MAPPING)?)
}

/// The switch's mode: the uid's where it is not the switch's default,
/// else the package's (`getUidMode`, `getPackageMode`). OP_NONE's entry
/// (the deprecated op 96) has no switch.
fn persisted_mode(sys: &dyn System, uid: i32, package: &str, info: &AppOpInfo) -> Result<i32, Exception> {
    if info.code == OP_NONE {
        return Err(Exception::new(EX_ILLEGAL_STATE, "a deprecated app op has no switch"));
    }
    let switch = &APP_OP_INFOS[info.switch_code as usize];
    let default = default_mode(sys, switch)?;
    let uid_mode = sys.uid_mode(app_id(uid), switch.name)?.unwrap_or(default);
    if uid_mode != default {
        return Ok(uid_mode);
    }
    Ok(sys.package_mode(package, switch.name)?.unwrap_or(default))
}

/// `AppOpsUidStateTracker.evalMode`: a foreground mode allows a uid with a
/// process, all of them top.
fn eval(sys: &dyn System, uid: i32, mode: i32) -> i32 {
    match mode {
        MODE_FOREGROUND if sys.running(uid) => MODE_ALLOWED,
        MODE_FOREGROUND => MODE_IGNORED,
        mode => mode,
    }
}

/// `checkOperationImpl`, by `getAppOpMode`: `raw` keeps a foreground mode.
/// The app host has no virtual devices (`isValidVirtualDeviceId`).
fn check(sys: &dyn System, caller: i32, code: i32, uid: i32, package: Option<&str>, device: i32, raw: bool) -> Result<i32, Exception> {
    let user = uid.div_euclid(PER_USER_RANGE);
    let validate = sys.flag(CHECK_OP_VALIDATE_PACKAGE)?;
    let info = verify_incoming_op(sys, caller, code)?;
    if device != DEVICE_ID_DEFAULT {
        return Ok(MODE_IGNORED);
    }
    if !valid_package(sys, caller, package, user)? {
        return if validate { Ok(MODE_IGNORED) } else { default_mode(sys, info) };
    }
    let Some(package) = resolve_package_name(uid, package) else { return Ok(MODE_IGNORED) };
    if !sys.flag(APPOP_MODE_CACHING)? {
        return Err(sys.unimplemented("checkOperationUnchecked (#1565)"));
    }
    let Verified::Bypass(bypass) = verify_and_get_bypass(sys, uid, &package)? else { return Ok(MODE_IGNORED) };
    // `isOpRestrictedDueToSuspend`.
    if OPS_RESTRICTED_ON_SUSPEND.contains(&code) && sys.package(&package, user)?.is_some_and(|p| p.suspended) {
        return Ok(MODE_IGNORED);
    }
    if restricted(sys, uid, info, bypass)? {
        return Ok(MODE_IGNORED);
    }
    if allowed_for_uid(sys, uid)? {
        return Ok(MODE_ALLOWED);
    }
    let mode = persisted_mode(sys, uid, &package, info)?;
    Ok(if raw { mode } else { eval(sys, uid, mode) })
}

/// `AppOpsPolicy.resolveDatasourceOp`: RECORD_AUDIO_HOTWORD is
/// RECORD_AUDIO where a hotword detection service is required, there being
/// none; a datasource's attribution tags are not known (#1561).
fn resolve_datasource_op(sys: &dyn System, code: i32, tag: Option<&str>) -> Result<i32, Exception> {
    // `isHotwordDetectionServiceRequired`.
    let hotword_required = || -> Result<bool, Exception> {
        Ok(!sys.feature(FEATURE_AUTOMOTIVE)?
            && !sys.feature(FEATURE_LEANBACK)?
            && super::system_boolean(sys.property("ro.hotword.detection_service_required").as_deref(), false))
    };
    let code = if code == OP_RECORD_AUDIO_HOTWORD && hotword_required()? { OP_RECORD_AUDIO } else { code };
    if tag.is_some() && DATASOURCE_OPS.contains(&code) {
        return Err(sys.unimplemented("a datasource's attribution tags (#1561)"));
    }
    Ok(code)
}

/// `noteOperationImpl` behind `AppOpsPolicy.noteOperation`: a package that
/// is not the uid's errs.
fn note_op(
    sys: &dyn System,
    caller: i32,
    code: i32,
    uid: i32,
    package: Option<&str>,
    tag: Option<&str>,
    device: i32,
) -> Result<i32, Exception> {
    let code = resolve_datasource_op(sys, code, tag)?;
    let user = uid.div_euclid(PER_USER_RANGE);
    let validate = sys.flag(CHECK_OP_VALIDATE_PACKAGE)?;
    // `validateOpRequest` verifies the op first.
    let info = if validate {
        let info = verify_incoming_op(sys, caller, code)?;
        verify_incoming_uid(sys, caller, uid)?;
        info
    } else {
        verify_incoming_uid(sys, caller, uid)?;
        verify_incoming_op(sys, caller, code)?
    };
    if device != DEVICE_ID_DEFAULT {
        return Ok(MODE_IGNORED);
    }
    if !valid_package(sys, caller, package, user)? {
        return Ok(if validate { MODE_IGNORED } else { MODE_ERRORED });
    }
    let Some(package) = resolve_package_name(uid, package) else { return Ok(MODE_IGNORED) };
    // `noteOperationUnchecked`.
    let Verified::Bypass(bypass) = verify_and_get_bypass(sys, uid, &package)? else { return Ok(MODE_ERRORED) };
    if restricted(sys, uid, info, bypass)? {
        return Ok(MODE_IGNORED);
    }
    if allowed_for_uid(sys, uid)? {
        return Ok(MODE_ALLOWED);
    }
    Ok(eval(sys, uid, persisted_mode(sys, uid, &package, info)?))
}

/// `shouldCollectNotes`: an op whose notes are always collected, or whose
/// permission (by `protection`, `getPermissionInfo`) is dangerous or an
/// app op one.
fn should_collect_notes(
    sys: &dyn System,
    protection: impl FnOnce(&str) -> Result<Option<i32>, Exception>,
    code: i32,
) -> Result<bool, Exception> {
    // `Preconditions.checkArgumentInRange`.
    let side = if code < 0 { "too low" } else { "too high" };
    let info = usize::try_from(code).ok().and_then(|i| APP_OP_INFOS.get(i)).ok_or_else(|| {
        Exception::illegal_argument(format!("opCode is out of range of [0, {}] ({side})", NUM_OP - 1))
    })?;
    if info.force_collect_notes {
        return Ok(true);
    }
    let permission = match info.permission {
        Some((permission, None)) => permission,
        Some((permission, Some(flag))) if sys.flag(flag)? => permission,
        _ => return Ok(false),
    };
    Ok(protection(permission)?
        .is_some_and(|level| level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS || level & PROTECTION_FLAG_APPOP != 0))
}

/// The app host as the system server appops reads.
struct Host<'a>(&'a AppHost);

impl System for Host<'_> {
    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.0.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    fn property(&self, name: &str) -> Option<String> {
        self.0.property(name)
    }

    fn feature(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.0.packages()?;
        Ok(state.system.features.iter().any(|(n, version)| n == name && *version >= 0))
    }

    fn user_restriction(&self, name: &str) -> Result<bool, Exception> {
        Ok(self.0.user_restrictions()?.contains(name))
    }

    fn global_restriction(&self, op: i32) -> Result<bool, Exception> {
        super::sensor_privacy::restricts(self.0, op)
    }

    fn package(&self, name: &str, user: i32) -> Result<Option<Package>, Exception> {
        let (state, _) = self.0.packages()?;
        Ok(state.packages.get(name).map(|ps| Package {
            app_id: ps.app_id,
            parsed: ps.pkg.is_some(),
            privileged: ps.is.privileged,
            suspended: ps.users.get(&user).is_some_and(|u| !u.suspended_by.is_empty()),
        }))
    }

    fn filtered(&self, package: &str, caller: i32, user: i32) -> Result<bool, Exception> {
        let (state, resolution) = self.0.packages()?;
        let ps = state.packages.get(package);
        crate::package::apps_filter::should_filter_application(&state, &resolution.apps_filter, ps, caller, user, true, true)
            .map_err(|e| Exception::new(EX_UNSUPPORTED_OPERATION, format!("appops filterAppAccess: {}", e.0)))
    }

    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.0.check_permission(Some(permission), uid)? == 0)
    }

    fn uid_mode(&self, app_id: i32, op: &str) -> Result<Option<i32>, Exception> {
        let access = self.0.inputs()?.packages.access();
        let ops = access.and_then(|a| a.app_id_app_ops.iter().find(|(id, _)| *id == app_id));
        Ok(ops.and_then(|(_, ops)| ops.iter().find(|(n, _)| n == op)).map(|(_, mode)| *mode))
    }

    fn package_mode(&self, package: &str, op: &str) -> Result<Option<i32>, Exception> {
        let access = self.0.inputs()?.packages.access();
        let ops = access.and_then(|a| a.package_app_ops.iter().find(|(name, _)| name == package));
        Ok(ops.and_then(|(_, ops)| ops.iter().find(|(n, _)| n == op)).map(|(_, mode)| *mode))
    }

    fn running(&self, uid: i32) -> bool {
        self.0.uid_running(uid)
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.0.unimplemented_named("appops", what)
    }
}

/// `checkOperation` as the system server asks it.
pub(super) fn mode(host: &AppHost, code: i32, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
    check(&Host(host), SYSTEM_UID, code, uid, package, DEVICE_ID_DEFAULT, false)
}

/// `noteOperation` as the system server asks it.
pub(super) fn note(host: &AppHost, code: i32, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
    note_op(&Host(host), SYSTEM_UID, code, uid, package, None, DEVICE_ID_DEFAULT)
}

/// `AppOpsManager.NotedOp`, a batched note.
struct NotedOp {
    op: i32,
    uid: i32,
    package: Option<String>,
    attribution_tag: Option<String>,
    virtual_device_id: i32,
    message: Option<String>,
    should_collect_async_noted_op: bool,
}

/// A batch's key: a `NotedOp` as `writeValue` writes a parcelable (its
/// class, then `writeToParcel`'s fields).
fn noted_op(key: &aim_service_aidl::ParcelValue) -> Option<NotedOp> {
    let aim_service_aidl::ParcelValue::Custom { kind: 4, bytes } = key else { return None };
    let mut r = aim_binder_host::parcel::Reader::new(bytes, &[]);
    (r.read_string16().ok()?.as_deref() == Some("android.app.AppOpsManager$NotedOp")).then_some(())?;
    Some(NotedOp {
        op: r.read_i32().ok()?,
        uid: r.read_i32().ok()?,
        package: r.read_string16().ok()?,
        attribution_tag: r.read_string16().ok()?,
        virtual_device_id: r.read_i32().ok()?,
        message: r.read_string16().ok()?,
        should_collect_async_noted_op: r.read_bool().ok()?,
    })
}

/// `checkPackage`: whether `package` is `uid`'s.
fn check_package(host: &AppHost, uid: i32, package: Option<&str>) -> Result<i32, Exception> {
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
    permissions: Arc<PermissionManager>,
    watchers: Mutex<Vec<(i32, Binder)>>,
    active_watchers: Arc<Mutex<ActiveWatchers>>,
    async_noted: Arc<Mutex<Async>>,
}

impl AppOps {
    pub fn new(host: Arc<AppHost>, permissions: Arc<PermissionManager>) -> Self {
        AppOps { host, permissions, watchers: Mutex::new(Vec::new()), active_watchers: Default::default(), async_noted: Default::default() }
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
        match check_package(&self.host, uid, Some(package))? {
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
                match check_package(&self.host, args.uid, args.package_name.as_deref()) {
                    Ok(mode) => appops::write_check_package_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::CHECK_OPERATION => {
                let args = appops::CheckOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let caller = call.sender_euid as i32;
                match check(&Host(&self.host), caller, args.code, args.uid, args.package_name.as_deref(), DEVICE_ID_DEFAULT, false) {
                    Ok(mode) => appops::write_check_operation_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            // `checkAudioOperationImpl`: no audio restriction is set in the
            // app host (`setAudioRestriction`), so the op's check decides.
            appops::CHECK_AUDIO_OPERATION => {
                let args = appops::CheckAudioOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let caller = call.sender_euid as i32;
                match check(&Host(&self.host), caller, args.code, args.uid, args.package_name.as_deref(), DEVICE_ID_DEFAULT, false) {
                    Ok(mode) => appops::write_check_audio_operation_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::CHECK_OPERATION_FOR_DEVICE => {
                let args = appops::CheckOperationForDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let caller = call.sender_euid as i32;
                let package = args.package_name.as_deref();
                match check(&Host(&self.host), caller, args.code, args.uid, package, args.virtual_device_id, false) {
                    Ok(mode) => appops::write_check_operation_for_device_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::CHECK_OPERATION_RAW_FOR_DEVICE => {
                let args = appops::CheckOperationRawForDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let caller = call.sender_euid as i32;
                let package = args.package_name.as_deref();
                match check(&Host(&self.host), caller, args.code, args.uid, package, args.virtual_device_id, true) {
                    Ok(mode) => appops::write_check_operation_raw_for_device_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::NOTE_OPERATION => {
                let args = appops::NoteOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let caller = call.sender_euid as i32;
                let (package, tag) = (args.package_name.as_deref(), args.attribution_tag.as_deref());
                let mode = match note_op(&Host(&self.host), caller, args.code, args.uid, package, tag, DEVICE_ID_DEFAULT) {
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
            // One-way: each batched note as `noteOperation` notes it.
            appops::NOTE_OPERATIONS_IN_BATCH => {
                let args = appops::NoteOperationsInBatch::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                for (key, _count) in args.batched_note_ops.unwrap_or_default() {
                    let op = noted_op(&key).ok_or(BAD_VALUE)?;
                    let (package, tag) = (op.package.as_deref(), op.attribution_tag.as_deref());
                    match note_op(&Host(&self.host), call.sender_euid as i32, op.op, op.uid, package, tag, op.virtual_device_id) {
                        Ok(MODE_ALLOWED) if op.should_collect_async_noted_op => {
                            if let Some(package) = op.package.clone() {
                                self.collect(op.uid, package, op.op, op.attribution_tag.clone(), op.message.unwrap_or_default(), call.sender_euid as i32);
                            }
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("app host: appops noteOperationsInBatch: {e:?}"),
                    }
                }
                return Ok(Parcel::new());
            }
            appops::SHOULD_COLLECT_NOTES => {
                let args = appops::ShouldCollectNotes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let protection = |permission: &str| self.permissions.protection_level(permission);
                match should_collect_notes(&Host(&self.host), protection, args.op_code) {
                    Ok(collect) => appops::write_should_collect_notes_reply(&mut p, collect),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::START_WATCHING_MODE => {
                let args = appops::StartWatchingMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(callback) = args.callback {
                    self.watchers.lock().unwrap().push((args.op, callback));
                }
                appops::write_start_watching_mode_reply(&mut p);
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    const APP: i32 = 10_100;
    const PRIV_APP: i32 = 10_101;
    const OP_COARSE_LOCATION: i32 = 0;
    const OP_GPS: i32 = 2;
    const OP_VIBRATE: i32 = 3;
    const OP_WIFI_SCAN: i32 = 10;
    const OP_READ_CLIPBOARD: i32 = 29;
    const OP_LEGACY_STORAGE: i32 = 87;
    const OP_ACCESS_RESTRICTED_SETTINGS: i32 = 119;

    #[derive(Default)]
    struct Fake {
        flags: BTreeSet<&'static str>,
        restrictions: BTreeSet<&'static str>,
        global: BTreeSet<i32>,
        packages: BTreeMap<&'static str, Package>,
        granted: BTreeSet<(i32, &'static str)>,
        uid_modes: BTreeMap<(i32, &'static str), i32>,
        package_modes: BTreeMap<(&'static str, &'static str), i32>,
        running: BTreeSet<i32>,
    }

    impl System for Fake {
        fn flag(&self, name: &str) -> Result<bool, Exception> {
            Ok(self.flags.contains(name))
        }

        fn property(&self, _: &str) -> Option<String> {
            None
        }

        fn feature(&self, _: &str) -> Result<bool, Exception> {
            Ok(false)
        }

        fn user_restriction(&self, name: &str) -> Result<bool, Exception> {
            Ok(self.restrictions.contains(name))
        }

        fn global_restriction(&self, op: i32) -> Result<bool, Exception> {
            Ok(self.global.contains(&op))
        }

        fn package(&self, name: &str, _: i32) -> Result<Option<Package>, Exception> {
            Ok(self.packages.get(name).copied())
        }

        fn filtered(&self, package: &str, _: i32, _: i32) -> Result<bool, Exception> {
            Ok(package == "hidden")
        }

        fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
            Ok(matches!(app_id(uid), ROOT_UID | SYSTEM_UID) || self.granted.iter().any(|(u, p)| *u == uid && *p == permission))
        }

        fn uid_mode(&self, app_id: i32, op: &str) -> Result<Option<i32>, Exception> {
            Ok(self.uid_modes.iter().find(|((a, o), _)| *a == app_id && *o == op).map(|(_, m)| *m))
        }

        fn package_mode(&self, package: &str, op: &str) -> Result<Option<i32>, Exception> {
            Ok(self.package_modes.iter().find(|((p, o), _)| *p == package && *o == op).map(|(_, m)| *m))
        }

        fn running(&self, uid: i32) -> bool {
            self.running.contains(&uid)
        }

        fn unimplemented(&self, what: &str) -> Exception {
            Exception::new(EX_UNSUPPORTED_OPERATION, what)
        }
    }

    fn package(app_id: i32, privileged: bool) -> Package {
        Package { app_id, parsed: true, privileged, suspended: false }
    }

    fn fake() -> Fake {
        let mut f = Fake { flags: [APPOP_MODE_CACHING].into(), ..Fake::default() };
        f.packages.insert("app", package(APP, false));
        f.packages.insert("priv", package(PRIV_APP, true));
        f.packages.insert("android", package(SYSTEM_UID, true));
        f.packages.insert("hidden", package(10_102, false));
        f
    }

    fn check_mode(f: &Fake, code: i32, uid: i32, package: &str) -> i32 {
        check(f, uid, code, uid, Some(package), DEVICE_ID_DEFAULT, false).unwrap()
    }

    fn note_mode(f: &Fake, caller: i32, code: i32, uid: i32, package: &str) -> i32 {
        note_op(f, caller, code, uid, Some(package), None, DEVICE_ID_DEFAULT).unwrap()
    }

    #[test]
    fn modes_are_the_switchs() {
        let mut f = fake();
        assert_eq!(check_mode(&f, OP_GPS, APP, "app"), MODE_ALLOWED);
        // GPS and WIFI_SCAN switch to COARSE_LOCATION: its mode decides.
        f.uid_modes.insert((APP, "android:coarse_location"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_GPS, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_WIFI_SCAN, APP, "app"), MODE_IGNORED);
        f.uid_modes.insert((APP, "android:gps"), MODE_ERRORED);
        assert_eq!(check_mode(&f, OP_GPS, APP, "app"), MODE_IGNORED);
        assert_eq!(note_mode(&f, APP, OP_GPS, APP, "app"), MODE_IGNORED);
    }

    #[test]
    fn a_uid_mode_at_its_default_leaves_the_packages() {
        let mut f = fake();
        assert_eq!(check_mode(&f, OP_LEGACY_STORAGE, APP, "app"), MODE_DEFAULT);
        f.uid_modes.insert((APP, "android:legacy_storage"), MODE_DEFAULT);
        f.package_modes.insert(("app", "android:legacy_storage"), MODE_ALLOWED);
        assert_eq!(check_mode(&f, OP_LEGACY_STORAGE, APP, "app"), MODE_ALLOWED);
        f.uid_modes.insert((APP, "android:legacy_storage"), MODE_ERRORED);
        assert_eq!(check_mode(&f, OP_LEGACY_STORAGE, APP, "app"), MODE_ERRORED);
    }

    #[test]
    fn foreground_modes_by_the_uids_process() {
        let mut f = fake();
        assert_eq!(check_mode(&f, OP_TAKE_AUDIO_FOCUS, APP, "app"), MODE_IGNORED);
        assert_eq!(check(&f, APP, OP_TAKE_AUDIO_FOCUS, APP, Some("app"), DEVICE_ID_DEFAULT, true).unwrap(), MODE_FOREGROUND);
        f.running.insert(APP);
        assert_eq!(check_mode(&f, OP_TAKE_AUDIO_FOCUS, APP, "app"), MODE_ALLOWED);
    }

    #[test]
    fn user_restrictions_and_their_bypass() {
        let mut f = fake();
        f.restrictions.insert("no_share_location");
        // COARSE_LOCATION lets the system uid bypass it, WIFI_SCAN a
        // privileged app; GPS lets none.
        assert_eq!(check_mode(&f, OP_COARSE_LOCATION, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_COARSE_LOCATION, PRIV_APP, "priv"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_COARSE_LOCATION, SYSTEM_UID, "android"), MODE_ALLOWED);
        assert_eq!(check_mode(&f, OP_WIFI_SCAN, PRIV_APP, "priv"), MODE_ALLOWED);
        assert_eq!(check_mode(&f, OP_WIFI_SCAN, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_GPS, SYSTEM_UID, "android"), MODE_IGNORED);
        // A non-app uid's own name is unrestricted; root bypasses nothing.
        assert_eq!(check_mode(&f, OP_WIFI_SCAN, MEDIA_UID, "media"), MODE_ALLOWED);
        assert_eq!(check(&f, ROOT_UID, OP_WIFI_SCAN, ROOT_UID, None, DEVICE_ID_DEFAULT, false).unwrap(), MODE_IGNORED);
        assert_eq!(note_mode(&f, APP, OP_WIFI_SCAN, APP, "app"), MODE_IGNORED);
        // RECORD_AUDIO by EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS.
        f.restrictions.insert("no_record_audio");
        assert_eq!(check_mode(&f, OP_RECORD_AUDIO, APP, "app"), MODE_IGNORED);
        f.granted.insert((APP, EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS));
        assert_eq!(check_mode(&f, OP_RECORD_AUDIO, APP, "app"), MODE_ALLOWED);
    }

    #[test]
    fn global_restrictions_and_suspension() {
        let mut f = fake();
        f.global.insert(OP_CAMERA);
        assert_eq!(check_mode(&f, OP_CAMERA, SYSTEM_UID, "android"), MODE_IGNORED);
        f.global.clear();
        f.packages.get_mut("app").unwrap().suspended = true;
        assert_eq!(check_mode(&f, OP_CAMERA, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_VIBRATE, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_READ_CLIPBOARD, APP, "app"), MODE_ALLOWED);
        // Notes are not suspended.
        assert_eq!(note_mode(&f, APP, OP_CAMERA, APP, "app"), MODE_ALLOWED);
    }

    #[test]
    fn root_and_system_with_the_runtime_permission_mapping() {
        let mut f = fake();
        f.package_modes.insert(("android", "android:read_clipboard"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_READ_CLIPBOARD, SYSTEM_UID, "android"), MODE_IGNORED);
        f.flags.insert(RUNTIME_PERMISSION_APPOPS_MAPPING);
        assert_eq!(check_mode(&f, OP_READ_CLIPBOARD, SYSTEM_UID, "android"), MODE_ALLOWED);
        assert_eq!(check(&f, SYSTEM_UID, OP_READ_CLIPBOARD, SYSTEM_UID, None, DEVICE_ID_DEFAULT, false).unwrap(), MODE_ALLOWED);
    }

    #[test]
    fn packages_not_the_uids_or_not_visible() {
        let mut f = fake();
        // Without check_op_validate_package: a check of a package the
        // caller may not see answers the op's default, a note errs.
        assert_eq!(check(&f, APP, OP_LEGACY_STORAGE, APP, Some("hidden"), DEVICE_ID_DEFAULT, false).unwrap(), MODE_DEFAULT);
        assert_eq!(note_mode(&f, APP, OP_READ_CLIPBOARD, APP, "hidden"), MODE_ERRORED);
        // Another uid's package.
        assert_eq!(check(&f, SYSTEM_UID, OP_READ_CLIPBOARD, APP, Some("priv"), DEVICE_ID_DEFAULT, false).unwrap(), MODE_IGNORED);
        assert_eq!(note_mode(&f, SYSTEM_UID, OP_READ_CLIPBOARD, APP, "priv"), MODE_ERRORED);
        // An app uid without a package.
        assert_eq!(check(&f, APP, OP_READ_CLIPBOARD, APP, None, DEVICE_ID_DEFAULT, false).unwrap(), MODE_IGNORED);
        f.flags.insert(CHECK_OP_VALIDATE_PACKAGE);
        assert_eq!(check(&f, APP, OP_LEGACY_STORAGE, APP, Some("hidden"), DEVICE_ID_DEFAULT, false).unwrap(), MODE_IGNORED);
        assert_eq!(note_mode(&f, APP, OP_READ_CLIPBOARD, APP, "hidden"), MODE_IGNORED);
        // A virtual device is not known.
        assert_eq!(check(&f, APP, OP_READ_CLIPBOARD, APP, Some("app"), 7, false).unwrap(), MODE_IGNORED);
    }

    #[test]
    fn ops_the_caller_may_not_ask() {
        let f = fake();
        let bad = check(&f, APP, NUM_OP, APP, Some("app"), DEVICE_ID_DEFAULT, false).unwrap_err();
        assert_eq!(bad.message, "Bad operation #163");
        assert!(check(&f, APP, OP_ACCESS_RESTRICTED_SETTINGS, APP, Some("app"), DEVICE_ID_DEFAULT, false).is_err());
        assert_eq!(check_mode(&f, OP_ACCESS_RESTRICTED_SETTINGS, SYSTEM_UID, "android"), MODE_DEFAULT);
        // Noting for another uid needs UPDATE_APP_OPS_STATS.
        assert!(note_op(&f, PRIV_APP, OP_READ_CLIPBOARD, APP, Some("app"), None, DEVICE_ID_DEFAULT).is_err());
        assert_eq!(note_mode(&f, SYSTEM_UID, OP_READ_CLIPBOARD, APP, "app"), MODE_ALLOWED);
        // The deprecated op 96 has no switch.
        assert!(check(&f, APP, 96, APP, Some("app"), DEVICE_ID_DEFAULT, false).is_err());
    }

    #[test]
    fn datasource_ops_of_notes() {
        let f = fake();
        assert_eq!(resolve_datasource_op(&f, OP_RECORD_AUDIO_HOTWORD, None).unwrap(), OP_RECORD_AUDIO_HOTWORD);
        assert_eq!(resolve_datasource_op(&f, OP_COARSE_LOCATION, None).unwrap(), OP_COARSE_LOCATION);
        assert!(resolve_datasource_op(&f, OP_COARSE_LOCATION, Some("tag")).is_err());
        assert_eq!(resolve_datasource_op(&f, OP_READ_CLIPBOARD, Some("tag")).unwrap(), OP_READ_CLIPBOARD);
    }

    #[test]
    fn notes_are_collected_for_dangerous_and_app_op_permissions() {
        let mut f = fake();
        let protection = |permission: &str| {
            Ok(match permission {
                "android.permission.READ_CONTACTS" | "android.permission.health.READ_HEART_RATE" => Some(PROTECTION_DANGEROUS),
                "android.permission.WRITE_SETTINGS" => Some(2 | PROTECTION_FLAG_APPOP),
                "android.permission.VIBRATE" => Some(0),
                _ => None,
            })
        };
        let collect = |f: &Fake, op| should_collect_notes(f, protection, op).unwrap();
        // RECEIVE_AMBIENT_TRIGGER_AUDIO always.
        assert!(collect(&f, 120));
        assert!(collect(&f, OP_READ_CONTACTS));
        assert!(collect(&f, OP_WRITE_SETTINGS));
        assert!(!collect(&f, OP_VIBRATE));
        assert!(!collect(&f, OP_READ_CLIPBOARD));
        // A permission not defined (NameNotFoundException).
        assert!(!collect(&f, OP_WRITE_CONTACTS));
        // READ_HEART_RATE's permission only with its flag.
        assert!(!collect(&f, 149));
        f.flags.insert("android.permission.flags.replace_body_sensor_permission_enabled");
        assert!(collect(&f, 149));
        let out = should_collect_notes(&f, protection, NUM_OP).unwrap_err();
        assert_eq!(out.message, "opCode is out of range of [0, 162] (too high)");
    }
}
