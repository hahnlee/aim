//! `appops`: AppOps for the app host (`AppOpsService` at
//! `android-16.0.0_r1`), deciding every op of `sAppOpInfos`
//! (`app_op_infos`) as `checkOperationImpl` (by `getAppOpMode`),
//! `noteOperationImpl` and `startOperationImpl` do. A package the caller
//! may not see or that is not the uid's is not decided by, and only the
//! installed packages' uids and AppOpsService's non-package uids have ops;
//! an attribution tag the package (or its proxy) does not declare is
//! dropped where SECURITY_EXCEPTION_ON_INVALID_ATTRIBUTION_TAG_CHANGE is
//! enabled. An op is ignored while a sensor privacy toggle restricts it, a
//! user restriction of it is set (UserManager's, or a client's from
//! `setUserRestriction` past its excluded packages and tags, until the
//! client's token dies: `appops_restrictions.rs`) and the app may not
//! bypass it, or (for
//! checks of the ops suspension restricts) the package is suspended; root
//! and system have every op once `runtime_permission_appops_mapping_enabled`
//! is on; else the op's switch decides: the uid's mode where it is not the
//! switch's default, else the package's, as the permission service last
//! persisted them (`access.abx`), else the switch's default. A foreground
//! mode allows a uid while it has a process, as each of the app host's
//! processes is top (`AppOpsUidStateTracker.evalMode`). Notes and starts go
//! through SystemServer's `AppOpsPolicy`: without a hotword detection
//! service RECORD_AUDIO_HOTWORD is RECORD_AUDIO, and activity recognition
//! with a system activity recognizer's attribution tag is its source op
//! (the location providers report no tags in the app host, #1748). A
//! started op (`startOperation`, `startProxyOperation`)
//! runs for its client binder until the client finishes each of its
//! starts or dies (`AttributedOp`), paused instead while restricted; active
//! watchers hear an op's first start and last finish, started watchers
//! each start. A change of a client's user restriction tells the op's mode
//! watchers (`startWatchingModeWithFlags`, kept until they die) and pauses
//! or resumes its started ops; the modes and the other restrictions do not
//! change while the app host runs (#1594); foreground changes are not watched
//! (#1529). Each attributed op keeps its last access and rejection by uid
//! state and flags, a finished start's with its duration
//! (`appops_history.rs`), which `getPackagesForOps(ForDevice)`,
//! `getOpsForPackage`, `getUidOps` and `isProxying` read; noted watchers
//! (#1593) and the historical registry (#2025) are not kept. An op noted or started for
//! an app by another uid with `shouldCollectAsyncNotedOp`, or by a proxy
//! without UPDATE_APP_OPS_STATS for itself, goes to the app's async-noted
//! watchers (`collectAsyncNotedOp`), or waits for its `extractAsyncOps`
//! when none took it. The app host's own services ask as the system server
//! does; an attribution source with a token is not known to be registered
//! (#1527). Apps' runtime access messages are sampled as the original
//! samples them (`appops_sampling.rs`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::com_android_internal_app_iappopsactivecallback as active_callback;
use aim_service_aidl::com_android_internal_app_iappopscallback as mode_callback;
use aim_service_aidl::com_android_internal_app_iappopsservice as appops;
use aim_service_aidl::com_android_internal_app_iappopsstartedcallback as started_callback;

use super::app_op_infos::{
    APP_OP_INFOS, AppOpInfo, Bypass, DefaultMode, NUM_OP, OP_NONE, UNRESTRICTED,
};
use super::appops_history::{
    self as history, AttributedEntry, Events, NoteEvent, OpEntry, PackageOps, ProxyInfo,
};
use super::appops_restrictions::{ModeWatchers, PackageTags, UserRestrictions};
use super::permission_checker::AttributionSourceState;
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
/// `OP_RECORD_AUDIO_HOTWORD`, `OP_RECEIVE_AMBIENT_TRIGGER_AUDIO`,
/// `OP_CAMERA_SANDBOXED`, `OP_RECORD_AUDIO_SANDBOXED`.
const OP_CAMERA: i32 = 26;
const OP_RECORD_AUDIO: i32 = 27;
const OP_RECORD_AUDIO_HOTWORD: i32 = 102;
const OP_RECEIVE_AMBIENT_TRIGGER_AUDIO: i32 = 120;
const OP_CAMERA_SANDBOXED: i32 = 134;
const OP_RECORD_AUDIO_SANDBOXED: i32 = 135;
/// `AppOpsService.OPS_RESTRICTED_ON_SUSPEND`: PLAY_AUDIO, RECORD_AUDIO,
/// CAMERA, VIBRATE.
const OPS_RESTRICTED_ON_SUSPEND: [i32; 4] = [28, OP_RECORD_AUDIO, OP_CAMERA, 3];
/// The ops `AppOpsPolicy.resolveDatasourceOp` resolves by attribution tag:
/// COARSE_LOCATION, FINE_LOCATION, ACTIVITY_RECOGNITION.
/// `OP_COARSE_LOCATION`, `OP_FINE_LOCATION`, `OP_ACTIVITY_RECOGNITION`
/// and their `*_SOURCE` ops (`AppOpsPolicy.resolveLocationOp`,
/// `resolveArOp`).
const DATASOURCE_OPS: [(i32, i32); 3] = [(0, 109), (1, 108), (79, 113)];
const OP_ACTIVITY_RECOGNITION: i32 = 79;
/// `AppOpsManager.OP_FLAG_SELF`, `OP_FLAG_TRUSTED_PROXY`,
/// `OP_FLAG_UNTRUSTED_PROXY`, `OP_FLAG_TRUSTED_PROXIED`,
/// `OP_FLAG_UNTRUSTED_PROXIED`.
const OP_FLAG_SELF: i32 = 0x1;
const OP_FLAG_TRUSTED_PROXY: i32 = 0x2;
const OP_FLAG_UNTRUSTED_PROXY: i32 = 0x4;
const OP_FLAG_TRUSTED_PROXIED: i32 = 0x8;
const OP_FLAG_UNTRUSTED_PROXIED: i32 = 0x10;
/// `AppOpsManager.ATTRIBUTION_FLAG_TRUSTED`, `ATTRIBUTION_CHAIN_ID_NONE`.
pub(super) const ATTRIBUTION_FLAG_TRUSTED: i32 = 0x8;
pub(super) const ATTRIBUTION_CHAIN_ID_NONE: i32 = -1;
/// `OnOpStartedListener.START_TYPE_FAILED`, `START_TYPE_STARTED`.
const START_TYPE_FAILED: i32 = 0;
const START_TYPE_STARTED: i32 = 1;
/// `OnOpStartedListener.START_TYPE_RESUMED`.
const START_TYPE_RESUMED: i32 = 2;
const MANAGE_APP_OPS_MODES: &str = "android.permission.MANAGE_APP_OPS_MODES";
/// `AppOpsManager.OP_VIBRATE`, `OP_PLAY_AUDIO`.
const OP_VIBRATE: i32 = 3;
const OP_PLAY_AUDIO: i32 = 28;
/// `CameraDevice.AUDIO_RESTRICTION_NONE`, `AUDIO_RESTRICTION_VIBRATION_SOUND`.
const AUDIO_RESTRICTION_NONE: i32 = 0;
const AUDIO_RESTRICTION_VIBRATION_SOUND: i32 = 3;

/// `AudioRestrictionManager.checkAudioOperation`'s camera part: under a
/// camera restriction, vibration (and sound, with
/// AUDIO_RESTRICTION_VIBRATION_SOUND) of the SDK usages that
/// `AudioAttributes.SUPPRESSIBLE_USAGES` marks notification, call or alarm
/// (`CAMERA_AUDIO_RESTRICTIONS`) is ignored.
fn camera_audio_muted(restriction: i32, code: i32, usage: i32) -> bool {
    // USAGE_ALARM, USAGE_NOTIFICATION, _RINGTONE,
    // _COMMUNICATION_REQUEST, _INSTANT, _DELAYED, _EVENT.
    const MUTED_USAGES: [i32; 7] = [4, 5, 6, 7, 8, 9, 10];
    restriction != AUDIO_RESTRICTION_NONE
        && (code == OP_VIBRATE
            || code == OP_PLAY_AUDIO && restriction == AUDIO_RESTRICTION_VIBRATION_SOUND)
        && MUTED_USAGES.contains(&usage)
}

/// `AppOpsService.UID_ANY`, `UserHandle.USER_ALL`.
const UID_ANY: i32 = -2;
const USER_ALL: i32 = -1;
/// `VirtualDeviceManager.PERSISTENT_DEVICE_ID_DEFAULT`.
const PERSISTENT_DEVICE_ID_DEFAULT: &str = "default:0";
const MANAGE_APP_OPS_RESTRICTIONS: &str = "android.permission.MANAGE_APP_OPS_RESTRICTIONS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
/// `Process.ROOT_UID`, `SYSTEM_UID`, `SHELL_UID`, `MEDIA_UID`,
/// `AUDIOSERVER_UID`, `CAMERASERVER_UID`, `INVALID_UID`,
/// `FIRST_APPLICATION_UID`, `FIRST_SDK_SANDBOX_UID`,
/// `LAST_SDK_SANDBOX_UID`; `UserHandle.PER_USER_RANGE`.
const ROOT_UID: i32 = 0;
pub(super) const SYSTEM_UID: i32 = 1000;
const SHELL_UID: i32 = 2000;
const MEDIA_UID: i32 = 1013;
const AUDIOSERVER_UID: i32 = 1041;
const CAMERASERVER_UID: i32 = 1047;
const INVALID_UID: i32 = -1;
const FIRST_APPLICATION_UID: i32 = 10_000;
const SDK_SANDBOX_UIDS: std::ops::RangeInclusive<i32> = 20000..=29999;
const PER_USER_RANGE: i32 = 100_000;
/// `AppOpsService.NON_PACKAGE_UIDS`: root, phone, bluetooth, audioserver,
/// NFC, the network stack and shell.
const NON_PACKAGE_UIDS: [i32; 7] = [ROOT_UID, 1001, 1002, AUDIOSERVER_UID, 1027, 1073, SHELL_UID];
/// `Context.DEVICE_ID_DEFAULT`.
const DEVICE_ID_DEFAULT: i32 = 0;
/// `PermissionInfo.PROTECTION_MASK_BASE`, `PROTECTION_DANGEROUS`,
/// `PROTECTION_FLAG_APPOP`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_FLAG_APPOP: i32 = 0x40;
/// `AppOpsManager.SECURITY_EXCEPTION_ON_INVALID_ATTRIBUTION_TAG_CHANGE`.
const SECURITY_EXCEPTION_ON_INVALID_ATTRIBUTION_TAG_CHANGE: i64 = 151_105_954;
/// `PackageManager.FEATURE_AUTOMOTIVE`, `FEATURE_LEANBACK`.
const FEATURE_AUTOMOTIVE: &str = "android.hardware.type.automotive";
const FEATURE_LEANBACK: &str = "android.software.leanback";
const UPDATE_APP_OPS_STATS: &str = "android.permission.UPDATE_APP_OPS_STATS";
const WATCH_APPOPS: &str = "android.permission.WATCH_APPOPS";
const MANAGE_APPOPS: &str = "android.permission.MANAGE_APPOPS";
const EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS: &str =
    "android.permission.EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS";
/// The permissions one of which reads a restricted-read op.
const READ_RESTRICTED: [&str; 3] = [
    MANAGE_APPOPS,
    "android.permission.GET_APP_OPS_STATS",
    "android.permission.MANAGE_APP_OPS_MODES",
];
const RUNTIME_PERMISSION_APPOPS_MAPPING: &str =
    "android.permission.flags.runtime_permission_appops_mapping_enabled";
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
    /// Whether another client's user restriction (`setUserRestriction`)
    /// restricts `code` for the package and tag of `user`.
    fn client_restricted(
        &self,
        code: i32,
        user: i32,
        package: &str,
        tag: Option<&str>,
        is_check: bool,
    ) -> Result<bool, Exception>;
    /// Whether sensor privacy restricts `op` for every app (a global
    /// restriction).
    fn global_restriction(&self, op: i32) -> Result<bool, Exception>;
    /// The package `name` (`getPackageStateInternal`), in `user`.
    fn package(&self, name: &str, user: i32) -> Result<Option<Package>, Exception>;
    /// Whether a package of `uid` is installed (`initializeUidStates`).
    fn uid_has_package(&self, uid: i32) -> Result<bool, Exception>;
    /// Whether the parsed package `name` declares the attribution `tag`
    /// (`isAttributionInPackage` of `getPackage`): any for no tag, none
    /// without the package.
    fn attribution_in_package(&self, name: &str, tag: Option<&str>) -> Result<bool, Exception>;
    /// `PackageManagerInternal.isSystemPackage`.
    fn system_package(&self, name: &str) -> Result<bool, Exception>;
    /// Whether SECURITY_EXCEPTION_ON_INVALID_ATTRIBUTION_TAG_CHANGE is
    /// enabled for `package` and for the uid `caller`.
    fn invalid_tags_rejected(&self, package: &str, caller: i32) -> Result<bool, Exception>;
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
    /// `AppOpsPolicy.mActivityRecognitionTags`: the system activity
    /// recognizers' tags (`initializeActivityRecognizersTags`), by app id
    /// and package.
    fn activity_recognition_tags(&self) -> Result<Vec<(i32, String, Vec<String>)>, Exception>;
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

/// `verifyAndGetBypass`: the package's restriction bypass and whether the
/// attribution tag is valid for it, or that it is not the uid's (its
/// SecurityException).
enum Verified {
    Bypass(Option<Bypass>, bool),
    NotUids,
}

fn app_id(uid: i32) -> i32 {
    uid.rem_euclid(PER_USER_RANGE)
}

/// `verifyIncomingOp`: a known op, a restricted-read one only for the app
/// ops managers.
fn verify_incoming_op(
    sys: &dyn System,
    caller: i32,
    code: i32,
) -> Result<&'static AppOpInfo, Exception> {
    let info = usize::try_from(code)
        .ok()
        .and_then(|i| APP_OP_INFOS.get(i))
        .ok_or_else(|| Exception::illegal_argument(format!("Bad operation #{code}")))?;
    if info.restrict_read
        && !READ_RESTRICTED.iter().try_fold(false, |held, p| {
            Ok::<_, Exception>(held || sys.holds(caller, p)?)
        })?
    {
        return Err(Exception::security(format!(
            "verifyIncomingOp: uid {caller} does not have any of {{MANAGE_APPOPS, GET_APP_OPS_STATS, MANAGE_APP_OPS_MODES}}"
        )));
    }
    Ok(info)
}

/// `verifyIncomingUid`: the caller's own uid, else UPDATE_APP_OPS_STATS.
/// The system server's own calls are the system uid's, which holds it.
fn verify_incoming_uid(sys: &dyn System, caller: i32, uid: i32) -> Result<(), Exception> {
    if uid == caller || sys.holds(caller, UPDATE_APP_OPS_STATS)? {
        return Ok(());
    }
    Err(Exception::security(format!(
        "uid {caller} does not have {UPDATE_APP_OPS_STATS}."
    )))
}

/// `AttributionSource.getNext`: the first of `next`.
fn next_of(source: &AttributionSourceState) -> Result<Option<&AttributionSourceState>, Exception> {
    match source.next.as_deref().and_then(<[_]>::first) {
        None => Ok(None),
        Some(next) => next
            .as_ref()
            .map(Some)
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "next")),
    }
}

/// `AttributionSource.isTrusted`: a source with a token the permission
/// manager registered (#1527).
fn trusted(sys: &dyn System, source: &AttributionSourceState) -> Result<bool, Exception> {
    match source.token {
        None => Ok(false),
        Some(_) => Err(
            sys.unimplemented("trusted attribution sources (isRegisteredAttributionSource, #1527)")
        ),
    }
}

/// `verifyIncomingProxyUid`: the caller's own source, or the system
/// server's call, or a trusted source, else UPDATE_APP_OPS_STATS.
fn verify_incoming_proxy_uid(
    sys: &dyn System,
    caller: i32,
    source: &AttributionSourceState,
) -> Result<(), Exception> {
    if source.uid == caller || caller == SYSTEM_UID || trusted(sys, source)? {
        return Ok(());
    }
    verify_incoming_uid(sys, caller, source.uid)
}

/// `isCallerAndAttributionTrusted`: another uid's trusted source (and its
/// next), or a caller with UPDATE_APP_OPS_STATS.
fn caller_and_attribution_trusted(
    sys: &dyn System,
    caller: i32,
    source: &AttributionSourceState,
) -> Result<bool, Exception> {
    if source.uid != caller && trusted(sys, source)? {
        match next_of(source)? {
            None => return Ok(true),
            Some(next) if trusted(sys, next)? => return Ok(true),
            Some(_) => {}
        }
    }
    sys.holds(caller, UPDATE_APP_OPS_STATS)
}

/// `opToDefaultMode`.
fn default_mode(sys: &dyn System, info: &AppOpInfo) -> Result<i32, Exception> {
    Ok(match info.default_mode {
        DefaultMode::Mode(mode) => mode,
        // `ActivityManager.isLowRamDeviceStatic`.
        DefaultMode::SystemAlertWindow => {
            let low_ram =
                super::system_boolean(sys.property("ro.config.low_ram").as_deref(), false)
                    || (sys.property("ro.debuggable").as_deref() == Some("1")
                        && super::system_boolean(
                            sys.property("debug.force_low_ram").as_deref(),
                            false,
                        ));
            if low_ram && !sys.feature(FEATURE_LEANBACK)? {
                MODE_IGNORED
            } else {
                MODE_DEFAULT
            }
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
fn valid_package(
    sys: &dyn System,
    caller: i32,
    package: Option<&str>,
    user: i32,
) -> Result<bool, Exception> {
    let Some(package) = package else {
        return Ok(true);
    };
    if caller == SYSTEM_UID
        || resolve_package_name(caller, Some(package))
            .as_deref()
            .and_then(non_app_uid)
            .is_some()
    {
        return Ok(true);
    }
    Ok(sys.package(package, user)?.is_some() && !sys.filtered(package, caller, user)?)
}

/// `verifyAndGetBypass` of `uid`'s `package` and attribution `tag` for
/// `caller`, with the proxy's uid and package for a proxied op: root has
/// no bypass, a non-app uid's own name is unrestricted (with any tag
/// unless a proxy that is an app is not a system one), else the package
/// must be the uid's and declare the tag, or its proxy must, or the tag
/// is kept where the package or the caller may give an undeclared one.
fn verify_and_get_bypass(
    sys: &dyn System,
    caller: i32,
    uid: i32,
    package: Option<&str>,
    tag: Option<&str>,
    proxy: Option<(i32, &str)>,
) -> Result<Verified, Exception> {
    if uid == ROOT_UID {
        return Ok(Verified::Bypass(None, true));
    }
    if SDK_SANDBOX_UIDS.contains(&app_id(uid)) {
        return Err(sys.unimplemented("an SDK sandbox uid's ops (#1562)"));
    }
    let non_app = if package == Some("com.android.shell") {
        Some(SHELL_UID)
    } else {
        package.and_then(non_app_uid)
    };
    if let Some(non_app) = non_app {
        if non_app != app_id(uid) {
            return Ok(Verified::NotUids);
        }
        let proxy_system_or_none = match proxy {
            Some((proxy_uid, proxy_package)) if app_id(proxy_uid) >= FIRST_APPLICATION_UID => {
                sys.system_package(proxy_package)?
            }
            _ => true,
        };
        return Ok(Verified::Bypass(Some(UNRESTRICTED), proxy_system_or_none));
    }
    let user = uid.div_euclid(PER_USER_RANGE);
    let Some(package) = package else {
        return Ok(Verified::NotUids);
    };
    let Some(ps) = sys.package(package, user)?.filter(|p| p.parsed) else {
        return Ok(Verified::NotUids);
    };
    let mut tag_valid = sys.attribution_in_package(package, tag)?;
    if !tag_valid {
        tag_valid = match proxy {
            Some((_, proxy_package)) => sys.attribution_in_package(proxy_package, tag)?,
            None => false,
        } || !sys.invalid_tags_rejected(package, caller)?;
    }
    if user * PER_USER_RANGE + ps.app_id != uid {
        return Ok(Verified::NotUids);
    }
    // `getBypassforPackage`.
    let bypass = Bypass {
        system_uid: ps.app_id == SYSTEM_UID,
        privileged: ps.privileged,
        record_audio_exempt: sys.holds(ps.app_id, EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS)?,
    };
    Ok(Verified::Bypass(Some(bypass), tag_valid))
}

/// Whether appops keeps a uid state of `uid` (`getOpsLocked`): the
/// non-package uids' and the installed packages'.
fn has_ops(sys: &dyn System, uid: i32) -> Result<bool, Exception> {
    Ok(NON_PACKAGE_UIDS.contains(&uid) || sys.uid_has_package(uid)?)
}

/// `isOpRestrictedLocked` of the default device: a global restriction, or
/// a client's user restriction of the op (UserManager's, of user 0, or one
/// set with `setUserRestriction` past its excluded packages and tags)
/// unless the app may bypass it.
#[allow(clippy::too_many_arguments)]
fn restricted(
    sys: &dyn System,
    uid: i32,
    info: &AppOpInfo,
    bypass: Option<Bypass>,
    package: &str,
    tag: Option<&str>,
    is_check: bool,
) -> Result<bool, Exception> {
    if sys.global_restriction(info.code)? {
        return Ok(true);
    }
    if info.code == OP_CAMERA
        && sys.feature(FEATURE_AUTOMOTIVE)?
        && sys.flag(CAMERA_PRIVACY_ALLOWLIST)?
    {
        return Err(sys.unimplemented("the camera privacy allowlist (#1563)"));
    }
    let user = uid.div_euclid(PER_USER_RANGE);
    let by_user_manager = match info.restriction {
        Some(restriction) => user == 0 && sys.user_restriction(restriction)?,
        None => false,
    };
    if !by_user_manager && !sys.client_restricted(info.code, user, package, tag, is_check)? {
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
    Ok(matches!(app_id(uid), ROOT_UID | SYSTEM_UID)
        && sys.flag(RUNTIME_PERMISSION_APPOPS_MAPPING)?)
}

/// The switch's mode: the uid's where it is not the switch's default,
/// else the package's (`getUidMode`, `getPackageMode`). OP_NONE's entry
/// (the deprecated op 96) has no switch.
fn persisted_mode(
    sys: &dyn System,
    uid: i32,
    package: &str,
    info: &AppOpInfo,
) -> Result<i32, Exception> {
    if info.code == OP_NONE {
        return Err(Exception::new(
            EX_ILLEGAL_STATE,
            "a deprecated app op has no switch",
        ));
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

/// `UidState.getState`: a uid with a process is top (each of the app
/// host's processes is), one without cached.
fn uid_state(sys: &dyn System, uid: i32) -> i32 {
    if sys.running(uid) {
        history::UID_STATE_TOP
    } else {
        history::UID_STATE_CACHED
    }
}

/// Whether a note or a start reads the switch op's package mode, making
/// that `Op` (`getOpLocked(ops, switchCode, uid, true)`): no root or
/// system allowance, the uid's mode the switch's default.
fn reads_package_mode(
    sys: &dyn System,
    uid: i32,
    info: &AppOpInfo,
    allow_uid: bool,
) -> Result<bool, Exception> {
    if info.code == OP_NONE || (allow_uid && allowed_for_uid(sys, uid)?) {
        return Ok(false);
    }
    let switch = &APP_OP_INFOS[info.switch_code as usize];
    let default = default_mode(sys, switch)?;
    Ok(sys.uid_mode(app_id(uid), switch.name)?.unwrap_or(default) == default)
}

/// The proxy of an op noted or started for another app, as its event
/// records it (`OpEventProxyInfo`).
fn proxy_info(op: &Unchecked<'_>) -> Option<ProxyInfo> {
    op.proxy.map(|(uid, package)| ProxyInfo {
        uid,
        package: Some(package.to_string()),
        tag: op.proxy_tag.map(String::from),
        device_id: PERSISTENT_DEVICE_ID_DEFAULT.to_string(),
    })
}

/// The mode a note or a start evaluates: root and system's
/// (`isOpAllowedForUid`, which a start's dry run does not ask), else the
/// switch's.
fn op_mode(
    sys: &dyn System,
    uid: i32,
    package: &str,
    info: &AppOpInfo,
    allow_uid: bool,
) -> Result<i32, Exception> {
    if allow_uid && allowed_for_uid(sys, uid)? {
        return Ok(MODE_ALLOWED);
    }
    Ok(eval(sys, uid, persisted_mode(sys, uid, package, info)?))
}

/// `checkOperationImpl`, by `getAppOpMode`: `raw` keeps a foreground mode.
/// The app host has no virtual devices (`isValidVirtualDeviceId`).
fn check(
    sys: &dyn System,
    caller: i32,
    code: i32,
    uid: i32,
    package: Option<&str>,
    device: i32,
    raw: bool,
) -> Result<i32, Exception> {
    let user = uid.div_euclid(PER_USER_RANGE);
    let validate = sys.flag(CHECK_OP_VALIDATE_PACKAGE)?;
    let info = verify_incoming_op(sys, caller, code)?;
    if device != DEVICE_ID_DEFAULT {
        return Ok(MODE_IGNORED);
    }
    if !valid_package(sys, caller, package, user)? {
        return if validate {
            Ok(MODE_IGNORED)
        } else {
            default_mode(sys, info)
        };
    }
    let Some(package) = resolve_package_name(uid, package) else {
        return Ok(MODE_IGNORED);
    };
    if !sys.flag(APPOP_MODE_CACHING)? {
        return Err(sys.unimplemented("checkOperationUnchecked (#1565)"));
    }
    let Verified::Bypass(bypass, _) =
        verify_and_get_bypass(sys, caller, uid, Some(&package), None, None)?
    else {
        return Ok(MODE_IGNORED);
    };
    // `isOpRestrictedDueToSuspend`.
    if OPS_RESTRICTED_ON_SUSPEND.contains(&code)
        && sys.package(&package, user)?.is_some_and(|p| p.suspended)
    {
        return Ok(MODE_IGNORED);
    }
    if restricted(sys, uid, info, bypass, &package, None, true)? {
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
/// none (nor a sandboxed one, `resolveSandboxedServiceOp`); a location or
/// activity recognition op noted with a datasource's attribution tag is its
/// `*_SOURCE` op. The location providers' tags (`mLocationTags`) are none:
/// in the app host the location service reports none (#1748).
fn resolve_datasource_op(
    sys: &dyn System,
    code: i32,
    uid: i32,
    package: Option<&str>,
    tag: Option<&str>,
) -> Result<i32, Exception> {
    // `isHotwordDetectionServiceRequired`.
    let hotword_required = || -> Result<bool, Exception> {
        Ok(!sys.feature(FEATURE_AUTOMOTIVE)?
            && !sys.feature(FEATURE_LEANBACK)?
            && super::system_boolean(
                sys.property("ro.hotword.detection_service_required")
                    .as_deref(),
                false,
            ))
    };
    let code = if code == OP_RECORD_AUDIO_HOTWORD && hotword_required()? {
        OP_RECORD_AUDIO
    } else {
        code
    };
    let (Some(tag), Some(&(_, source))) = (tag, DATASOURCE_OPS.iter().find(|(c, _)| *c == code))
    else {
        return Ok(code);
    };
    if code != OP_ACTIVITY_RECOGNITION {
        return Ok(code);
    }
    // `isDatasourceAttributionTag`.
    let app_id = app_id(uid);
    let datasource = sys.activity_recognition_tags()?.iter().any(|(a, p, tags)| {
        *a == app_id && Some(p.as_str()) == package && tags.iter().any(|t| t == tag)
    });
    Ok(if datasource { source } else { code })
}

/// `validateOpRequest`, or the checks before it (by
/// `check_op_validate_package`) of `noteOperationImpl` and
/// `startOperationImpl`: the package resolved, or the mode a request that
/// fails them answers. The app host has no virtual devices.
fn validate_request(
    sys: &dyn System,
    caller: i32,
    code: i32,
    uid: i32,
    package: Option<&str>,
    device: i32,
) -> Result<Result<String, i32>, Exception> {
    let validate = sys.flag(CHECK_OP_VALIDATE_PACKAGE)?;
    if validate {
        verify_incoming_op(sys, caller, code)?;
        verify_incoming_uid(sys, caller, uid)?;
    } else {
        verify_incoming_uid(sys, caller, uid)?;
        verify_incoming_op(sys, caller, code)?;
    }
    if device != DEVICE_ID_DEFAULT {
        return Ok(Err(MODE_IGNORED));
    }
    if !valid_package(sys, caller, package, uid.div_euclid(PER_USER_RANGE))? {
        return Ok(Err(if validate { MODE_IGNORED } else { MODE_ERRORED }));
    }
    Ok(resolve_package_name(uid, package).ok_or(MODE_IGNORED))
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
    let info = usize::try_from(code)
        .ok()
        .and_then(|i| APP_OP_INFOS.get(i))
        .ok_or_else(|| {
            Exception::illegal_argument(format!(
                "opCode is out of range of [0, {}] ({side})",
                NUM_OP - 1
            ))
        })?;
    if info.force_collect_notes {
        return Ok(true);
    }
    let permission = match info.permission {
        Some((permission, None)) => permission,
        Some((permission, Some(flag))) if sys.flag(flag)? => permission,
        _ => return Ok(false),
    };
    Ok(protection(permission)?.is_some_and(|level| {
        level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS || level & PROTECTION_FLAG_APPOP != 0
    }))
}

/// `Preconditions.checkArrayElementsInRange` of the ops a watcher gives.
fn ops_in_range(ops: &[i32]) -> Result<(), Exception> {
    let name = format!(
        "Invalid op code in: [{}]",
        ops.iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (i, &op) in ops.iter().enumerate() {
        let side = if op < 0 {
            "too low"
        } else if op > NUM_OP - 1 {
            "too high"
        } else {
            continue;
        };
        return Err(Exception::illegal_argument(format!(
            "{name}[{i}] is out of range of [0, {}] ({side})",
            NUM_OP - 1
        )));
    }
    Ok(())
}

/// `SyncNotedAppOp`: the mode an op was noted or started with, for the
/// attribution tag and the package it was decided for.
#[derive(Debug, PartialEq)]
pub(super) struct SyncNoted {
    pub mode: i32,
    op: i32,
    tag: Option<String>,
    package: Option<String>,
}

impl SyncNoted {
    fn new(mode: i32, op: i32, tag: Option<&str>, package: Option<&str>) -> Self {
        SyncNoted {
            mode,
            op,
            tag: tag.map(String::from),
            package: package.map(String::from),
        }
    }
}

/// Its parcel: which nullable fields follow, the mode, the op, the
/// attribution tag, the package.
impl WriteParcelable for SyncNoted {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(
            if self.tag.is_some() { 0x4 } else { 0 } | if self.package.is_some() { 0x8 } else { 0 },
        );
        p.write_i32(self.mode);
        p.write_i32(self.op);
        if let Some(tag) = &self.tag {
            p.write_string16(Some(tag));
        }
        if let Some(package) = &self.package {
            p.write_string16(Some(package));
        }
    }
}

/// An op of a uid's package and attribution tag as `noteOperationUnchecked`
/// and `startOperationUnchecked` take it: the proxy's uid and package for a
/// proxied one, its `OP_FLAG_*`, and the message it is collected with for
/// the app (`shouldCollectAsyncNotedOp`).
struct Unchecked<'a> {
    code: i32,
    uid: i32,
    package: Option<&'a str>,
    tag: Option<&'a str>,
    proxy: Option<(i32, &'a str)>,
    /// The proxy's attribution tag, as its events record it.
    proxy_tag: Option<&'a str>,
    flags: i32,
    collect: Option<String>,
}

/// How a start is attributed: whether it starts at the default mode, its
/// `ATTRIBUTION_FLAG_*` and its attribution chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Attribution {
    pub start_if_mode_default: bool,
    pub flags: i32,
    pub chain_id: i32,
}

/// How a proxy start is attributed: as `Attribution`, with the proxy's and
/// the proxied's flags.
#[derive(Clone, Copy, Debug)]
pub(super) struct ProxyAttribution {
    pub start_if_mode_default: bool,
    pub proxy_flags: i32,
    pub proxied_flags: i32,
    pub chain_id: i32,
}

/// `shouldStartForMode`.
fn should_start(mode: i32, start_if_mode_default: bool) -> bool {
    mode == MODE_ALLOWED || (mode == MODE_DEFAULT && start_if_mode_default)
}

/// An op of a uid's package (`Op`).
type OpKey = (i32, String, i32);

/// A client binder of an attributed op (`AttributedOp`), which its death
/// finishes.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Client {
    key: OpKey,
    tag: Option<String>,
    binder: Binder,
}

/// A client's unfinished starts of an attributed op
/// (`InProgressStartOpEvent`): their count, the first's `OP_FLAG_*` and
/// attribution, and the link to the client's death.
struct InProgress<L> {
    starts: u32,
    flags: i32,
    attribution_flags: i32,
    chain_id: i32,
    link: L,
    /// Its start, by the wall clock and the monotonic one, the uid's state
    /// then and its proxy.
    start_time: i64,
    started: std::time::Instant,
    uid_state: i32,
    proxy: Option<ProxyInfo>,
}

impl<L> InProgress<L> {
    /// The access it is so far (`createAttributedOpEntryLocked`'s, and
    /// `finishOrPause`'s): from its start for how long it ran.
    fn access(&self) -> (i64, NoteEvent) {
        let duration = self.started.elapsed().as_millis() as i64;
        (
            history::make_key(self.uid_state, self.flags),
            NoteEvent {
                time: self.start_time,
                duration,
                proxy: self.proxy.clone(),
            },
        )
    }
}

/// `AttributedOp`: its clients' running and paused (restricted at their
/// start) events, and its last accesses and rejections.
struct Attributed<L> {
    running: HashMap<Binder, InProgress<L>>,
    paused: HashMap<Binder, InProgress<L>>,
    events: Events,
}

impl<L> Default for Attributed<L> {
    fn default() -> Self {
        Attributed {
            running: HashMap::new(),
            paused: HashMap::new(),
            events: Events::default(),
        }
    }
}

impl<L> Attributed<L> {
    /// `createAttributedOpEntryLocked`: its accesses with the running
    /// events as accesses so far.
    fn entry(&self, op: i32) -> AttributedEntry {
        let mut access = self.events.access.clone();
        if !self.running.is_empty() {
            let access = access.get_or_insert_with(Default::default);
            let mut running: Vec<&InProgress<L>> = self.running.values().collect();
            running.sort_by_key(|e| e.start_time);
            for event in running {
                let (key, event) = event.access();
                access.insert(key, event);
            }
        }
        AttributedEntry {
            op,
            running: !self.running.is_empty(),
            access,
            reject: self.events.reject.clone(),
        }
    }
}

/// Which watchers: `startWatchingActive`'s or `startWatchingStarted`'s.
#[derive(Clone, Copy)]
enum Watch {
    Active,
    Started,
}

/// A watcher (`ActiveCallback`, `StartedCallback`): by op, the uid it
/// watches (`Process.INVALID_UID` for all) and the uid that registered
/// it; and the link to its death.
struct Watcher<L> {
    link: L,
    ops: HashMap<i32, (i32, i32)>,
}

/// What a watcher hears: `opActiveChanged`, or `opStarted` with the op's
/// flags, mode and start type.
#[derive(Debug, PartialEq)]
enum Heard {
    Active(bool),
    Started {
        flags: i32,
        mode: i32,
        start_type: i32,
    },
}

/// A call to the watchers of an op (`scheduleOpActiveChangedIfNeededLocked`,
/// `scheduleOpStartedIfNeededLocked`): their binders and the uids that
/// registered them.
#[derive(Debug, PartialEq)]
pub(super) struct Notice {
    watchers: Vec<(u32, i32)>,
    op: i32,
    uid: i32,
    package: Option<String>,
    tag: Option<String>,
    attribution_flags: i32,
    chain_id: i32,
    heard: Heard,
}

/// An op noted or started for an app, to collect for it
/// (`collectAsyncNotedOp`).
#[derive(Debug, PartialEq)]
struct Collected {
    uid: i32,
    package: String,
    op: i32,
    tag: Option<String>,
    message: String,
}

/// What a note, start or finish leaves to do once appops' lock is
/// released: the watchers to call and the ops to collect.
#[derive(Default)]
struct Effects {
    notices: Vec<Notice>,
    collected: Vec<Collected>,
}

/// The ops of each uid's packages by attribution tag (`Op`, made by a note
/// or a start, with its attributed ops' history), and the watchers of their
/// starts.
pub(super) struct Started<L> {
    ops: HashMap<OpKey, HashMap<Option<String>, Attributed<L>>>,
    active: HashMap<u32, Watcher<L>>,
    started: HashMap<u32, Watcher<L>>,
}

impl<L> Default for Started<L> {
    fn default() -> Self {
        Started {
            ops: HashMap::new(),
            active: HashMap::new(),
            started: HashMap::new(),
        }
    }
}

impl<L> Started<L> {
    fn watchers(&mut self, which: Watch) -> &mut HashMap<u32, Watcher<L>> {
        match which {
            Watch::Active => &mut self.active,
            Watch::Started => &mut self.started,
        }
    }

    /// `Op.isRunning`: a client of one of its attributed ops runs.
    fn running(&self, key: &OpKey) -> bool {
        self.ops
            .get(key)
            .is_some_and(|tags| tags.values().any(|a| !a.running.is_empty()))
    }

    /// `binder` watches `ops` of `watched` for the uid `calling`.
    fn watch(
        &mut self,
        which: Watch,
        binder: u32,
        ops: &[i32],
        watched: i32,
        calling: i32,
        link: impl FnOnce() -> L,
    ) {
        let watcher = self
            .watchers(which)
            .entry(binder)
            .or_insert_with(|| Watcher {
                link: link(),
                ops: HashMap::new(),
            });
        for &op in ops {
            watcher.ops.insert(op, (watched, calling));
        }
    }

    fn unwatch(&mut self, which: Watch, binder: u32) -> Option<L> {
        self.watchers(which).remove(&binder).map(|w| w.link)
    }

    /// Tells `which` watchers of `op` that watch `uid`.
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        which: Watch,
        op: i32,
        uid: i32,
        package: Option<&str>,
        tag: Option<&str>,
        attribution_flags: i32,
        chain_id: i32,
        heard: Heard,
        effects: &mut Effects,
    ) {
        let watchers = match which {
            Watch::Active => &self.active,
            Watch::Started => &self.started,
        };
        let mut watching: Vec<(u32, i32)> = watchers
            .iter()
            .filter_map(|(binder, w)| {
                w.ops
                    .get(&op)
                    .filter(|(watched, _)| *watched < 0 || *watched == uid)
                    .map(|(_, calling)| (*binder, *calling))
            })
            .collect();
        if watching.is_empty() {
            return;
        }
        watching.sort_unstable();
        effects.notices.push(Notice {
            watchers: watching,
            op,
            uid,
            package: package.map(String::from),
            tag: tag.map(String::from),
            attribution_flags,
            chain_id,
            heard,
        });
    }

    fn notify_active(
        &self,
        client: &Client,
        active: bool,
        attribution_flags: i32,
        chain_id: i32,
        effects: &mut Effects,
    ) {
        let (uid, package, op) = &client.key;
        let heard = Heard::Active(active);
        self.notify(
            Watch::Active,
            *op,
            *uid,
            Some(package),
            client.tag.as_deref(),
            attribution_flags,
            chain_id,
            heard,
            effects,
        );
    }

    /// `getOpLocked(edit)`: the op of a package, made empty if new.
    fn op(&mut self, key: &OpKey) {
        self.ops.entry(key.clone()).or_default();
    }

    /// `getOrCreateAttribution`.
    fn attributed(&mut self, key: &OpKey, tag: Option<&str>) -> &mut Attributed<L> {
        self.ops
            .entry(key.clone())
            .or_default()
            .entry(tag.map(String::from))
            .or_default()
    }

    /// `Op.createEntryLocked`: its attributions' entries in `ArrayMap`
    /// order, with the package's mode.
    fn entry(&self, key: &OpKey, mode: i32) -> OpEntry {
        let mut attributed: Vec<(Option<String>, AttributedEntry)> = self.ops[key]
            .iter()
            .map(|(tag, a)| (tag.clone(), a.entry(key.2)))
            .collect();
        attributed.sort_by(|(a, _), (b, _)| a.cmp(b));
        history::array_map_order(&mut attributed);
        OpEntry {
            op: key.2,
            mode,
            attributed,
        }
    }

    /// `AttributedOp.started`, or `createPaused` for a restricted op: one
    /// more start of `client`, the op active from its first.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        client: &Client,
        restricted: bool,
        flags: i32,
        attribution: Attribution,
        uid_state: i32,
        proxy: Option<ProxyInfo>,
        link: &mut dyn FnMut(&Client) -> L,
        effects: &mut Effects,
    ) {
        if !restricted && !self.running(&client.key) {
            self.notify_active(
                client,
                true,
                attribution.flags,
                attribution.chain_id,
                effects,
            );
        }
        let attributed = self
            .ops
            .entry(client.key.clone())
            .or_default()
            .entry(client.tag.clone())
            .or_default();
        let events = if restricted {
            &mut attributed.paused
        } else {
            &mut attributed.running
        };
        let event = events.entry(client.binder).or_insert_with(|| InProgress {
            starts: 0,
            flags,
            attribution_flags: attribution.flags,
            chain_id: attribution.chain_id,
            link: link(client),
            start_time: history::now_ms(),
            started: std::time::Instant::now(),
            uid_state,
            proxy,
        });
        event.starts += 1;
    }

    /// `AttributedOp.finished`: one start of `client` finished, the access
    /// recorded once all of a running event's are, the op inactive once
    /// none of its clients runs; the links of the events that ended.
    fn finished(&mut self, client: &Client, effects: &mut Effects) -> Vec<L> {
        let Some(attributed) = self
            .ops
            .get_mut(&client.key)
            .and_then(|tags| tags.get_mut(&client.tag))
        else {
            return Vec::new();
        };
        let mut ended = Vec::new();
        let mut inactive = None;
        if let Some(event) = attributed.running.get_mut(&client.binder) {
            event.starts -= 1;
            if event.starts == 0 {
                let event = attributed
                    .running
                    .remove(&client.binder)
                    .expect("a running event");
                let (key, access) = event.access();
                attributed.events.accessed(key, access);
                inactive = attributed
                    .running
                    .is_empty()
                    .then_some((event.attribution_flags, event.chain_id));
                ended.push(event.link);
            }
        } else if let Some(event) = attributed.paused.get_mut(&client.binder) {
            // `finishPossiblyPaused`.
            event.starts -= 1;
            if event.starts == 0 {
                ended.push(
                    attributed
                        .paused
                        .remove(&client.binder)
                        .expect("a paused event")
                        .link,
                );
            }
        }
        if let Some((attribution_flags, chain_id)) = inactive
            && !self.running(&client.key)
        {
            self.notify_active(client, false, attribution_flags, chain_id, effects);
        }
        ended
    }

    /// `AttributedOp.pause`: its running events paused, each recorded as an
    /// access so far, the active watchers told of each that it is inactive.
    fn pause(&mut self, key: &OpKey, tag: &Option<String>, effects: &mut Effects) {
        let attributed = self
            .ops
            .get_mut(key)
            .and_then(|tags| tags.get_mut(tag))
            .expect("a started op");
        let events: Vec<(Binder, InProgress<L>)> = attributed.running.drain().collect();
        for (_, event) in &events {
            let (key, access) = event.access();
            attributed.events.accessed(key, access);
        }
        let notices: Vec<(Binder, i32, i32)> = events
            .iter()
            .map(|(b, e)| (*b, e.attribution_flags, e.chain_id))
            .collect();
        attributed.paused.extend(events);
        for (binder, attribution_flags, chain_id) in notices {
            let client = Client {
                key: key.clone(),
                tag: tag.clone(),
                binder,
            };
            self.notify_active(&client, false, attribution_flags, chain_id, effects);
        }
    }

    /// `AttributedOp.resume`: its paused events running again from now,
    /// active once more if none ran, and the started watchers told of each
    /// (START_TYPE_RESUMED, always MODE_ALLOWED as the original).
    fn resume(&mut self, key: &OpKey, tag: &Option<String>, effects: &mut Effects) {
        let attributed = self
            .ops
            .get_mut(key)
            .and_then(|tags| tags.get_mut(tag))
            .expect("a started op");
        let send_active = !attributed.paused.is_empty() && attributed.running.is_empty();
        let mut events: Vec<(Binder, InProgress<L>)> = attributed.paused.drain().collect();
        let (now, started) = (history::now_ms(), std::time::Instant::now());
        for (_, event) in &mut events {
            event.start_time = now;
            event.started = started;
        }
        let notices: Vec<(i32, i32, i32)> = events
            .iter()
            .map(|(_, e)| (e.flags, e.attribution_flags, e.chain_id))
            .collect();
        attributed.running.extend(events);
        let (uid, package, op) = key;
        for (flags, attribution_flags, chain_id) in notices {
            if send_active {
                let heard = Heard::Active(true);
                self.notify(
                    Watch::Active,
                    *op,
                    *uid,
                    Some(package),
                    tag.as_deref(),
                    attribution_flags,
                    chain_id,
                    heard,
                    effects,
                );
            }
            let heard = Heard::Started {
                flags,
                mode: MODE_ALLOWED,
                start_type: START_TYPE_RESUMED,
            };
            self.notify(
                Watch::Started,
                *op,
                *uid,
                Some(package),
                tag.as_deref(),
                attribution_flags,
                chain_id,
                heard,
                effects,
            );
        }
    }

    /// `AttributedOp.onClientDeath`: the client's starts all finished, of
    /// the paused events while any is paused.
    fn client_died(&mut self, client: &Client, effects: &mut Effects) -> Vec<L> {
        let Some(attributed) = self
            .ops
            .get_mut(&client.key)
            .and_then(|tags| tags.get_mut(&client.tag))
        else {
            return Vec::new();
        };
        let events = if attributed.paused.is_empty() {
            &mut attributed.running
        } else {
            &mut attributed.paused
        };
        if let Some(event) = events.get_mut(&client.binder) {
            event.starts = 1;
        }
        self.finished(client, effects)
    }
}

/// AppOps as one caller reaches it: the system server it reads, and the
/// started ops it keeps with how a client's death is linked; what it
/// leaves to do, and the links of the events that ended.
struct Ops<'a, L> {
    sys: &'a dyn System,
    caller: i32,
    started: &'a mut Started<L>,
    link: &'a mut dyn FnMut(&Client) -> L,
    effects: Effects,
    ended: Vec<L>,
}

impl<L> Ops<'_, L> {
    /// `updateStartedOpModeForUserForDefaultDevice`: the started ops of
    /// `code` of the user (`USER_ALL`: of any) whose package mode allows
    /// them, paused where now restricted, resumed where no restriction is
    /// left.
    fn update_started_mode(
        &mut self,
        code: i32,
        restricted_now: bool,
        user: i32,
    ) -> Result<(), Exception> {
        let sys = self.sys;
        let info = &APP_OP_INFOS[code as usize];
        let mut keys: Vec<OpKey> = self
            .started
            .ops
            .keys()
            .filter(|(uid, _, op)| {
                *op == code && (user == USER_ALL || uid.div_euclid(PER_USER_RANGE) == user)
            })
            .cloned()
            .collect();
        keys.sort();
        for key in keys {
            let (uid, package, _) = &key;
            // `AppOpsCheckingService.getPackageMode`.
            let mode = match sys.package_mode(package, info.name)? {
                Some(mode) => mode,
                None => default_mode(sys, info)?,
            };
            if mode != MODE_ALLOWED && mode != MODE_FOREGROUND {
                continue;
            }
            let mut tags: Vec<Option<String>> = self.started.ops[&key].keys().cloned().collect();
            tags.sort();
            for tag in tags {
                let attributed = &self.started.ops[&key][&tag];
                if restricted_now && !attributed.running.is_empty() {
                    self.started.pause(&key, &tag, &mut self.effects);
                } else if !attributed.paused.is_empty() {
                    let Verified::Bypass(bypass, _) = verify_and_get_bypass(
                        sys,
                        SYSTEM_UID,
                        *uid,
                        Some(package),
                        tag.as_deref(),
                        None,
                    )?
                    else {
                        return Err(Exception::security(format!(
                            "Specified package \"{package}\" under uid {uid} but it is not"
                        )));
                    };
                    if !restricted(sys, *uid, info, bypass, package, tag.as_deref(), false)? {
                        self.started.resume(&key, &tag, &mut self.effects);
                    }
                }
            }
        }
        Ok(())
    }

    /// `noteOperationImpl` behind `AppOpsPolicy.noteOperation`.
    fn note(&mut self, op: &Unchecked<'_>, device: i32) -> Result<SyncNoted, Exception> {
        let code = resolve_datasource_op(self.sys, op.code, op.uid, op.package, op.tag)?;
        let resolved =
            match validate_request(self.sys, self.caller, code, op.uid, op.package, device)? {
                Ok(resolved) => resolved,
                Err(mode) => return Ok(SyncNoted::new(mode, code, op.tag, op.package)),
            };
        self.note_unchecked(&Unchecked {
            code,
            package: Some(&resolved),
            collect: op.collect.clone(),
            ..*op
        })
    }

    /// `noteOperationUnchecked`: a package that is not the uid's errs.
    fn note_unchecked(&mut self, op: &Unchecked<'_>) -> Result<SyncNoted, Exception> {
        let sys = self.sys;
        let info = &APP_OP_INFOS[op.code as usize];
        let Verified::Bypass(bypass, tag_valid) =
            verify_and_get_bypass(sys, self.caller, op.uid, op.package, op.tag, op.proxy)?
        else {
            return Ok(SyncNoted::new(MODE_ERRORED, op.code, op.tag, op.package));
        };
        let tag = op.tag.filter(|_| tag_valid);
        if !has_ops(sys, op.uid)? {
            return Ok(SyncNoted::new(MODE_ERRORED, op.code, tag, op.package));
        }
        let package = op
            .package
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
        // The op and its attribution are made first, then the access or
        // the rejection recorded.
        let key = (op.uid, package.to_string(), op.code);
        let event = history::make_key(uid_state(sys, op.uid), op.flags);
        self.started.attributed(&key, tag);
        let mode = if restricted(sys, op.uid, info, bypass, package, tag, false)? {
            MODE_IGNORED
        } else {
            if info.switch_code != op.code && reads_package_mode(sys, op.uid, info, true)? {
                self.started
                    .op(&(op.uid, package.to_string(), info.switch_code));
            }
            op_mode(sys, op.uid, package, info, true)?
        };
        let events = &mut self.started.attributed(&key, tag).events;
        if mode == MODE_ALLOWED {
            events.accessed(
                event,
                NoteEvent {
                    time: history::now_ms(),
                    duration: -1,
                    proxy: proxy_info(op),
                },
            );
            self.collect(op, package, tag);
        } else {
            events.rejected(event, history::now_ms());
        }
        Ok(SyncNoted::new(mode, op.code, tag, Some(package)))
    }

    fn collect(&mut self, op: &Unchecked<'_>, package: &str, tag: Option<&str>) {
        if let Some(message) = &op.collect {
            let (uid, op, package, tag, message) = (
                op.uid,
                op.code,
                package.to_string(),
                tag.map(String::from),
                message.clone(),
            );
            self.effects.collected.push(Collected {
                uid,
                package,
                op,
                tag,
                message,
            });
        }
    }

    /// `noteProxyOperationImpl` behind `AppOpsPolicy`: the proxy's op noted
    /// (unless a trusted caller skips it), then the proxied's; a proxy
    /// without UPDATE_APP_OPS_STATS, but for the proxied's own call, is
    /// collected for itself.
    fn note_proxy(
        &mut self,
        code: i32,
        source: &AttributionSourceState,
        skip_proxy: bool,
        collect: bool,
        message: Option<&str>,
    ) -> Result<SyncNoted, Exception> {
        let (sys, caller) = (self.sys, self.caller);
        let code = resolve_datasource_op(
            sys,
            code,
            source.uid,
            source.package.as_deref(),
            source.tag.as_deref(),
        )?;
        let next = next_of(source)?;
        let proxied_uid = next.map_or(INVALID_UID, |n| n.uid);
        let (proxied_package, proxied_tag) = (
            next.and_then(|n| n.package.as_deref()),
            next.and_then(|n| n.tag.as_deref()),
        );
        let synced = |mode| SyncNoted::new(mode, code, proxied_tag, proxied_package);
        verify_incoming_proxy_uid(sys, caller, source)?;
        verify_incoming_op(sys, caller, code)?;
        if source.device_id != DEVICE_ID_DEFAULT {
            return Ok(synced(MODE_IGNORED));
        }
        if !valid_package(
            sys,
            caller,
            proxied_package,
            proxied_uid.div_euclid(PER_USER_RANGE),
        )? || !valid_package(
            sys,
            caller,
            source.package.as_deref(),
            source.uid.div_euclid(PER_USER_RANGE),
        )? {
            return Ok(synced(MODE_ERRORED));
        }
        let skip = skip_proxy && caller_and_attribution_trusted(sys, caller, source)?;
        let Some(proxy_package) = resolve_package_name(source.uid, source.package.as_deref())
        else {
            return Ok(synced(MODE_IGNORED));
        };
        let proxy_trusted = sys.holds(source.uid, UPDATE_APP_OPS_STATS)? || caller == proxied_uid;
        if !skip {
            let proxy = Unchecked {
                code,
                uid: source.uid,
                package: Some(&proxy_package),
                tag: source.tag.as_deref(),
                proxy: None,
                proxy_tag: None,
                flags: if proxy_trusted {
                    OP_FLAG_TRUSTED_PROXY
                } else {
                    OP_FLAG_UNTRUSTED_PROXY
                },
                collect: (!proxy_trusted).then(|| format!("proxy {}", message.unwrap_or("null"))),
            };
            let noted = self.note_unchecked(&proxy)?;
            if noted.mode != MODE_ALLOWED {
                return Ok(synced(noted.mode));
            }
        }
        let Some(proxied) = resolve_package_name(proxied_uid, proxied_package) else {
            return Ok(synced(MODE_IGNORED));
        };
        self.note_unchecked(&Unchecked {
            code,
            uid: proxied_uid,
            package: Some(&proxied),
            tag: proxied_tag,
            proxy: Some((source.uid, &proxy_package)),
            proxy_tag: source.tag.as_deref(),
            flags: if proxy_trusted {
                OP_FLAG_TRUSTED_PROXIED
            } else {
                OP_FLAG_UNTRUSTED_PROXIED
            },
            collect: collect.then(|| message.unwrap_or_default().to_string()),
        })
    }

    /// `startOperationImpl` behind `AppOpsPolicy.startOperation`: the
    /// hotword, ambient trigger and sandboxed audio ops need RECORD_AUDIO
    /// allowed, the sandboxed camera CAMERA; the package is started as
    /// given.
    fn start(
        &mut self,
        client: Option<Binder>,
        op: &Unchecked<'_>,
        attribution: Attribution,
    ) -> Result<SyncNoted, Exception> {
        let code = resolve_datasource_op(self.sys, op.code, op.uid, op.package, op.tag)?;
        if let Err(mode) = validate_request(
            self.sys,
            self.caller,
            code,
            op.uid,
            op.package,
            DEVICE_ID_DEFAULT,
        )? {
            return Ok(SyncNoted::new(mode, code, op.tag, op.package));
        }
        let needed = match code {
            OP_RECORD_AUDIO_HOTWORD
            | OP_RECEIVE_AMBIENT_TRIGGER_AUDIO
            | OP_RECORD_AUDIO_SANDBOXED => Some(OP_RECORD_AUDIO),
            OP_CAMERA_SANDBOXED => Some(OP_CAMERA),
            _ => None,
        };
        if let Some(needed) = needed {
            let mode = check(
                self.sys,
                self.caller,
                needed,
                op.uid,
                op.package,
                DEVICE_ID_DEFAULT,
                false,
            )?;
            if mode != MODE_ALLOWED {
                return Ok(SyncNoted::new(mode, code, op.tag, op.package));
            }
        }
        self.start_unchecked(
            client,
            &Unchecked {
                code,
                collect: op.collect.clone(),
                ..*op
            },
            attribution,
        )
    }

    /// `startOperationUnchecked`: where the op's mode allows it (or is the
    /// default, with `startIfModeDefault`) `client` starts it, paused while
    /// it is restricted; the started watchers hear each start and
    /// rejection.
    fn start_unchecked(
        &mut self,
        client: Option<Binder>,
        op: &Unchecked<'_>,
        attribution: Attribution,
    ) -> Result<SyncNoted, Exception> {
        let (tag, package, mode, restricted) = match decide(self.sys, self.caller, op, true)? {
            Decided::NotUids => {
                return Ok(SyncNoted::new(MODE_ERRORED, op.code, op.tag, op.package));
            }
            Decided::NoOps(tag) => {
                self.notify_started(op, tag, MODE_IGNORED, START_TYPE_FAILED, attribution);
                return Ok(SyncNoted::new(MODE_ERRORED, op.code, tag, op.package));
            }
            Decided::Mode {
                tag,
                package,
                mode,
                restricted,
            } => (tag, package, mode, restricted),
        };
        let key = (op.uid, package.to_string(), op.code);
        let uid_state = uid_state(self.sys, op.uid);
        self.started.attributed(&key, tag);
        let info = &APP_OP_INFOS[op.code as usize];
        if info.switch_code != op.code && reads_package_mode(self.sys, op.uid, info, true)? {
            self.started
                .op(&(op.uid, package.to_string(), info.switch_code));
        }
        if !should_start(mode, attribution.start_if_mode_default) {
            self.started
                .attributed(&key, tag)
                .events
                .rejected(history::make_key(uid_state, op.flags), history::now_ms());
            self.notify_started(op, tag, mode, START_TYPE_FAILED, attribution);
            return Ok(SyncNoted::new(mode, op.code, tag, Some(package)));
        }
        let binder = client.ok_or_else(|| Exception::new(EX_NULL_POINTER, "clientId"))?;
        let client = Client {
            key: (op.uid, package.to_string(), op.code),
            tag: tag.map(String::from),
            binder,
        };
        self.started.start(
            &client,
            restricted,
            op.flags,
            attribution,
            uid_state,
            proxy_info(op),
            self.link,
            &mut self.effects,
        );
        let (mode, start_type) = if restricted {
            (MODE_IGNORED, START_TYPE_FAILED)
        } else {
            (MODE_ALLOWED, START_TYPE_STARTED)
        };
        self.notify_started(op, tag, mode, start_type, attribution);
        if !restricted {
            self.collect(op, package, tag);
        }
        Ok(SyncNoted::new(mode, op.code, tag, Some(package)))
    }

    fn notify_started(
        &mut self,
        op: &Unchecked<'_>,
        tag: Option<&str>,
        mode: i32,
        start_type: i32,
        attribution: Attribution,
    ) {
        let heard = Heard::Started {
            flags: op.flags,
            mode,
            start_type,
        };
        let (flags, chain_id) = (attribution.flags, attribution.chain_id);
        self.started.notify(
            Watch::Started,
            op.code,
            op.uid,
            op.package,
            tag,
            flags,
            chain_id,
            heard,
            &mut self.effects,
        );
    }

    /// `startProxyOperationImpl` behind `AppOpsPolicy`: unless a trusted
    /// caller skips it, the proxy starts the op once the proxied could
    /// (`startOperationDryRun`), then the proxied starts it. A proxy is
    /// trusted with UPDATE_APP_OPS_STATS, for the proxied's own call, or in
    /// a trusted caller's trusted chain; one that is not is collected for
    /// itself.
    #[allow(clippy::too_many_arguments)]
    fn start_proxy(
        &mut self,
        client: Option<Binder>,
        code: i32,
        source: &AttributionSourceState,
        skip_proxy: bool,
        attribution: ProxyAttribution,
        collect: bool,
        message: Option<&str>,
    ) -> Result<SyncNoted, Exception> {
        let (sys, caller) = (self.sys, self.caller);
        let code = resolve_datasource_op(
            sys,
            code,
            source.uid,
            source.package.as_deref(),
            source.tag.as_deref(),
        )?;
        let next = next_of(source)?;
        let (proxied_uid, proxied_device) =
            next.map_or((INVALID_UID, DEVICE_ID_DEFAULT), |n| (n.uid, n.device_id));
        let (proxied_package, proxied_tag) = (
            next.and_then(|n| n.package.as_deref()),
            next.and_then(|n| n.tag.as_deref()),
        );
        let synced = |mode| SyncNoted::new(mode, code, proxied_tag, proxied_package);
        verify_incoming_proxy_uid(sys, caller, source)?;
        verify_incoming_op(sys, caller, code)?;
        if source.device_id != DEVICE_ID_DEFAULT || proxied_device != DEVICE_ID_DEFAULT {
            return Ok(synced(MODE_IGNORED));
        }
        if !valid_package(
            sys,
            caller,
            source.package.as_deref(),
            source.uid.div_euclid(PER_USER_RANGE),
        )? || !valid_package(
            sys,
            caller,
            proxied_package,
            proxied_uid.div_euclid(PER_USER_RANGE),
        )? {
            return Ok(synced(MODE_ERRORED));
        }
        let caller_trusted = caller_and_attribution_trusted(sys, caller, source)?;
        let skip = caller_trusted && skip_proxy;
        let Some(proxy_package) = resolve_package_name(source.uid, source.package.as_deref())
        else {
            return Ok(synced(MODE_IGNORED));
        };
        let chain_trusted = caller_trusted
            && attribution.chain_id != ATTRIBUTION_CHAIN_ID_NONE
            && (attribution.proxy_flags | attribution.proxied_flags) & ATTRIBUTION_FLAG_TRUSTED
                != 0;
        let proxy_trusted =
            sys.holds(source.uid, UPDATE_APP_OPS_STATS)? || caller == proxied_uid || chain_trusted;
        let Some(proxied) = resolve_package_name(proxied_uid, proxied_package) else {
            return Ok(synced(MODE_IGNORED));
        };
        let start_if_mode_default = attribution.start_if_mode_default;
        let proxied_op = Unchecked {
            code,
            uid: proxied_uid,
            package: Some(&proxied),
            tag: proxied_tag,
            proxy: Some((source.uid, &proxy_package)),
            proxy_tag: source.tag.as_deref(),
            flags: if proxy_trusted {
                OP_FLAG_TRUSTED_PROXIED
            } else {
                OP_FLAG_UNTRUSTED_PROXIED
            },
            collect: collect.then(|| message.unwrap_or_default().to_string()),
        };
        if !skip {
            let tested = start_dry_run(sys, caller, &proxied_op, start_if_mode_default)?;
            if !should_start(tested.mode, start_if_mode_default) {
                return Ok(tested);
            }
            let proxy_op = Unchecked {
                code,
                uid: source.uid,
                package: Some(&proxy_package),
                tag: source.tag.as_deref(),
                proxy: None,
                proxy_tag: None,
                flags: if proxy_trusted {
                    OP_FLAG_TRUSTED_PROXY
                } else {
                    OP_FLAG_UNTRUSTED_PROXY
                },
                collect: (!proxy_trusted).then(|| format!("proxy {}", message.unwrap_or("null"))),
            };
            let flags = attribution.proxy_flags;
            let started = self.start_unchecked(
                client,
                &proxy_op,
                Attribution {
                    start_if_mode_default,
                    flags,
                    chain_id: attribution.chain_id,
                },
            )?;
            if !should_start(started.mode, start_if_mode_default) {
                return Ok(started);
            }
        }
        let flags = attribution.proxied_flags;
        self.start_unchecked(
            client,
            &proxied_op,
            Attribution {
                start_if_mode_default,
                flags,
                chain_id: attribution.chain_id,
            },
        )
    }

    /// `finishOperationImpl` behind `AppOpsPolicy.finishOperation`.
    fn finish(
        &mut self,
        client: Option<Binder>,
        code: i32,
        uid: i32,
        package: Option<&str>,
        tag: Option<&str>,
    ) -> Result<(), Exception> {
        let code = resolve_datasource_op(self.sys, code, uid, package, tag)?;
        verify_incoming_uid(self.sys, self.caller, uid)?;
        verify_incoming_op(self.sys, self.caller, code)?;
        if !valid_package(
            self.sys,
            self.caller,
            package,
            uid.div_euclid(PER_USER_RANGE),
        )? {
            return Ok(());
        }
        let Some(resolved) = resolve_package_name(uid, package) else {
            return Ok(());
        };
        self.finish_unchecked(client, code, uid, &resolved, tag)
    }

    /// `finishOperationUnchecked`: one of `client`'s starts of the op
    /// finished.
    fn finish_unchecked(
        &mut self,
        client: Option<Binder>,
        code: i32,
        uid: i32,
        package: &str,
        tag: Option<&str>,
    ) -> Result<(), Exception> {
        let Verified::Bypass(_, tag_valid) =
            verify_and_get_bypass(self.sys, self.caller, uid, Some(package), tag, None)?
        else {
            return Ok(());
        };
        if !has_ops(self.sys, uid)? {
            return Ok(());
        }
        let Some(binder) = client else { return Ok(()) };
        let client = Client {
            key: (uid, package.to_string(), code),
            tag: tag.filter(|_| tag_valid).map(String::from),
            binder,
        };
        let ended = self.started.finished(&client, &mut self.effects);
        self.ended.extend(ended);
        Ok(())
    }

    /// `finishProxyOperationImpl` behind `AppOpsPolicy`: the proxy's op
    /// (unless a trusted caller skips it) and the proxied's.
    fn finish_proxy(
        &mut self,
        client: Option<Binder>,
        code: i32,
        source: &AttributionSourceState,
        skip_proxy: bool,
    ) -> Result<(), Exception> {
        let (sys, caller) = (self.sys, self.caller);
        let code = resolve_datasource_op(
            sys,
            code,
            source.uid,
            source.package.as_deref(),
            source.tag.as_deref(),
        )?;
        let next = next_of(source)?;
        let proxied_uid = next.map_or(INVALID_UID, |n| n.uid);
        let (proxied_package, proxied_tag) = (
            next.and_then(|n| n.package.as_deref()),
            next.and_then(|n| n.tag.as_deref()),
        );
        let skip = skip_proxy && caller_and_attribution_trusted(sys, caller, source)?;
        verify_incoming_proxy_uid(sys, caller, source)?;
        verify_incoming_op(sys, caller, code)?;
        if source.device_id != DEVICE_ID_DEFAULT
            || !valid_package(
                sys,
                caller,
                source.package.as_deref(),
                source.uid.div_euclid(PER_USER_RANGE),
            )?
            || !valid_package(
                sys,
                caller,
                proxied_package,
                proxied_uid.div_euclid(PER_USER_RANGE),
            )?
        {
            return Ok(());
        }
        let Some(proxy_package) = resolve_package_name(source.uid, source.package.as_deref())
        else {
            return Ok(());
        };
        if !skip {
            self.finish_unchecked(
                client,
                code,
                source.uid,
                &proxy_package,
                source.tag.as_deref(),
            )?;
        }
        let Some(proxied) = resolve_package_name(proxied_uid, proxied_package) else {
            return Ok(());
        };
        self.finish_unchecked(client, code, proxied_uid, &proxied, proxied_tag)
    }

    /// `isOperationActive`: another uid's op only with WATCH_APPOPS.
    fn active(&self, code: i32, uid: i32, package: Option<&str>) -> Result<bool, Exception> {
        if self.caller != uid && !self.sys.holds(self.caller, WATCH_APPOPS)? {
            return Ok(false);
        }
        verify_incoming_op(self.sys, self.caller, code)?;
        if !valid_package(
            self.sys,
            self.caller,
            package,
            uid.div_euclid(PER_USER_RANGE),
        )? {
            return Ok(false);
        }
        let Some(resolved) = resolve_package_name(uid, package) else {
            return Ok(false);
        };
        Ok(self.started.running(&(uid, resolved, code)))
    }
}

/// What `startOperationUnchecked` and `startOperationDryRun` decide before
/// starting: that the package is not the uid's, that the uid has no ops,
/// or the op's mode and whether it is restricted; with the attribution tag
/// (none where it is not valid).
enum Decided<'a> {
    NotUids,
    NoOps(Option<&'a str>),
    Mode {
        tag: Option<&'a str>,
        package: &'a str,
        mode: i32,
        restricted: bool,
    },
}

fn decide<'a>(
    sys: &dyn System,
    caller: i32,
    op: &Unchecked<'a>,
    allow_uid: bool,
) -> Result<Decided<'a>, Exception> {
    let info = &APP_OP_INFOS[op.code as usize];
    let Verified::Bypass(bypass, tag_valid) =
        verify_and_get_bypass(sys, caller, op.uid, op.package, op.tag, op.proxy)?
    else {
        return Ok(Decided::NotUids);
    };
    let tag = op.tag.filter(|_| tag_valid);
    if !has_ops(sys, op.uid)? {
        return Ok(Decided::NoOps(tag));
    }
    let package = op
        .package
        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
    let restricted = restricted(sys, op.uid, info, bypass, package, tag, false)?;
    let mode = op_mode(sys, op.uid, package, info, allow_uid)?;
    Ok(Decided::Mode {
        tag,
        package,
        mode,
        restricted,
    })
}

/// `startOperationDryRun`: what a start would answer, without root and
/// system's own allowance, starting nothing.
fn start_dry_run(
    sys: &dyn System,
    caller: i32,
    op: &Unchecked<'_>,
    start_if_mode_default: bool,
) -> Result<SyncNoted, Exception> {
    Ok(match decide(sys, caller, op, false)? {
        Decided::NotUids => SyncNoted::new(MODE_ERRORED, op.code, op.tag, op.package),
        Decided::NoOps(tag) => SyncNoted::new(MODE_ERRORED, op.code, tag, op.package),
        Decided::Mode {
            tag, package, mode, ..
        } if !should_start(mode, start_if_mode_default) => {
            SyncNoted::new(mode, op.code, tag, Some(package))
        }
        Decided::Mode {
            tag,
            package,
            restricted,
            ..
        } => SyncNoted::new(
            if restricted {
                MODE_IGNORED
            } else {
                MODE_ALLOWED
            },
            op.code,
            tag,
            Some(package),
        ),
    })
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
        Ok(state
            .system
            .features
            .iter()
            .any(|(n, version)| n == name && *version >= 0))
    }

    fn user_restriction(&self, name: &str) -> Result<bool, Exception> {
        Ok(self.0.user_restrictions()?.contains(name))
    }

    fn client_restricted(
        &self,
        code: i32,
        user: i32,
        package: &str,
        tag: Option<&str>,
        is_check: bool,
    ) -> Result<bool, Exception> {
        self.0
            .app_ops
            .restrictions
            .lock()
            .unwrap()
            .0
            .any(code, user, package, tag, is_check)
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
            suspended: ps
                .users
                .get(&user)
                .is_some_and(|u| !u.suspended_by.is_empty()),
        }))
    }

    fn uid_has_package(&self, uid: i32) -> Result<bool, Exception> {
        let (state, _) = self.0.packages()?;
        Ok(uid.div_euclid(PER_USER_RANGE) == 0
            && state
                .packages
                .values()
                .any(|ps| !ps.is.apex && ps.app_id == app_id(uid)))
    }

    fn attribution_in_package(&self, name: &str, tag: Option<&str>) -> Result<bool, Exception> {
        let (state, _) = self.0.packages()?;
        let Some(pkg) = state.packages.get(name).and_then(|ps| ps.pkg.as_ref()) else {
            return Ok(false);
        };
        Ok(tag.is_none_or(|tag| {
            pkg.attributions
                .iter()
                .any(|a| a.tag.as_deref() == Some(tag))
        }))
    }

    /// `ensureSystemPackageName`: a factory package of user 0.
    fn system_package(&self, name: &str) -> Result<bool, Exception> {
        use crate::package::info::flags::MATCH_FACTORY_ONLY;
        let (state, resolution) = self.0.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        match query.package_info(name, -1, MATCH_FACTORY_ONLY, 0) {
            Ok(info) => info.map(|info| info.is_some()),
            Err(e) => Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("appops isSystemPackage: {}", e.0),
            )),
        }
    }

    /// `isChangeEnabledByPackageName` (the installed package's, else
    /// whether the change will be enabled for it) and
    /// `isChangeEnabledByUid`.
    fn invalid_tags_rejected(&self, package: &str, caller: i32) -> Result<bool, Exception> {
        let change = SECURITY_EXCEPTION_ON_INVALID_ATTRIBUTION_TAG_CHANGE;
        let inputs = self.0.inputs()?;
        let by_package = match inputs.packages.application_info(package, 0) {
            Some(ai) => inputs
                .compat
                .is_enabled(change, Some(package), ai.target_sdk_version),
            None => inputs.compat.will_be_enabled(change, package),
        };
        Ok(by_package && self.0.change_enabled(change, caller)?)
    }

    fn filtered(&self, package: &str, caller: i32, user: i32) -> Result<bool, Exception> {
        self.0.filter_package_access(package, caller, user)
    }

    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.0.check_permission(Some(permission), uid)? == 0)
    }

    fn uid_mode(&self, app_id: i32, op: &str) -> Result<Option<i32>, Exception> {
        let access = self.0.inputs()?.packages.access();
        let ops = access.and_then(|a| a.app_id_app_ops.iter().find(|(id, _)| *id == app_id));
        Ok(ops
            .and_then(|(_, ops)| ops.iter().find(|(n, _)| n == op))
            .map(|(_, mode)| *mode))
    }

    fn package_mode(&self, package: &str, op: &str) -> Result<Option<i32>, Exception> {
        let access = self.0.inputs()?.packages.access();
        let ops = access.and_then(|a| a.package_app_ops.iter().find(|(name, _)| name == package));
        Ok(ops
            .and_then(|(_, ops)| ops.iter().find(|(n, _)| n == op))
            .map(|(_, mode)| *mode))
    }

    fn running(&self, uid: i32) -> bool {
        self.0.uid_running(uid)
    }

    /// `updateActivityRecognizerTags` of each holder of
    /// `ROLE_SYSTEM_ACTIVITY_RECOGNIZER`: the tags its
    /// `ACTION_ACTIVITY_RECOGNIZER` service lists in its meta-data.
    fn activity_recognition_tags(&self) -> Result<Vec<(i32, String, Vec<String>)>, Exception> {
        const ROLE: &str = "android.app.role.SYSTEM_ACTIVITY_RECOGNIZER";
        const ACTION: &str = "android.intent.action.ACTIVITY_RECOGNIZER";
        const TAGS: &str = "android:activity_recognition_allow_listed_tags";
        // GET_SERVICES, GET_META_DATA, MATCH_DISABLED_UNTIL_USED_COMPONENTS,
        // MATCH_DIRECT_BOOT_AWARE and MATCH_DIRECT_BOOT_UNAWARE.
        const FLAGS: i64 = 0x4 | 0x80 | 0x8000 | 0x80000 | 0x40000;
        let roles = self
            .0
            .role_service
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "appops: no role service"))?;
        let (_, resolution) = self.0.packages()?;
        let mut tags = Vec::new();
        for recognizer in roles.holders_of(ROLE)? {
            let intent = crate::package::intent::Intent {
                action: Some(ACTION.into()),
                package: Some(recognizer.clone()),
                ..Default::default()
            };
            let resolved = resolution
                .resolve_service(&intent, None, FLAGS, 0, SYSTEM_UID)
                .map_err(|e| {
                    Exception::new(EX_ILLEGAL_STATE, format!("appops: resolveService: {e:?}"))
                })?;
            let Some(crate::package::component_resolver::Info::Service(si)) =
                resolved.map(|r| r.info)
            else {
                eprintln!(
                    "app host: appops: Service recognizer doesn't handle {ACTION}, ignoring!"
                );
                continue;
            };
            let list = si
                .info
                .item
                .meta_data
                .as_ref()
                .and_then(|m| m.0.iter().find(|(k, _)| k == TAGS))
                .and_then(|(_, v)| match v {
                    crate::package::pkg::Value::String(Some(s)) if !s.is_empty() => {
                        Some(s.split(';').map(str::to_owned).collect())
                    }
                    _ => None,
                });
            if let Some(list) = list {
                tags.push((
                    app_id(si.info.application_info.uid),
                    si.info.item.package_name.clone().unwrap_or_default(),
                    list,
                ));
            }
        }
        Ok(tags)
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.0.unimplemented_named("appops", what)
    }
}

/// A link to a binder's death: its reference and the cookie; none for a
/// binder of the app host's own, which does not die.
type Link = Option<(Strong, u64)>;

/// AppOpsService's state in the app host: the started ops and their
/// watchers, the async-noted watchers and the ops none took, the clients'
/// user restrictions with the binders their tokens keep, and the mode
/// watchers.
#[derive(Default)]
pub(super) struct State {
    started: Mutex<Started<Link>>,
    async_noted: Mutex<Async>,
    restrictions: Mutex<(UserRestrictions<Link>, HashMap<u32, Strong>)>,
    mode_watchers: Mutex<ModeWatchers<Link>>,
    /// `AudioRestrictionManager.mCameraAudioRestriction`.
    camera_audio_restriction: std::sync::atomic::AtomicI32,
}

/// Holds a reference to each token the user restrictions key, and to no
/// other (an `ArrayMap` key holds its binder).
fn keep_tokens(
    host: &AppHost,
    (restrictions, kept): &mut (UserRestrictions<Link>, HashMap<u32, Strong>),
) {
    let tokens = restrictions.tokens();
    kept.retain(|token, _| tokens.contains(token));
    for token in tokens {
        kept.entry(token)
            .or_insert_with(|| host.process.strong(token));
    }
}

/// `setUserRestrictionNoCheck`: the client made for the token (linked to
/// its death) where it has none, the restriction set for the user (each
/// user for `USER_ALL`, `resolveUserId`), the client dropped once it
/// restricts nothing; a change tells the op's mode watchers and pauses or
/// resumes its started ops.
fn set_user_restriction(
    host: &AppHost,
    code: i32,
    restricted: bool,
    token: Binder,
    user: i32,
    excluded: Option<&PackageTags>,
) -> Result<(), Exception> {
    let Binder::Handle(token) = token else {
        return Err(host.unimplemented_named(
            "appops",
            "a user restriction token of the app host's own (#1594)",
        ));
    };
    let users: Vec<i32> = if user == USER_ALL {
        host.packages()?.0.users.keys().copied().collect()
    } else {
        vec![user]
    };
    let (changed, dropped) = {
        let mut state = host.app_ops.restrictions.lock().unwrap();
        if !state.0.has_client(token) {
            state
                .0
                .add_client(token, link_restriction_client(host, token));
        }
        let changed = state.0.set(token, &users, code, restricted, excluded);
        let dropped = if state.0.has_restrictions(token) {
            None
        } else {
            state.0.remove_client(token)
        };
        keep_tokens(host, &mut state);
        (changed, dropped)
    };
    unlink(host, dropped.into_iter().collect());
    if changed {
        notify_mode_watchers(host, code);
        with_ops(host, SYSTEM_UID, |ops| {
            ops.update_started_mode(code, restricted, user)
        })?;
    }
    Ok(())
}

/// Links a restriction client's death to `ClientUserRestrictionState.binderDied`:
/// its restrictions cleared, the mode watchers of each op it restricted
/// told (`notifyAllUserRestrictions`).
fn link_restriction_client(host: &AppHost, token: u32) -> Link {
    let strong = host.process.strong(token);
    let me = host.me.clone();
    let death = host.process.link_to_death(
        &strong,
        Box::new(move || {
            let Some(host) = me.upgrade() else { return };
            let (codes, link) = {
                let mut state = host.app_ops.restrictions.lock().unwrap();
                let codes = state.0.clear(token);
                let link = state.0.remove_client(token);
                keep_tokens(&host, &mut state);
                (codes, link)
            };
            unlink(&host, link.into_iter().collect());
            for code in codes {
                notify_mode_watchers(&host, code);
            }
        }),
    );
    Some((strong, death))
}

/// `notifyWatchersOnDefaultDevice(code, UID_ANY)`: `opChanged` to each
/// mode watcher of the op (a restricted-read op's only with MANAGE_APPOPS,
/// `shouldIgnoreCallback`), one-way.
fn notify_mode_watchers(host: &AppHost, code: i32) {
    let changes = host
        .app_ops
        .mode_watchers
        .lock()
        .unwrap()
        .changes(code, UID_ANY, UID_ANY);
    for change in changes {
        if APP_OP_INFOS[change.op as usize].restrict_read {
            match host.check_permission(Some(MANAGE_APPOPS), change.calling_uid) {
                Ok(0) => {}
                Ok(_) => continue,
                Err(e) => {
                    eprintln!("app host: appops mode watcher of op {}: {e:?}", change.op);
                    continue;
                }
            }
        }
        let mut p = Parcel::new();
        mode_callback::OpChanged {
            op: change.op,
            uid: UID_ANY,
            package_name: Some(String::new()),
            persistent_device_id: Some(PERSISTENT_DEVICE_ID_DEFAULT.into()),
        }
        .write(&mut p);
        // One-way; a watcher that died is not called.
        let _ = match change.binder {
            Binder::Handle(handle) => host
                .process
                .transact(handle, mode_callback::OP_CHANGED, &p, true)
                .map(drop),
            Binder::Local(ptr) => match host.process.local_service(ptr) {
                Some(service) => service
                    .transact(mode_callback::OP_CHANGED, &p, true)
                    .map(drop),
                None => Ok(()),
            },
        };
    }
}

/// Runs `f` over the app host's appops as `caller`, then unlinks the
/// events that ended and does what it left to do.
fn with_ops<T>(
    host: &AppHost,
    caller: i32,
    f: impl FnOnce(&mut Ops<'_, Link>) -> Result<T, Exception>,
) -> Result<T, Exception> {
    let sys = Host(host);
    let mut link = |client: &Client| link_client(host, client);
    let (result, effects, ended) = {
        let mut started = host.app_ops.started.lock().unwrap();
        let mut ops = Ops {
            sys: &sys,
            caller,
            started: &mut started,
            link: &mut link,
            effects: Effects::default(),
            ended: Vec::new(),
        };
        let result = f(&mut ops);
        (result, ops.effects, ops.ended)
    };
    unlink(host, ended);
    dispatch(host, caller, effects);
    result
}

fn unlink(host: &AppHost, links: Vec<Link>) {
    for (strong, death) in links.into_iter().flatten() {
        host.process.clear_death(&strong, death);
    }
}

/// Links `client`'s death to `AttributedOp.onClientDeath`.
fn link_client(host: &AppHost, client: &Client) -> Link {
    let Binder::Handle(handle) = client.binder else {
        return None;
    };
    let strong = host.process.strong(handle);
    let (me, client) = (host.me.clone(), client.clone());
    let death = host.process.link_to_death(
        &strong,
        Box::new(move || {
            let Some(host) = me.upgrade() else { return };
            let mut effects = Effects::default();
            let ended = host
                .app_ops
                .started
                .lock()
                .unwrap()
                .client_died(&client, &mut effects);
            unlink(&host, ended);
            dispatch(&host, SYSTEM_UID, effects);
        }),
    );
    Some((strong, death))
}

/// Calls the watchers a note, start or finish told, as
/// `notifyOpActiveChanged` and `notifyOpStarted` do (a restricted-read
/// op's only with MANAGE_APPOPS), and collects the ops it left for their
/// apps as noted by `caller`.
fn dispatch(host: &AppHost, caller: i32, effects: Effects) {
    for notice in effects.notices {
        let mut p = Parcel::new();
        let (op, uid, package_name, attribution_tag) =
            (notice.op, notice.uid, notice.package, notice.tag);
        let (attribution_flags, attribution_chain_id) = (notice.attribution_flags, notice.chain_id);
        let code = match notice.heard {
            Heard::Active(active) => {
                let virtual_device_id = DEVICE_ID_DEFAULT;
                let call = active_callback::OpActiveChanged {
                    op,
                    uid,
                    package_name,
                    attribution_tag,
                    virtual_device_id,
                    active,
                    attribution_flags,
                    attribution_chain_id,
                };
                call.write(&mut p);
                active_callback::OP_ACTIVE_CHANGED
            }
            Heard::Started {
                flags,
                mode,
                start_type,
            } => {
                let call = started_callback::OpStarted {
                    op,
                    uid,
                    package_name,
                    attribution_tag,
                    virtual_device_id: DEVICE_ID_DEFAULT,
                    flags,
                    mode,
                    started_type: start_type,
                    attribution_flags,
                    attribution_chain_id,
                };
                call.write(&mut p);
                started_callback::OP_STARTED
            }
        };
        for (handle, calling) in notice.watchers {
            // `shouldIgnoreCallback`.
            if APP_OP_INFOS[op as usize].restrict_read {
                match host.check_permission(Some(MANAGE_APPOPS), calling) {
                    Ok(0) => {}
                    Ok(_) => continue,
                    Err(e) => {
                        eprintln!("app host: appops watcher of op {op}: {e:?}");
                        continue;
                    }
                }
            }
            // One-way; a watcher that died is not called (its
            // RemoteException is dropped).
            let _ = host.process.transact(handle, code, &p, true);
        }
    }
    for collected in effects.collected {
        collect(host, collected, caller);
    }
}

/// `checkOperation` as the system server asks it.
pub(super) fn mode(
    host: &AppHost,
    code: i32,
    uid: i32,
    package: Option<&str>,
) -> Result<i32, Exception> {
    check_from(host, SYSTEM_UID, code, uid, package)
}

/// `checkOperation` from `caller`.
pub(super) fn check_from(
    host: &AppHost,
    caller: i32,
    code: i32,
    uid: i32,
    package: Option<&str>,
) -> Result<i32, Exception> {
    check(
        &Host(host),
        caller,
        code,
        uid,
        package,
        DEVICE_ID_DEFAULT,
        false,
    )
}

/// `noteOperation` as the system server asks it.
pub(super) fn note(
    host: &AppHost,
    code: i32,
    uid: i32,
    package: Option<&str>,
) -> Result<i32, Exception> {
    note_from(host, SYSTEM_UID, code, uid, package, None)
}

/// The ops the system server's own `AppOpsManager` notes or starts for an
/// app are not collected for it (`getNotedOpCollectionMode`, #1592).
const NOT_COLLECTED: Option<String> = None;

/// `noteOperation` of `uid`'s package and attribution tag from `caller`.
pub(super) fn note_from(
    host: &AppHost,
    caller: i32,
    code: i32,
    uid: i32,
    package: Option<&str>,
    tag: Option<&str>,
) -> Result<i32, Exception> {
    let op = Unchecked {
        code,
        uid,
        package,
        tag,
        proxy: None,
        proxy_tag: None,
        flags: OP_FLAG_SELF,
        collect: NOT_COLLECTED,
    };
    with_ops(host, caller, |ops| ops.note(&op, DEVICE_ID_DEFAULT)).map(|noted| noted.mode)
}

/// `noteProxyOperation` of `source` and its next from `caller`.
pub(super) fn note_proxy(
    host: &AppHost,
    caller: i32,
    code: i32,
    source: &AttributionSourceState,
    skip_proxy: bool,
    message: Option<&str>,
) -> Result<i32, Exception> {
    with_ops(host, caller, |ops| {
        ops.note_proxy(code, source, skip_proxy, false, message)
    })
    .map(|noted| noted.mode)
}

/// `startOperation` of `uid`'s package and attribution tag by `client`
/// from `caller`.
#[allow(clippy::too_many_arguments)]
pub(super) fn start(
    host: &AppHost,
    caller: i32,
    client: Option<Binder>,
    code: i32,
    uid: i32,
    package: Option<&str>,
    tag: Option<&str>,
    attribution: Attribution,
) -> Result<i32, Exception> {
    let op = Unchecked {
        code,
        uid,
        package,
        tag,
        proxy: None,
        proxy_tag: None,
        flags: OP_FLAG_SELF,
        collect: NOT_COLLECTED,
    };
    with_ops(host, caller, |ops| ops.start(client, &op, attribution)).map(|started| started.mode)
}

/// `startProxyOperation` of `source` and its next by `client` from
/// `caller`.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_proxy(
    host: &AppHost,
    caller: i32,
    client: Option<Binder>,
    code: i32,
    source: &AttributionSourceState,
    skip_proxy: bool,
    attribution: ProxyAttribution,
    message: Option<&str>,
) -> Result<i32, Exception> {
    with_ops(host, caller, |ops| {
        ops.start_proxy(
            client,
            code,
            source,
            skip_proxy,
            attribution,
            false,
            message,
        )
    })
    .map(|started| started.mode)
}

/// `finishOperation` of `uid`'s package and attribution tag by `client`
/// from `caller`.
pub(super) fn finish(
    host: &AppHost,
    caller: i32,
    client: Option<Binder>,
    code: i32,
    uid: i32,
    package: Option<&str>,
    tag: Option<&str>,
) -> Result<(), Exception> {
    with_ops(host, caller, |ops| {
        ops.finish(client, code, uid, package, tag)
    })
}

/// `finishProxyOperation` of `source` and its next by `client` from
/// `caller`.
pub(super) fn finish_proxy(
    host: &AppHost,
    caller: i32,
    client: Option<Binder>,
    code: i32,
    source: &AttributionSourceState,
    skip_proxy: bool,
) -> Result<(), Exception> {
    with_ops(host, caller, |ops| {
        ops.finish_proxy(client, code, source, skip_proxy)
    })
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
    let aim_service_aidl::ParcelValue::Custom { kind: 4, bytes } = key else {
        return None;
    };
    let mut r = aim_binder_host::parcel::Reader::new(bytes, &[]);
    (r.read_string16().ok()?.as_deref() == Some("android.app.AppOpsManager$NotedOp"))
        .then_some(())?;
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
    let owns = match package {
        Some(package) => host.app_ops_owns_package(uid, package)?,
        None => false,
    };
    Ok(if owns { MODE_ALLOWED } else { MODE_ERRORED })
}

const GET_APP_OPS_STATS: &str = "android.permission.GET_APP_OPS_STATS";
/// `AppOpsService.NUM_ATTRIBUTED_OP_ENTRY_THRESHOLD`.
const NUM_ATTRIBUTED_OP_ENTRY_THRESHOLD: usize = 2000;

/// `collectOps` of `package`'s ops of `uid`: all of them by code, or those
/// of `ops` it has in their order (none if it has none of them); an op
/// that restricts reading only with `read_restricted`; each with the
/// package's mode and, for the default device, its attributions' entries;
/// cut off once the attributions' entries pass the threshold.
#[allow(clippy::too_many_arguments)]
fn collect_ops<L>(
    started: &Started<L>,
    sys: &dyn System,
    uid: i32,
    package: &str,
    ops: Option<&[i32]>,
    default_device: bool,
    read_restricted: bool,
) -> Result<Option<Vec<OpEntry>>, Exception> {
    let mut mine: Vec<i32> = started
        .ops
        .keys()
        .filter(|(u, p, _)| *u == uid && p == package)
        .map(|(_, _, op)| *op)
        .collect();
    mine.sort_unstable();
    let (mut result, codes) = match ops {
        None => (Some(Vec::new()), mine),
        Some(ops) => (
            None,
            ops.iter().copied().filter(|op| mine.contains(op)).collect(),
        ),
    };
    let mut total = 0;
    for code in codes {
        let entries = result.get_or_insert_with(Vec::new);
        let info = &APP_OP_INFOS[code as usize];
        if info.restrict_read && !read_restricted {
            continue;
        }
        if total > NUM_ATTRIBUTED_OP_ENTRY_THRESHOLD {
            break;
        }
        let mode = match sys.package_mode(package, info.name)? {
            Some(mode) => mode,
            None => default_mode(sys, info)?,
        };
        let mut entry = started.entry(&(uid, package.to_string(), code), mode);
        if !default_device {
            entry.attributed.clear();
        }
        total += entry.attributed.len();
        entries.push(entry);
    }
    if total > NUM_ATTRIBUTED_OP_ENTRY_THRESHOLD {
        eprintln!(
            "app host: appops: The number of attributed op entries has exceeded the threshold. This could be due to DoS attack from malicious apps. The result is throttled."
        );
    }
    Ok(result)
}

/// `getPackagesForOpsForDevice`: each uid's packages that have ops (the
/// caller's own, or all with GET_APP_OPS_STATS), in `SparseArray` and
/// `ArrayMap` order.
fn packages_for_ops<L>(
    started: &Started<L>,
    sys: &dyn System,
    caller: i32,
    ops: Option<&[i32]>,
    device: &str,
) -> Result<Option<Vec<PackageOps>>, Exception> {
    let all = sys.holds(caller, GET_APP_OPS_STATS)?;
    let mut packages: Vec<(i32, String)> = started
        .ops
        .keys()
        .filter(|(uid, _, _)| all || *uid == caller)
        .map(|(uid, package, _)| (*uid, package.clone()))
        .collect();
    packages.sort_by(|(a, p), (b, q)| {
        (a, crate::package::info::java_hash(p), p).cmp(&(b, crate::package::info::java_hash(q), q))
    });
    packages.dedup();
    let mut result: Option<Vec<PackageOps>> = None;
    for (uid, package) in packages {
        let default_device = device == PERSISTENT_DEVICE_ID_DEFAULT;
        if let Some(entries) = collect_ops(started, sys, uid, &package, ops, default_device, all)? {
            result.get_or_insert_with(Vec::new).push(PackageOps {
                package: Some(package),
                uid,
                entries,
            });
        }
    }
    Ok(result)
}

/// `OpEntry.getLastProxyInfo` of proxied accesses: the proxy of the latest
/// access noted for the app by another (`OP_FLAG_TRUSTED_PROXIED`,
/// `OP_FLAG_UNTRUSTED_PROXIED`), of any attribution and uid state.
fn last_proxy(entry: &OpEntry) -> Option<&ProxyInfo> {
    const KEY_FLAGS: i64 = 0x7fff_ffff;
    let proxied = (OP_FLAG_TRUSTED_PROXIED | OP_FLAG_UNTRUSTED_PROXIED) as i64;
    entry
        .attributed
        .iter()
        .filter_map(|(_, a)| a.access.as_ref())
        .flat_map(|access| access.iter())
        .filter(|(key, _)| *key & KEY_FLAGS & proxied != 0)
        .max_by_key(|(_, event)| event.time)
        .and_then(|(_, event)| event.proxy.as_ref())
}

/// `collectUidOps`: the uid's modes that are not their op's default, by
/// code, or of `ops` in their order.
fn collect_uid_ops(
    sys: &dyn System,
    uid: i32,
    ops: Option<&[i32]>,
) -> Result<Option<Vec<OpEntry>>, Exception> {
    let mut modes = Vec::new();
    for info in APP_OP_INFOS.iter().filter(|info| info.code != OP_NONE) {
        if let Some(mode) = sys.uid_mode(app_id(uid), info.name)?
            && mode != default_mode(sys, info)?
        {
            modes.push((info.code, mode));
        }
    }
    if modes.is_empty() {
        return Ok(None);
    }
    let entry = |(op, mode): (i32, i32)| OpEntry {
        op,
        mode,
        attributed: Vec::new(),
    };
    Ok(match ops {
        None => Some(modes.into_iter().map(entry).collect()),
        Some(ops) => {
            let found: Vec<OpEntry> = ops
                .iter()
                .filter_map(|op| modes.iter().find(|(code, _)| code == op).copied())
                .map(entry)
                .collect();
            (!found.is_empty()).then_some(found)
        }
    })
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
        p.write_i32(if self.attribution_tag.is_some() {
            0x4
        } else {
            0
        });
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

/// `collectAsyncNotedOp`: the op noted for an app by `noting_uid`, to its
/// watchers, else kept for `extractAsyncOps`.
fn collect(host: &AppHost, collected: Collected, noting_uid: i32) {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let Collected {
        uid,
        package,
        op,
        tag,
        message,
    } = collected;
    let noted = AsyncNoted {
        op,
        noting_uid,
        attribution_tag: tag,
        message,
        time,
    };
    let key = (package, uid);
    let watchers: Vec<u32> = match host.app_ops.async_noted.lock().unwrap().watchers.get(&key) {
        Some(watchers) => watchers.iter().map(|(w, _)| w.handle).collect(),
        None => return,
    };
    let mut p = Parcel::new();
    p.write_interface_token(ASYNC_NOTED_DESCRIPTOR);
    p.write_i32(1);
    noted.write_to(&mut p);
    let forwarded = watchers.iter().fold(false, |sent, &w| {
        host.process.transact(w, OP_NOTED, &p, true).is_ok() || sent
    });
    if !forwarded {
        let mut state = host.app_ops.async_noted.lock().unwrap();
        let unforwarded = state.unforwarded.entry(key).or_default();
        unforwarded.push(noted);
        if unforwarded.len() > MAX_UNFORWARDED_OPS {
            unforwarded.remove(0);
        }
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

pub struct AppOps {
    host: Arc<AppHost>,
    permissions: Arc<PermissionManager>,
    sampling: Mutex<super::appops_sampling::Sampling>,
}

impl AppOps {
    pub fn new(host: Arc<AppHost>, permissions: Arc<PermissionManager>) -> Self {
        AppOps {
            host,
            permissions,
            sampling: Mutex::new(super::appops_sampling::Sampling::new()),
        }
    }

    /// `getPackageNamesForSampling`: the packages of user 0 that request a
    /// dangerous permission (`isSamplingTarget`).
    fn sampling_targets(&self) -> Vec<String> {
        let Ok((state, _)) = self.host.packages() else {
            return Vec::new();
        };
        let mut names: Vec<String> = state
            .packages
            .values()
            .filter(|ps| ps.users.get(&0).is_none_or(|u| u.installed))
            .filter_map(|ps| ps.pkg.as_ref())
            .filter(|pkg| {
                pkg.requested_permissions.iter().any(|p| {
                    self.permissions
                        .protection_level(p)
                        .ok()
                        .flatten()
                        .is_some_and(|level| level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS)
                })
            })
            .map(|pkg| pkg.package_name.clone())
            .collect();
        names.sort();
        names
    }

    /// `startWatchingModeWithFlags`: a valid op or OP_NONE; a callback,
    /// kept until it dies if another process's; a package the caller may
    /// see.
    fn start_watching_mode(
        &self,
        uid: i32,
        pid: i32,
        op: i32,
        package: Option<&str>,
        flags: i32,
        callback: Option<Binder>,
    ) -> Result<(), Exception> {
        if !(OP_NONE..NUM_OP).contains(&op) {
            let side = if op < OP_NONE { "too low" } else { "too high" };
            return Err(Exception::illegal_argument(format!(
                "Invalid op code: {op} is out of range of [{OP_NONE}, {}] ({side})",
                NUM_OP - 1
            )));
        }
        let Some(callback) = callback else {
            return Ok(());
        };
        // `filterAppAccessUnlocked`.
        let package = match package {
            Some(package)
                if !Host(&self.host).filtered(package, uid, uid.div_euclid(PER_USER_RANGE))? =>
            {
                Some(package)
            }
            _ => None,
        };
        let host = &self.host;
        let link = || {
            let Binder::Handle(handle) = callback else {
                return None;
            };
            let strong = host.process.strong(handle);
            let me = host.me.clone();
            let death = host.process.link_to_death(
                &strong,
                Box::new(move || {
                    // `ModeCallback.binderDied`: `stopWatchingMode`.
                    let Some(host) = me.upgrade() else { return };
                    let link = host.app_ops.mode_watchers.lock().unwrap().unwatch(callback);
                    unlink(&host, link.into_iter().collect());
                }),
            );
            Some((strong, death))
        };
        host.app_ops
            .mode_watchers
            .lock()
            .unwrap()
            .watch(callback, op, package, flags, uid, pid, link);
        Ok(())
    }

    /// `setUserRestriction`: MANAGE_APP_OPS_RESTRICTIONS (the caller being
    /// another process), INTERACT_ACROSS_USERS(_FULL) for another user, a
    /// valid op the caller may read and a token.
    fn set_user_restriction(
        &self,
        uid: i32,
        a: appops::SetUserRestriction<PackageTags>,
    ) -> Result<(), Exception> {
        let host = &self.host;
        if host.check_permission(Some(MANAGE_APP_OPS_RESTRICTIONS), uid)? != 0 {
            return Err(Exception::security(format!(
                "uid {uid} does not have {MANAGE_APP_OPS_RESTRICTIONS}."
            )));
        }
        if a.user_handle != uid.div_euclid(PER_USER_RANGE)
            && host.check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)? != 0
            && host.check_permission(Some(INTERACT_ACROSS_USERS), uid)? != 0
        {
            return Err(Exception::security(
                "Need INTERACT_ACROSS_USERS_FULL or INTERACT_ACROSS_USERS to interact cross user ",
            ));
        }
        verify_incoming_op(&Host(host), uid, a.code)?;
        let token = a
            .token
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "token"))?;
        set_user_restriction(
            host,
            a.code,
            a.restricted,
            token,
            a.user_handle,
            a.excluded_package_tags.as_ref(),
        )
    }

    /// `startWatchingActive` and `startWatchingStarted`: the caller's own
    /// uid's ops watched, unless it holds WATCH_APPOPS; a watcher goes at
    /// its death.
    fn start_watching(
        &self,
        which: Watch,
        uid: i32,
        ops: Option<Vec<i32>>,
        callback: Option<Binder>,
    ) -> Result<(), Exception> {
        let watched = if self.host.check_permission(Some(WATCH_APPOPS), uid)? == 0 {
            INVALID_UID
        } else {
            uid
        };
        let (ops, callback) = match which {
            Watch::Active => {
                ops_in_range(ops.as_deref().unwrap_or_default())?;
                let Some(callback) = callback else {
                    return Ok(());
                };
                (
                    ops.ok_or_else(|| Exception::new(EX_NULL_POINTER, "ops"))?,
                    callback,
                )
            }
            Watch::Started => {
                let ops = ops
                    .filter(|ops| !ops.is_empty())
                    .ok_or_else(|| Exception::illegal_argument("Ops cannot be null or empty"))?;
                ops_in_range(&ops)?;
                (
                    ops,
                    callback.ok_or_else(|| {
                        Exception::new(EX_NULL_POINTER, "Callback cannot be null")
                    })?,
                )
            }
        };
        let Binder::Handle(handle) = callback else {
            return Err(Exception::illegal_argument(
                "startWatching: a local callback",
            ));
        };
        let host = &self.host;
        let link = || {
            let strong = host.process.strong(handle);
            let me = host.me.clone();
            let death = host.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(host) = me.upgrade() {
                        host.app_ops.started.lock().unwrap().unwatch(which, handle);
                    }
                }),
            );
            Some((strong, death))
        };
        host.app_ops
            .started
            .lock()
            .unwrap()
            .watch(which, handle, &ops, watched, uid, link);
        Ok(())
    }

    /// `stopWatchingActive` and `stopWatchingStarted`.
    fn stop_watching(&self, which: Watch, callback: Binder) {
        if let Binder::Handle(handle) = callback {
            let link = self
                .host
                .app_ops
                .started
                .lock()
                .unwrap()
                .unwatch(which, handle);
            unlink(&self.host, link.into_iter().collect());
        }
    }

    /// `verifyAndGetBypass`'s package check.
    fn verify(&self, uid: i32, package: &str) -> Result<(), Exception> {
        match check_package(&self.host, uid, Some(package))? {
            MODE_ALLOWED => Ok(()),
            _ => Err(Exception::security(format!(
                "Specified package \"{package}\" under uid {uid} but it is not"
            ))),
        }
    }

    /// `startWatchingAsyncNoted`: a watcher of the caller's `package`,
    /// gone at its death.
    fn start_watching_async(
        &self,
        uid: i32,
        package: String,
        callback: Binder,
    ) -> Result<(), Exception> {
        self.verify(uid, &package)?;
        let Binder::Handle(handle) = callback else {
            return Err(Exception::illegal_argument(
                "startWatchingAsyncNoted: a local callback",
            ));
        };
        let strong = self.host.process.strong(handle);
        let me = self.host.me.clone();
        let key = (package, uid);
        let dead = key.clone();
        let death = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(host) = me.upgrade() {
                    remove_watcher(&mut host.app_ops.async_noted.lock().unwrap(), &dead, handle);
                }
            }),
        );
        self.host
            .app_ops
            .async_noted
            .lock()
            .unwrap()
            .watchers
            .entry(key)
            .or_default()
            .push((strong, death));
        Ok(())
    }
}

impl Service for AppOps {
    fn descriptor(&self) -> &str {
        appops::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let caller = call.sender_euid as i32;
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
                match check(
                    &Host(&self.host),
                    caller,
                    args.code,
                    args.uid,
                    args.package_name.as_deref(),
                    DEVICE_ID_DEFAULT,
                    false,
                ) {
                    Ok(mode) => appops::write_check_operation_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            // `checkAudioOperationImpl`: the camera's audio restriction,
            // then the op's check; no zen mode restriction is set in the
            // app host (`setAudioRestriction`).
            appops::CHECK_AUDIO_OPERATION => {
                let args =
                    appops::CheckAudioOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let restriction = self
                    .host
                    .app_ops
                    .camera_audio_restriction
                    .load(std::sync::atomic::Ordering::Relaxed);
                if camera_audio_muted(restriction, args.code, args.usage) {
                    appops::write_check_audio_operation_reply(&mut p, MODE_IGNORED);
                    return Ok(p);
                }
                match check(
                    &Host(&self.host),
                    caller,
                    args.code,
                    args.uid,
                    args.package_name.as_deref(),
                    DEVICE_ID_DEFAULT,
                    false,
                ) {
                    Ok(mode) => appops::write_check_audio_operation_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::SET_CAMERA_AUDIO_RESTRICTION => {
                let args = appops::SetCameraAudioRestriction::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                // `enforceManageAppOpsModes(pid, uid, -1)`: no caller is the
                // app host itself (`Process.myPid()`), and with no target uid
                // a profile owner does not match; MANAGE_APP_OPS_MODES.
                match self
                    .host
                    .check_permission(Some(MANAGE_APP_OPS_MODES), caller)
                {
                    Ok(0) => {}
                    Ok(_) => {
                        return reply(Err(Exception::security(format!(
                            "Neither user {caller} nor current process has {MANAGE_APP_OPS_MODES}."
                        ))));
                    }
                    Err(e) => return reply(Err(e)),
                }
                self.host
                    .app_ops
                    .camera_audio_restriction
                    .store(args.mode, std::sync::atomic::Ordering::Relaxed);
                notify_mode_watchers(&self.host, OP_PLAY_AUDIO);
                notify_mode_watchers(&self.host, OP_VIBRATE);
                appops::write_set_camera_audio_restriction_reply(&mut p);
            }
            appops::CHECK_OPERATION_FOR_DEVICE => {
                let args =
                    appops::CheckOperationForDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = args.package_name.as_deref();
                match check(
                    &Host(&self.host),
                    caller,
                    args.code,
                    args.uid,
                    package,
                    args.virtual_device_id,
                    false,
                ) {
                    Ok(mode) => appops::write_check_operation_for_device_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::CHECK_OPERATION_RAW_FOR_DEVICE => {
                let args = appops::CheckOperationRawForDevice::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let package = args.package_name.as_deref();
                match check(
                    &Host(&self.host),
                    caller,
                    args.code,
                    args.uid,
                    package,
                    args.virtual_device_id,
                    true,
                ) {
                    Ok(mode) => appops::write_check_operation_raw_for_device_reply(&mut p, mode),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::NOTE_OPERATION => {
                let args = appops::NoteOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let op = Unchecked {
                    code: args.code,
                    uid: args.uid,
                    package: args.package_name.as_deref(),
                    tag: args.attribution_tag.as_deref(),
                    proxy: None,
                    proxy_tag: None,
                    flags: OP_FLAG_SELF,
                    collect: args
                        .should_collect_async_noted_op
                        .then(|| args.message.clone().unwrap_or_default()),
                };
                match with_ops(&self.host, caller, |ops| ops.note(&op, DEVICE_ID_DEFAULT)) {
                    Ok(noted) => appops::write_note_operation_reply(&mut p, Some(&noted)),
                    Err(e) => return reply(Err(e)),
                }
            }
            // One-way: each batched note as `noteOperation` notes it.
            appops::NOTE_OPERATIONS_IN_BATCH => {
                let args =
                    appops::NoteOperationsInBatch::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                for (key, _count) in args.batched_note_ops.unwrap_or_default() {
                    let noted = noted_op(&key).ok_or(BAD_VALUE)?;
                    let op = Unchecked {
                        code: noted.op,
                        uid: noted.uid,
                        package: noted.package.as_deref(),
                        tag: noted.attribution_tag.as_deref(),
                        proxy: None,
                        proxy_tag: None,
                        flags: OP_FLAG_SELF,
                        collect: noted
                            .should_collect_async_noted_op
                            .then(|| noted.message.clone().unwrap_or_default()),
                    };
                    if let Err(e) = with_ops(&self.host, caller, |ops| {
                        ops.note(&op, noted.virtual_device_id)
                    }) {
                        eprintln!("app host: appops noteOperationsInBatch: {e:?}");
                    }
                }
                return Ok(Parcel::new());
            }
            appops::NOTE_PROXY_OPERATION_WITH_STATE => {
                let args = appops::NoteProxyOperationWithState::<AttributionSourceState>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                let Some(source) = args.attribution_source_state_state.as_ref() else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "attributionSource")));
                };
                let (skip, collect, message) = (
                    args.skip_proxy_operation,
                    args.should_collect_async_noted_op,
                    args.message.as_deref(),
                );
                match with_ops(&self.host, caller, |ops| {
                    ops.note_proxy(args.code, source, skip, collect, message)
                }) {
                    Ok(noted) => {
                        appops::write_note_proxy_operation_with_state_reply(&mut p, Some(&noted))
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::START_OPERATION => {
                let args = appops::StartOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let op = Unchecked {
                    code: args.code,
                    uid: args.uid,
                    package: args.package_name.as_deref(),
                    tag: args.attribution_tag.as_deref(),
                    proxy: None,
                    proxy_tag: None,
                    flags: OP_FLAG_SELF,
                    collect: args
                        .should_collect_async_noted_op
                        .then(|| args.message.clone().unwrap_or_default()),
                };
                let attribution = Attribution {
                    start_if_mode_default: args.start_if_mode_default,
                    flags: args.attribution_flags,
                    chain_id: args.attribution_chain_id,
                };
                match with_ops(&self.host, caller, |ops| {
                    ops.start(args.client_id, &op, attribution)
                }) {
                    Ok(started) => appops::write_start_operation_reply(&mut p, Some(&started)),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::START_PROXY_OPERATION_WITH_STATE => {
                let args = appops::StartProxyOperationWithState::<AttributionSourceState>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                let Some(source) = args.attribution_source_state_state.as_ref() else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "attributionSource")));
                };
                let attribution = ProxyAttribution {
                    start_if_mode_default: args.start_if_mode_default,
                    proxy_flags: args.proxy_attribution_flags,
                    proxied_flags: args.proxied_attribution_flags,
                    chain_id: args.attribution_chain_id,
                };
                let (skip, collect, message) = (
                    args.skip_proxy_operation,
                    args.should_collect_async_noted_op,
                    args.message.as_deref(),
                );
                match with_ops(&self.host, caller, |ops| {
                    ops.start_proxy(
                        args.client_id,
                        args.code,
                        source,
                        skip,
                        attribution,
                        collect,
                        message,
                    )
                }) {
                    Ok(started) => {
                        appops::write_start_proxy_operation_with_state_reply(&mut p, Some(&started))
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::FINISH_OPERATION => {
                let args = appops::FinishOperation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (package, tag) = (
                    args.package_name.as_deref(),
                    args.attribution_tag.as_deref(),
                );
                match with_ops(&self.host, caller, |ops| {
                    ops.finish(args.client_id, args.code, args.uid, package, tag)
                }) {
                    Ok(()) => appops::write_finish_operation_reply(&mut p),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::FINISH_PROXY_OPERATION_WITH_STATE => {
                let args = appops::FinishProxyOperationWithState::<AttributionSourceState>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                let Some(source) = args.attribution_source_state_state.as_ref() else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "attributionSource")));
                };
                match with_ops(&self.host, caller, |ops| {
                    ops.finish_proxy(args.client_id, args.code, source, args.skip_proxy_operation)
                }) {
                    Ok(()) => appops::write_finish_proxy_operation_with_state_reply(&mut p),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::IS_OPERATION_ACTIVE => {
                let args =
                    appops::IsOperationActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match with_ops(&self.host, caller, |ops| {
                    ops.active(args.code, args.uid, args.package_name.as_deref())
                }) {
                    Ok(active) => appops::write_is_operation_active_reply(&mut p, active),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::SHOULD_COLLECT_NOTES => {
                let args =
                    appops::ShouldCollectNotes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let protection = |permission: &str| self.permissions.protection_level(permission);
                match should_collect_notes(&Host(&self.host), protection, args.op_code) {
                    Ok(collect) => appops::write_should_collect_notes_reply(&mut p, collect),
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::REPORT_RUNTIME_APP_OP_ACCESS_MESSAGE_AND_GET_CONFIG => {
                let args = appops::ReportRuntimeAppOpAccessMessageAndGetConfig::<
                    super::appops_sampling::Noted,
                >::read(&mut call.data)
                .map_err(|_| BAD_VALUE)?;
                let Some(package) = args.package_name else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "packageName")));
                };
                let config = self.sampling.lock().unwrap().report(
                    &package,
                    args.app_op.as_ref(),
                    args.message.as_deref(),
                    || self.sampling_targets(),
                );
                match config {
                    Ok(config) => {
                        appops::write_report_runtime_app_op_access_message_and_get_config_reply(
                            &mut p,
                            Some(&config),
                        )
                    }
                    Err(what) => return reply(Err(Exception::new(EX_NULL_POINTER, what))),
                }
            }
            appops::START_WATCHING_MODE => {
                let args =
                    appops::StartWatchingMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.start_watching_mode(
                    caller,
                    call.sender_pid,
                    args.op,
                    args.package_name.as_deref(),
                    0,
                    args.callback,
                ) {
                    return reply(Err(e));
                }
                appops::write_start_watching_mode_reply(&mut p);
            }
            appops::START_WATCHING_MODE_WITH_FLAGS => {
                let args = appops::StartWatchingModeWithFlags::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let package = args.package_name.as_deref();
                if let Err(e) = self.start_watching_mode(
                    caller,
                    call.sender_pid,
                    args.op,
                    package,
                    args.flags,
                    args.callback,
                ) {
                    return reply(Err(e));
                }
                appops::write_start_watching_mode_with_flags_reply(&mut p);
            }
            appops::STOP_WATCHING_MODE => {
                let args = appops::StopWatchingMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(callback) = args.callback {
                    let link = self
                        .host
                        .app_ops
                        .mode_watchers
                        .lock()
                        .unwrap()
                        .unwatch(callback);
                    unlink(&self.host, link.into_iter().collect());
                }
                appops::write_stop_watching_mode_reply(&mut p);
            }
            appops::SET_USER_RESTRICTION => {
                let args = appops::SetUserRestriction::<PackageTags>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.set_user_restriction(caller, args) {
                    return reply(Err(e));
                }
                appops::write_set_user_restriction_reply(&mut p);
            }
            appops::START_WATCHING_ACTIVE => {
                let args =
                    appops::StartWatchingActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.start_watching(Watch::Active, caller, args.ops, args.callback)
                {
                    return reply(Err(e));
                }
                appops::write_start_watching_active_reply(&mut p);
            }
            appops::STOP_WATCHING_ACTIVE => {
                let args =
                    appops::StopWatchingActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(callback) = args.callback {
                    self.stop_watching(Watch::Active, callback);
                }
                appops::write_stop_watching_active_reply(&mut p);
            }
            appops::START_WATCHING_STARTED => {
                let args =
                    appops::StartWatchingStarted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.start_watching(Watch::Started, caller, args.ops, args.callback)
                {
                    return reply(Err(e));
                }
                appops::write_start_watching_started_reply(&mut p);
            }
            appops::STOP_WATCHING_STARTED => {
                let args =
                    appops::StopWatchingStarted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(callback) = args.callback else {
                    return reply(Err(Exception::new(
                        EX_NULL_POINTER,
                        "Callback cannot be null",
                    )));
                };
                self.stop_watching(Watch::Started, callback);
                appops::write_stop_watching_started_reply(&mut p);
            }
            appops::START_WATCHING_ASYNC_NOTED => {
                let args =
                    appops::StartWatchingAsyncNoted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (Some(package), Some(callback)) = (args.package_name, args.callback) else {
                    return reply(Err(Exception::new(
                        EX_NULL_POINTER,
                        "packageName or callback",
                    )));
                };
                if let Err(e) = self.start_watching_async(caller, package, callback) {
                    return reply(Err(e));
                }
                appops::write_start_watching_async_noted_reply(&mut p);
            }
            appops::STOP_WATCHING_ASYNC_NOTED => {
                let args =
                    appops::StopWatchingAsyncNoted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (Some(package), Some(callback)) = (args.package_name, args.callback) else {
                    return reply(Err(Exception::new(
                        EX_NULL_POINTER,
                        "packageName or callback",
                    )));
                };
                if let Err(e) = self.verify(caller, &package) {
                    return reply(Err(e));
                }
                if let Binder::Handle(handle) = callback
                    && let Some((strong, death)) = remove_watcher(
                        &mut self.host.app_ops.async_noted.lock().unwrap(),
                        &(package, caller),
                        handle,
                    )
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
                if let Err(e) = self.verify(caller, &package) {
                    return reply(Err(e));
                }
                // Null when none waits.
                let ops = self
                    .host
                    .app_ops
                    .async_noted
                    .lock()
                    .unwrap()
                    .unforwarded
                    .remove(&(package, caller));
                let ops: Option<Vec<Option<AsyncNoted>>> =
                    ops.map(|ops| ops.into_iter().map(Some).collect());
                appops::write_extract_async_ops_reply(&mut p, ops.as_deref());
            }
            appops::GET_PACKAGES_FOR_OPS | appops::GET_PACKAGES_FOR_OPS_FOR_DEVICE => {
                let (ops, device) = if call.code == appops::GET_PACKAGES_FOR_OPS {
                    let args =
                        appops::GetPackagesForOps::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (args.ops, PERSISTENT_DEVICE_ID_DEFAULT.to_string())
                } else {
                    let args = appops::GetPackagesForOpsForDevice::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    let device = args
                        .persistent_device_id
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "persistentDeviceId"));
                    match device {
                        Ok(device) => (args.ops, device),
                        Err(e) => return reply(Err(e)),
                    }
                };
                let sys = Host(&self.host);
                let found = {
                    let started = self.host.app_ops.started.lock().unwrap();
                    packages_for_ops(&started, &sys, caller, ops.as_deref(), &device)
                };
                match found {
                    Ok(found) => {
                        let found: Option<Vec<Option<PackageOps>>> =
                            found.map(|f| f.into_iter().map(Some).collect());
                        appops::write_get_packages_for_ops_reply(&mut p, found.as_deref());
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::GET_OPS_FOR_PACKAGE => {
                let args = appops::GetOpsForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = args.package_name.as_deref();
                // `enforceGetAppOpsStatsPermissionIfNeeded`: a uid's own
                // package, else GET_APP_OPS_STATS.
                let own = args.uid == caller
                    && package.is_some()
                    && match check_package(&self.host, args.uid, package) {
                        Ok(mode) => mode == MODE_ALLOWED,
                        Err(e) => return reply(Err(e)),
                    };
                let sys = Host(&self.host);
                if !own {
                    match sys.holds(caller, GET_APP_OPS_STATS) {
                        Ok(true) => {}
                        Ok(false) => {
                            return reply(Err(Exception::security(format!(
                                "Neither user {caller} nor current process has {GET_APP_OPS_STATS}."
                            ))));
                        }
                        Err(e) => return reply(Err(e)),
                    }
                }
                let found: Result<Option<Vec<PackageOps>>, Exception> =
                    match resolve_package_name(args.uid, package) {
                        None => Ok(Some(Vec::new())),
                        Some(resolved) => {
                            let started = self.host.app_ops.started.lock().unwrap();
                            let read_restricted = sys.holds(caller, GET_APP_OPS_STATS);
                            read_restricted.and_then(|read_restricted| {
                                collect_ops(
                                    &started,
                                    &sys,
                                    args.uid,
                                    &resolved,
                                    args.ops.as_deref(),
                                    true,
                                    read_restricted,
                                )
                                .map(|entries| {
                                    entries.filter(|e| !e.is_empty()).map(|entries| {
                                        vec![PackageOps {
                                            package: Some(resolved),
                                            uid: args.uid,
                                            entries,
                                        }]
                                    })
                                })
                            })
                        }
                    };
                match found {
                    Ok(found) => {
                        let found: Option<Vec<Option<PackageOps>>> =
                            found.map(|f| f.into_iter().map(Some).collect());
                        appops::write_get_ops_for_package_reply(&mut p, found.as_deref());
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            appops::GET_UID_OPS => {
                let args = appops::GetUidOps::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let sys = Host(&self.host);
                let found = sys.holds(caller, GET_APP_OPS_STATS).and_then(|holds| {
                    if !holds {
                        return Err(Exception::security(format!(
                            "Neither user {caller} nor current process has {GET_APP_OPS_STATS}."
                        )));
                    }
                    // `getUidStateLocked(uid, false)`: a uid with ops.
                    if !has_ops(&sys, args.uid)? {
                        return Ok(None);
                    }
                    Ok(
                        collect_uid_ops(&sys, args.uid, args.ops.as_deref())?.map(|entries| {
                            vec![PackageOps {
                                package: None,
                                uid: args.uid,
                                entries,
                            }]
                        }),
                    )
                });
                match found {
                    Ok(found) => {
                        let found: Option<Vec<Option<PackageOps>>> =
                            found.map(|f| f.into_iter().map(Some).collect());
                        appops::write_get_uid_ops_reply(&mut p, found.as_deref());
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            // `isProxying`: whether the caller is the last proxy of the
            // proxied app's running op, asked as the system.
            appops::IS_PROXYING => {
                let args = appops::IsProxying::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (Some(proxy_package), Some(proxied_package)) =
                    (args.proxy_package_name, args.proxied_package_name)
                else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "packageName")));
                };
                let sys = Host(&self.host);
                let proxying = match resolve_package_name(args.proxied_uid, Some(&proxied_package))
                {
                    None => Ok(false),
                    Some(resolved) => {
                        let started = self.host.app_ops.started.lock().unwrap();
                        collect_ops(
                            &started,
                            &sys,
                            args.proxied_uid,
                            &resolved,
                            Some(&[args.op]),
                            true,
                            true,
                        )
                        .map(|entries| {
                            let Some(entry) = entries.as_ref().and_then(|e| e.first()) else {
                                return false;
                            };
                            entry.attributed.iter().any(|(_, a)| a.running)
                                && last_proxy(entry).is_some_and(|proxy| {
                                    proxy.uid == caller
                                        && proxy.package.as_deref() == Some(&proxy_package)
                                        && proxy.tag == args.proxy_attribution_tag
                                })
                        })
                    }
                };
                match proxying {
                    Ok(proxying) => appops::write_is_proxying_reply(&mut p, proxying),
                    Err(e) => return reply(Err(e)),
                }
            }
            // The historical registry is not kept.
            appops::GET_HISTORICAL_OPS | appops::GET_HISTORICAL_OPS_FROM_DISK_RAW => {
                let name = aim_service_aidl::method_name(self.descriptor(), call.code)
                    .unwrap_or("an ops query");
                return reply(Err(self.host.unimplemented_named(
                    "appops",
                    &format!("{name}: the historical ops (#2025)"),
                )));
            }
            code => {
                return reply(Err(self.host.unimplemented(
                    "appops",
                    self.descriptor(),
                    code,
                )));
            }
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

    /// `CAMERA_AUDIO_RESTRICTIONS`: vibration under any restriction, sound
    /// only under VIBRATION_SOUND, and only for notification, call and alarm
    /// usages.
    #[test]
    fn camera_audio_restriction_mutes_the_original_usages() {
        const VIBRATION: i32 = 1;
        const USAGE_MEDIA: i32 = 1;
        const USAGE_ALARM: i32 = 4;
        const USAGE_RINGTONE: i32 = 6;
        assert!(!camera_audio_muted(
            AUDIO_RESTRICTION_NONE,
            OP_VIBRATE,
            USAGE_ALARM
        ));
        assert!(camera_audio_muted(VIBRATION, OP_VIBRATE, USAGE_RINGTONE));
        assert!(!camera_audio_muted(
            VIBRATION,
            OP_PLAY_AUDIO,
            USAGE_RINGTONE
        ));
        assert!(camera_audio_muted(
            AUDIO_RESTRICTION_VIBRATION_SOUND,
            OP_PLAY_AUDIO,
            USAGE_ALARM
        ));
        assert!(!camera_audio_muted(
            AUDIO_RESTRICTION_VIBRATION_SOUND,
            OP_PLAY_AUDIO,
            USAGE_MEDIA
        ));
        assert!(!camera_audio_muted(
            AUDIO_RESTRICTION_VIBRATION_SOUND,
            OP_COARSE_LOCATION,
            USAGE_ALARM
        ));
    }
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
        attributions: BTreeMap<&'static str, Vec<&'static str>>,
        invalid_tags_rejected: bool,
        recognizer_tags: Vec<(i32, String, Vec<String>)>,
        clients: UserRestrictions<()>,
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

        fn client_restricted(
            &self,
            code: i32,
            user: i32,
            package: &str,
            tag: Option<&str>,
            is_check: bool,
        ) -> Result<bool, Exception> {
            self.clients.any(code, user, package, tag, is_check)
        }

        fn global_restriction(&self, op: i32) -> Result<bool, Exception> {
            Ok(self.global.contains(&op))
        }

        fn package(&self, name: &str, _: i32) -> Result<Option<Package>, Exception> {
            Ok(self.packages.get(name).copied())
        }

        fn uid_has_package(&self, uid: i32) -> Result<bool, Exception> {
            Ok(self.packages.values().any(|p| p.app_id == uid))
        }

        fn attribution_in_package(&self, name: &str, tag: Option<&str>) -> Result<bool, Exception> {
            let declared = self.attributions.get(name).cloned().unwrap_or_default();
            Ok(self.packages.contains_key(name) && tag.is_none_or(|tag| declared.contains(&tag)))
        }

        fn system_package(&self, name: &str) -> Result<bool, Exception> {
            Ok(self.packages.get(name).is_some_and(|p| p.privileged))
        }

        fn invalid_tags_rejected(&self, _: &str, _: i32) -> Result<bool, Exception> {
            Ok(self.invalid_tags_rejected)
        }

        fn filtered(&self, package: &str, _: i32, _: i32) -> Result<bool, Exception> {
            Ok(package == "hidden")
        }

        fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
            Ok(matches!(app_id(uid), ROOT_UID | SYSTEM_UID)
                || self
                    .granted
                    .iter()
                    .any(|(u, p)| *u == uid && *p == permission))
        }

        fn uid_mode(&self, app_id: i32, op: &str) -> Result<Option<i32>, Exception> {
            Ok(self
                .uid_modes
                .iter()
                .find(|((a, o), _)| *a == app_id && *o == op)
                .map(|(_, m)| *m))
        }

        fn package_mode(&self, package: &str, op: &str) -> Result<Option<i32>, Exception> {
            Ok(self
                .package_modes
                .iter()
                .find(|((p, o), _)| *p == package && *o == op)
                .map(|(_, m)| *m))
        }

        fn running(&self, uid: i32) -> bool {
            self.running.contains(&uid)
        }

        fn activity_recognition_tags(&self) -> Result<Vec<(i32, String, Vec<String>)>, Exception> {
            Ok(self.recognizer_tags.clone())
        }

        fn unimplemented(&self, what: &str) -> Exception {
            Exception::new(EX_UNSUPPORTED_OPERATION, what)
        }
    }

    fn package(app_id: i32, privileged: bool) -> Package {
        Package {
            app_id,
            parsed: true,
            privileged,
            suspended: false,
        }
    }

    fn fake() -> Fake {
        let mut f = Fake {
            flags: [APPOP_MODE_CACHING].into(),
            ..Fake::default()
        };
        f.packages.insert("app", package(APP, false));
        f.packages.insert("priv", package(PRIV_APP, true));
        f.packages.insert("android", package(SYSTEM_UID, true));
        f.packages.insert("hidden", package(10_102, false));
        f
    }

    fn check_mode(f: &Fake, code: i32, uid: i32, package: &str) -> i32 {
        check(f, uid, code, uid, Some(package), DEVICE_ID_DEFAULT, false).unwrap()
    }

    /// Runs `op` over `f`'s appops as `caller` with `started`, each client
    /// linked as itself: its result, what it left to do and the clients
    /// whose events ended.
    fn run<T>(
        f: &Fake,
        caller: i32,
        started: &mut Started<Binder>,
        op: impl FnOnce(&mut Ops<'_, Binder>) -> Result<T, Exception>,
    ) -> (Result<T, Exception>, Effects, Vec<Binder>) {
        let mut link = |client: &Client| client.binder;
        let mut ops = Ops {
            sys: f,
            caller,
            started,
            link: &mut link,
            effects: Effects::default(),
            ended: Vec::new(),
        };
        let result = op(&mut ops);
        (result, ops.effects, ops.ended)
    }

    fn own(code: i32, uid: i32, package: &str) -> Unchecked<'_> {
        Unchecked {
            code,
            uid,
            package: Some(package),
            tag: None,
            proxy: None,
            proxy_tag: None,
            flags: OP_FLAG_SELF,
            collect: None,
        }
    }

    fn note_with(
        f: &Fake,
        caller: i32,
        code: i32,
        uid: i32,
        package: &str,
    ) -> Result<SyncNoted, Exception> {
        run(f, caller, &mut Started::default(), |ops| {
            ops.note(&own(code, uid, package), DEVICE_ID_DEFAULT)
        })
        .0
    }

    fn note_mode(f: &Fake, caller: i32, code: i32, uid: i32, package: &str) -> i32 {
        note_with(f, caller, code, uid, package).unwrap().mode
    }

    #[test]
    fn modes_are_the_switchs() {
        let mut f = fake();
        assert_eq!(check_mode(&f, OP_GPS, APP, "app"), MODE_ALLOWED);
        // GPS and WIFI_SCAN switch to COARSE_LOCATION: its mode decides.
        f.uid_modes
            .insert((APP, "android:coarse_location"), MODE_IGNORED);
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
        f.uid_modes
            .insert((APP, "android:legacy_storage"), MODE_DEFAULT);
        f.package_modes
            .insert(("app", "android:legacy_storage"), MODE_ALLOWED);
        assert_eq!(check_mode(&f, OP_LEGACY_STORAGE, APP, "app"), MODE_ALLOWED);
        f.uid_modes
            .insert((APP, "android:legacy_storage"), MODE_ERRORED);
        assert_eq!(check_mode(&f, OP_LEGACY_STORAGE, APP, "app"), MODE_ERRORED);
    }

    #[test]
    fn foreground_modes_by_the_uids_process() {
        let mut f = fake();
        assert_eq!(
            check_mode(&f, OP_TAKE_AUDIO_FOCUS, APP, "app"),
            MODE_IGNORED
        );
        assert_eq!(
            check(
                &f,
                APP,
                OP_TAKE_AUDIO_FOCUS,
                APP,
                Some("app"),
                DEVICE_ID_DEFAULT,
                true
            )
            .unwrap(),
            MODE_FOREGROUND
        );
        f.running.insert(APP);
        assert_eq!(
            check_mode(&f, OP_TAKE_AUDIO_FOCUS, APP, "app"),
            MODE_ALLOWED
        );
    }

    #[test]
    fn user_restrictions_and_their_bypass() {
        let mut f = fake();
        f.restrictions.insert("no_share_location");
        // COARSE_LOCATION lets the system uid bypass it, WIFI_SCAN a
        // privileged app; GPS lets none.
        assert_eq!(check_mode(&f, OP_COARSE_LOCATION, APP, "app"), MODE_IGNORED);
        assert_eq!(
            check_mode(&f, OP_COARSE_LOCATION, PRIV_APP, "priv"),
            MODE_IGNORED
        );
        assert_eq!(
            check_mode(&f, OP_COARSE_LOCATION, SYSTEM_UID, "android"),
            MODE_ALLOWED
        );
        assert_eq!(check_mode(&f, OP_WIFI_SCAN, PRIV_APP, "priv"), MODE_ALLOWED);
        assert_eq!(check_mode(&f, OP_WIFI_SCAN, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_GPS, SYSTEM_UID, "android"), MODE_IGNORED);
        // A non-app uid's own name is unrestricted; root bypasses nothing.
        assert_eq!(
            check_mode(&f, OP_WIFI_SCAN, MEDIA_UID, "media"),
            MODE_ALLOWED
        );
        assert_eq!(
            check(
                &f,
                ROOT_UID,
                OP_WIFI_SCAN,
                ROOT_UID,
                None,
                DEVICE_ID_DEFAULT,
                false
            )
            .unwrap(),
            MODE_IGNORED
        );
        assert_eq!(note_mode(&f, APP, OP_WIFI_SCAN, APP, "app"), MODE_IGNORED);
        // RECORD_AUDIO by EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS.
        f.restrictions.insert("no_record_audio");
        assert_eq!(check_mode(&f, OP_RECORD_AUDIO, APP, "app"), MODE_IGNORED);
        f.granted
            .insert((APP, EXEMPT_FROM_AUDIO_RECORD_RESTRICTIONS));
        assert_eq!(check_mode(&f, OP_RECORD_AUDIO, APP, "app"), MODE_ALLOWED);
    }

    #[test]
    fn global_restrictions_and_suspension() {
        let mut f = fake();
        f.global.insert(OP_CAMERA);
        assert_eq!(
            check_mode(&f, OP_CAMERA, SYSTEM_UID, "android"),
            MODE_IGNORED
        );
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
        f.package_modes
            .insert(("android", "android:read_clipboard"), MODE_IGNORED);
        assert_eq!(
            check_mode(&f, OP_READ_CLIPBOARD, SYSTEM_UID, "android"),
            MODE_IGNORED
        );
        f.flags.insert(RUNTIME_PERMISSION_APPOPS_MAPPING);
        assert_eq!(
            check_mode(&f, OP_READ_CLIPBOARD, SYSTEM_UID, "android"),
            MODE_ALLOWED
        );
        assert_eq!(
            check(
                &f,
                SYSTEM_UID,
                OP_READ_CLIPBOARD,
                SYSTEM_UID,
                None,
                DEVICE_ID_DEFAULT,
                false
            )
            .unwrap(),
            MODE_ALLOWED
        );
    }

    #[test]
    fn packages_not_the_uids_or_not_visible() {
        let mut f = fake();
        // Without check_op_validate_package: a check of a package the
        // caller may not see answers the op's default, a note errs.
        assert_eq!(
            check(
                &f,
                APP,
                OP_LEGACY_STORAGE,
                APP,
                Some("hidden"),
                DEVICE_ID_DEFAULT,
                false
            )
            .unwrap(),
            MODE_DEFAULT
        );
        assert_eq!(
            note_mode(&f, APP, OP_READ_CLIPBOARD, APP, "hidden"),
            MODE_ERRORED
        );
        // Another uid's package.
        assert_eq!(
            check(
                &f,
                SYSTEM_UID,
                OP_READ_CLIPBOARD,
                APP,
                Some("priv"),
                DEVICE_ID_DEFAULT,
                false
            )
            .unwrap(),
            MODE_IGNORED
        );
        assert_eq!(
            note_mode(&f, SYSTEM_UID, OP_READ_CLIPBOARD, APP, "priv"),
            MODE_ERRORED
        );
        // An app uid without a package.
        assert_eq!(
            check(
                &f,
                APP,
                OP_READ_CLIPBOARD,
                APP,
                None,
                DEVICE_ID_DEFAULT,
                false
            )
            .unwrap(),
            MODE_IGNORED
        );
        f.flags.insert(CHECK_OP_VALIDATE_PACKAGE);
        assert_eq!(
            check(
                &f,
                APP,
                OP_LEGACY_STORAGE,
                APP,
                Some("hidden"),
                DEVICE_ID_DEFAULT,
                false
            )
            .unwrap(),
            MODE_IGNORED
        );
        assert_eq!(
            note_mode(&f, APP, OP_READ_CLIPBOARD, APP, "hidden"),
            MODE_IGNORED
        );
        // A virtual device is not known.
        assert_eq!(
            check(&f, APP, OP_READ_CLIPBOARD, APP, Some("app"), 7, false).unwrap(),
            MODE_IGNORED
        );
    }

    #[test]
    fn ops_the_caller_may_not_ask() {
        let f = fake();
        let bad = check(&f, APP, NUM_OP, APP, Some("app"), DEVICE_ID_DEFAULT, false).unwrap_err();
        assert_eq!(bad.message, "Bad operation #163");
        assert!(
            check(
                &f,
                APP,
                OP_ACCESS_RESTRICTED_SETTINGS,
                APP,
                Some("app"),
                DEVICE_ID_DEFAULT,
                false
            )
            .is_err()
        );
        assert_eq!(
            check_mode(&f, OP_ACCESS_RESTRICTED_SETTINGS, SYSTEM_UID, "android"),
            MODE_DEFAULT
        );
        // Noting for another uid needs UPDATE_APP_OPS_STATS.
        assert!(note_with(&f, PRIV_APP, OP_READ_CLIPBOARD, APP, "app").is_err());
        assert_eq!(
            note_mode(&f, SYSTEM_UID, OP_READ_CLIPBOARD, APP, "app"),
            MODE_ALLOWED
        );
        // The deprecated op 96 has no switch.
        assert!(check(&f, APP, 96, APP, Some("app"), DEVICE_ID_DEFAULT, false).is_err());
    }

    /// A datasource's tag makes an activity recognition note its source
    /// op; any other tag, package or uid, and location (no provider tags in
    /// the app host), leave it.
    #[test]
    fn datasource_ops_of_notes() {
        let mut f = fake();
        f.recognizer_tags = vec![(PRIV_APP, "priv".into(), vec!["ar".into(), "ar2".into()])];
        let resolve = |f: &Fake, code, uid, package, tag| {
            resolve_datasource_op(f, code, uid, Some(package), tag).unwrap()
        };
        assert_eq!(
            resolve(&f, OP_RECORD_AUDIO_HOTWORD, APP, "app", None),
            OP_RECORD_AUDIO_HOTWORD
        );
        assert_eq!(
            resolve(&f, OP_COARSE_LOCATION, APP, "app", None),
            OP_COARSE_LOCATION
        );
        assert_eq!(
            resolve(&f, OP_COARSE_LOCATION, PRIV_APP, "priv", Some("ar")),
            OP_COARSE_LOCATION
        );
        assert_eq!(
            resolve(&f, OP_READ_CLIPBOARD, APP, "app", Some("tag")),
            OP_READ_CLIPBOARD
        );
        assert_eq!(
            resolve(&f, OP_ACTIVITY_RECOGNITION, PRIV_APP, "priv", Some("ar2")),
            113
        );
        assert_eq!(
            resolve(
                &f,
                OP_ACTIVITY_RECOGNITION,
                PRIV_APP + PER_USER_RANGE,
                "priv",
                Some("ar")
            ),
            113
        );
        assert_eq!(
            resolve(&f, OP_ACTIVITY_RECOGNITION, PRIV_APP, "priv", Some("other")),
            OP_ACTIVITY_RECOGNITION
        );
        assert_eq!(
            resolve(&f, OP_ACTIVITY_RECOGNITION, APP, "app", Some("ar")),
            OP_ACTIVITY_RECOGNITION
        );
        assert_eq!(
            resolve(&f, OP_ACTIVITY_RECOGNITION, PRIV_APP, "priv", None),
            OP_ACTIVITY_RECOGNITION
        );
    }

    #[test]
    fn notes_are_collected_for_dangerous_and_app_op_permissions() {
        let mut f = fake();
        let protection = |permission: &str| {
            Ok(match permission {
                "android.permission.READ_CONTACTS"
                | "android.permission.health.READ_HEART_RATE" => Some(PROTECTION_DANGEROUS),
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
        f.flags
            .insert("android.permission.flags.replace_body_sensor_permission_enabled");
        assert!(collect(&f, 149));
        let out = should_collect_notes(&f, protection, NUM_OP).unwrap_err();
        assert_eq!(out.message, "opCode is out of range of [0, 162] (too high)");
    }

    const CLIENT: Binder = Binder::Handle(7);
    const OTHER_CLIENT: Binder = Binder::Handle(8);
    const WATCHER: u32 = 40;
    const OTHER_WATCHER: u32 = 41;
    const NO_CHAIN: Attribution = Attribution {
        start_if_mode_default: false,
        flags: 0,
        chain_id: ATTRIBUTION_CHAIN_ID_NONE,
    };

    fn active(op: i32, uid: i32, package: &str, active: bool, watchers: Vec<(u32, i32)>) -> Notice {
        Notice {
            watchers,
            op,
            uid,
            package: Some(package.into()),
            tag: None,
            attribution_flags: 0,
            chain_id: ATTRIBUTION_CHAIN_ID_NONE,
            heard: Heard::Active(active),
        }
    }

    fn start(
        f: &Fake,
        started: &mut Started<Binder>,
        client: Binder,
        op: &Unchecked<'_>,
        attribution: Attribution,
    ) -> (SyncNoted, Vec<Notice>) {
        let (synced, effects, _) = run(f, op.uid, started, |ops| {
            ops.start(Some(client), op, attribution)
        });
        (synced.unwrap(), effects.notices)
    }

    fn finish(
        f: &Fake,
        started: &mut Started<Binder>,
        client: Binder,
        code: i32,
        uid: i32,
        package: &str,
    ) -> (Vec<Notice>, Vec<Binder>) {
        let (result, effects, ended) = run(f, uid, started, |ops| {
            ops.finish(Some(client), code, uid, Some(package), None)
        });
        result.unwrap();
        (effects.notices, ended)
    }

    /// A note leaves its access, or its rejection, by the uid's state and
    /// the note's flags; a running start counts as an access so far, a
    /// finished one keeps its duration; the package's ops are collected by
    /// code with the package's mode, and a uid's non-default modes as its
    /// ops.
    #[test]
    fn notes_and_starts_leave_their_last_access() {
        let mut f = fake();
        f.running.insert(APP);
        f.uid_modes.insert((APP, "android:camera"), MODE_IGNORED);
        let mut started = Started::default();
        let noted = run(&f, APP, &mut started, |ops| {
            ops.note(&own(OP_COARSE_LOCATION, APP, "app"), DEVICE_ID_DEFAULT)
        });
        assert_eq!(noted.0.unwrap().mode, MODE_ALLOWED);
        let rejected = run(&f, APP, &mut started, |ops| {
            ops.note(&own(OP_CAMERA, APP, "app"), DEVICE_ID_DEFAULT)
        });
        assert_eq!(rejected.0.unwrap().mode, MODE_IGNORED);
        let key = history::make_key(history::UID_STATE_TOP, OP_FLAG_SELF);
        let entries = collect_ops(&started, &f, APP, "app", None, true, false)
            .unwrap()
            .unwrap();
        let codes: Vec<i32> = entries.iter().map(|e| e.op).collect();
        assert_eq!(codes, [OP_COARSE_LOCATION, OP_CAMERA]);
        let (tag, coarse) = &entries[0].attributed[0];
        assert_eq!(*tag, None);
        let access = &coarse.access.as_ref().unwrap()[&key];
        assert_eq!(access.duration, -1);
        assert!(coarse.reject.is_none() && !coarse.running);
        let camera = &entries[1].attributed[0].1;
        assert!(camera.access.is_none() && camera.reject.as_ref().unwrap().contains_key(&key));
        // Only the requested ops the package has, in their order.
        let only = collect_ops(
            &started,
            &f,
            APP,
            "app",
            Some(&[OP_CAMERA, OP_GPS]),
            true,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(only.iter().map(|e| e.op).collect::<Vec<_>>(), [OP_CAMERA]);
        assert_eq!(
            collect_ops(&started, &f, APP, "app", Some(&[OP_GPS]), true, false).unwrap(),
            None
        );
        start(&f, &mut started, CLIENT, &own(OP_GPS, APP, "app"), NO_CHAIN);
        let gps = |started: &Started<Binder>| {
            collect_ops(started, &f, APP, "app", Some(&[OP_GPS]), true, false)
                .unwrap()
                .unwrap()
                .remove(0)
                .attributed
                .remove(0)
                .1
        };
        let running = gps(&started);
        assert!(running.running && running.access.as_ref().unwrap()[&key].duration >= 0);
        finish(&f, &mut started, CLIENT, OP_GPS, APP, "app");
        let finished = gps(&started);
        assert!(!finished.running && finished.access.as_ref().unwrap()[&key].duration >= 0);
        // Another device's entries carry no attributions.
        let other = collect_ops(&started, &f, APP, "app", None, false, false)
            .unwrap()
            .unwrap();
        assert!(other.iter().all(|e| e.attributed.is_empty()));
        let uid_ops = collect_uid_ops(&f, APP, None).unwrap().unwrap();
        assert_eq!(
            uid_ops.iter().map(|e| (e.op, e.mode)).collect::<Vec<_>>(),
            [(OP_CAMERA, MODE_IGNORED)]
        );
    }

    /// `getLastProxyInfo` of proxied accesses: the latest's proxy, whatever
    /// its attribution; a self access is not one.
    #[test]
    fn the_last_proxy_is_the_latest_proxied_access() {
        let proxy = |uid| ProxyInfo {
            uid,
            package: Some("priv".into()),
            tag: None,
            device_id: PERSISTENT_DEVICE_ID_DEFAULT.into(),
        };
        let attributed = |key: i64, time, proxy| AttributedEntry {
            op: OP_CAMERA,
            running: true,
            access: Some(
                [(
                    key,
                    NoteEvent {
                        time,
                        duration: 3,
                        proxy,
                    },
                )]
                .into(),
            ),
            reject: None,
        };
        let top = history::UID_STATE_TOP;
        let entry = OpEntry {
            op: OP_CAMERA,
            mode: MODE_ALLOWED,
            attributed: vec![
                (
                    None,
                    attributed(
                        history::make_key(top, OP_FLAG_TRUSTED_PROXIED),
                        5,
                        Some(proxy(PRIV_APP)),
                    ),
                ),
                (
                    Some("a".into()),
                    attributed(
                        history::make_key(top, OP_FLAG_UNTRUSTED_PROXIED),
                        9,
                        Some(proxy(APP)),
                    ),
                ),
                (
                    Some("b".into()),
                    attributed(history::make_key(top, OP_FLAG_SELF), 12, None),
                ),
            ],
        };
        assert_eq!(last_proxy(&entry).map(|p| p.uid), Some(APP));
    }

    fn is_active(
        f: &Fake,
        started: &mut Started<Binder>,
        code: i32,
        uid: i32,
        package: &str,
    ) -> bool {
        run(f, uid, started, |ops| ops.active(code, uid, Some(package)))
            .0
            .unwrap()
    }

    /// An op a client starts twice is active (watchers hear it once)
    /// until the client finished both starts.
    #[test]
    fn a_started_op_is_active_until_its_starts_finish() {
        let f = fake();
        let mut started = Started::default();
        started.watch(
            Watch::Active,
            WATCHER,
            &[OP_CAMERA],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(WATCHER),
        );
        let camera = own(OP_CAMERA, APP, "app");
        let (synced, notices) = start(&f, &mut started, CLIENT, &camera, NO_CHAIN);
        assert_eq!(
            synced,
            SyncNoted::new(MODE_ALLOWED, OP_CAMERA, None, Some("app"))
        );
        assert_eq!(
            notices,
            [active(
                OP_CAMERA,
                APP,
                "app",
                true,
                vec![(WATCHER, SYSTEM_UID)]
            )]
        );
        assert!(
            start(&f, &mut started, CLIENT, &camera, NO_CHAIN)
                .1
                .is_empty()
        );
        assert!(is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        assert_eq!(
            finish(&f, &mut started, CLIENT, OP_CAMERA, APP, "app"),
            (vec![], vec![])
        );
        assert!(is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        let inactive = active(OP_CAMERA, APP, "app", false, vec![(WATCHER, SYSTEM_UID)]);
        assert_eq!(
            finish(&f, &mut started, CLIENT, OP_CAMERA, APP, "app"),
            (vec![inactive], vec![CLIENT])
        );
        assert!(!is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        // Another uid's op only with WATCH_APPOPS.
        start(&f, &mut started, CLIENT, &camera, NO_CHAIN);
        assert!(
            !run(&f, PRIV_APP, &mut started, |ops| ops.active(
                OP_CAMERA,
                APP,
                Some("app")
            ))
            .0
            .unwrap()
        );
        assert!(
            run(&f, SYSTEM_UID, &mut started, |ops| ops.active(
                OP_CAMERA,
                APP,
                Some("app")
            ))
            .0
            .unwrap()
        );
    }

    /// A client's death finishes all of its starts; the op stays active
    /// while another client runs it.
    #[test]
    fn a_clients_death_finishes_its_starts() {
        let f = fake();
        let mut started = Started::default();
        started.watch(
            Watch::Active,
            WATCHER,
            &[OP_RECORD_AUDIO],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(WATCHER),
        );
        let audio = own(OP_RECORD_AUDIO, APP, "app");
        start(&f, &mut started, CLIENT, &audio, NO_CHAIN);
        start(&f, &mut started, CLIENT, &audio, NO_CHAIN);
        start(&f, &mut started, OTHER_CLIENT, &audio, NO_CHAIN);
        let dead = Client {
            key: (APP, "app".into(), OP_RECORD_AUDIO),
            tag: None,
            binder: CLIENT,
        };
        let mut effects = Effects::default();
        assert_eq!(started.client_died(&dead, &mut effects), [CLIENT]);
        assert!(effects.notices.is_empty());
        assert!(is_active(&f, &mut started, OP_RECORD_AUDIO, APP, "app"));
        let (notices, ended) = finish(&f, &mut started, OTHER_CLIENT, OP_RECORD_AUDIO, APP, "app");
        assert_eq!(
            notices,
            [active(
                OP_RECORD_AUDIO,
                APP,
                "app",
                false,
                vec![(WATCHER, SYSTEM_UID)]
            )]
        );
        assert_eq!(ended, [OTHER_CLIENT]);
        // A dead client that had finished is nothing.
        assert!(started.client_died(&dead, &mut effects).is_empty());
    }

    /// A watcher of one uid hears only that uid's ops; started watchers
    /// hear each start with its flags, mode and type.
    #[test]
    fn watchers_hear_their_uids_ops() {
        let f = fake();
        let mut started = Started::default();
        started.watch(Watch::Active, WATCHER, &[OP_CAMERA], APP, APP, || {
            Binder::Handle(WATCHER)
        });
        started.watch(
            Watch::Active,
            OTHER_WATCHER,
            &[OP_CAMERA],
            PRIV_APP,
            PRIV_APP,
            || Binder::Handle(OTHER_WATCHER),
        );
        started.watch(
            Watch::Started,
            WATCHER,
            &[OP_CAMERA],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(WATCHER),
        );
        let attribution = Attribution {
            start_if_mode_default: false,
            flags: 0x1,
            chain_id: 5,
        };
        let (_, notices) = start(
            &f,
            &mut started,
            CLIENT,
            &own(OP_CAMERA, APP, "app"),
            attribution,
        );
        let heard = Heard::Started {
            flags: OP_FLAG_SELF,
            mode: MODE_ALLOWED,
            start_type: START_TYPE_STARTED,
        };
        let attributed = |notice: Notice| Notice {
            attribution_flags: 0x1,
            chain_id: 5,
            ..notice
        };
        assert_eq!(
            notices,
            [
                attributed(active(OP_CAMERA, APP, "app", true, vec![(WATCHER, APP)])),
                attributed(Notice {
                    heard,
                    ..active(OP_CAMERA, APP, "app", true, vec![(WATCHER, SYSTEM_UID)])
                }),
            ]
        );
        assert_eq!(
            started.unwatch(Watch::Started, WATCHER),
            Some(Binder::Handle(WATCHER))
        );
        assert!(
            start(
                &f,
                &mut started,
                OTHER_CLIENT,
                &own(OP_CAMERA, APP, "app"),
                NO_CHAIN
            )
            .1
            .is_empty()
        );
    }

    /// A start the mode rejects starts nothing; a default mode starts only
    /// with `startIfModeDefault`; a restricted op starts paused, inactive.
    #[test]
    fn rejected_and_restricted_starts() {
        let mut f = fake();
        let mut started = Started::default();
        let watched = [OP_CAMERA, OP_LEGACY_STORAGE];
        started.watch(
            Watch::Started,
            WATCHER,
            &watched,
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(WATCHER),
        );
        f.uid_modes.insert((APP, "android:camera"), MODE_IGNORED);
        let (synced, notices) = start(
            &f,
            &mut started,
            CLIENT,
            &own(OP_CAMERA, APP, "app"),
            NO_CHAIN,
        );
        assert_eq!(synced.mode, MODE_IGNORED);
        assert_eq!(
            notices[0].heard,
            Heard::Started {
                flags: OP_FLAG_SELF,
                mode: MODE_IGNORED,
                start_type: START_TYPE_FAILED
            }
        );
        assert!(!is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        let storage = own(OP_LEGACY_STORAGE, APP, "app");
        assert_eq!(
            start(&f, &mut started, CLIENT, &storage, NO_CHAIN).0.mode,
            MODE_DEFAULT
        );
        let if_default = Attribution {
            start_if_mode_default: true,
            ..NO_CHAIN
        };
        assert_eq!(
            start(&f, &mut started, CLIENT, &storage, if_default).0.mode,
            MODE_ALLOWED
        );
        assert!(is_active(&f, &mut started, OP_LEGACY_STORAGE, APP, "app"));
        f.uid_modes.clear();
        f.global.insert(OP_CAMERA);
        started.watch(
            Watch::Active,
            OTHER_WATCHER,
            &[OP_CAMERA],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(OTHER_WATCHER),
        );
        let (synced, notices) = start(
            &f,
            &mut started,
            CLIENT,
            &own(OP_CAMERA, APP, "app"),
            NO_CHAIN,
        );
        assert_eq!(synced.mode, MODE_IGNORED);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].heard,
            Heard::Started {
                flags: OP_FLAG_SELF,
                mode: MODE_IGNORED,
                start_type: START_TYPE_FAILED
            }
        );
        assert!(!is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        assert_eq!(
            finish(&f, &mut started, CLIENT, OP_CAMERA, APP, "app"),
            (vec![], vec![CLIENT])
        );
    }

    /// A client's user restriction (`setUserRestriction`) ignores the op;
    /// `updateStartedOpModeForUserForDefaultDevice` pauses a running start
    /// (inactive) and, once nothing restricts it, resumes it (active, and
    /// START_TYPE_RESUMED to the started watchers).
    #[test]
    fn client_user_restrictions_pause_and_resume_started_ops() {
        let mut f = fake();
        let mut started = Started::default();
        started.watch(
            Watch::Active,
            WATCHER,
            &[OP_CAMERA],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(WATCHER),
        );
        started.watch(
            Watch::Started,
            OTHER_WATCHER,
            &[OP_CAMERA],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(OTHER_WATCHER),
        );
        start(
            &f,
            &mut started,
            CLIENT,
            &own(OP_CAMERA, APP, "app"),
            NO_CHAIN,
        );
        f.clients.add_client(1, ());
        assert!(f.clients.set(1, &[0], OP_CAMERA, true, None));
        assert_eq!(check_mode(&f, OP_CAMERA, APP, "app"), MODE_IGNORED);
        assert_eq!(check_mode(&f, OP_CAMERA, 1_010_100, "app"), MODE_ALLOWED);
        let (paused, effects, _) = run(&f, SYSTEM_UID, &mut started, |ops| {
            ops.update_started_mode(OP_CAMERA, true, 0)
        });
        paused.unwrap();
        assert_eq!(
            effects.notices,
            [active(
                OP_CAMERA,
                APP,
                "app",
                false,
                vec![(WATCHER, SYSTEM_UID)]
            )]
        );
        assert!(!is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        assert!(f.clients.set(1, &[0], OP_CAMERA, false, None));
        let (resumed, effects, _) = run(&f, SYSTEM_UID, &mut started, |ops| {
            ops.update_started_mode(OP_CAMERA, false, 0)
        });
        resumed.unwrap();
        let heard = Heard::Started {
            flags: OP_FLAG_SELF,
            mode: MODE_ALLOWED,
            start_type: START_TYPE_RESUMED,
        };
        assert_eq!(
            effects.notices,
            [
                active(OP_CAMERA, APP, "app", true, vec![(WATCHER, SYSTEM_UID)]),
                Notice {
                    heard,
                    ..active(
                        OP_CAMERA,
                        APP,
                        "app",
                        true,
                        vec![(OTHER_WATCHER, SYSTEM_UID)]
                    )
                },
            ]
        );
        assert!(is_active(&f, &mut started, OP_CAMERA, APP, "app"));
    }

    /// A package not the uid's errs, a uid without packages has no ops,
    /// starting needs a client, and another uid's start UPDATE_APP_OPS_STATS.
    #[test]
    fn starts_that_err() {
        let f = fake();
        let mut started = Started::default();
        assert_eq!(
            start(
                &f,
                &mut started,
                CLIENT,
                &own(OP_CAMERA, APP, "priv"),
                NO_CHAIN
            )
            .0
            .mode,
            MODE_ERRORED
        );
        let media = own(OP_CAMERA, MEDIA_UID, "media");
        assert_eq!(
            run(&f, SYSTEM_UID, &mut started, |ops| ops.start(
                Some(CLIENT),
                &media,
                NO_CHAIN
            ))
            .0
            .unwrap()
            .mode,
            MODE_ERRORED
        );
        let (result, _, _) = run(&f, APP, &mut started, |ops| {
            ops.start(None, &own(OP_CAMERA, APP, "app"), NO_CHAIN)
        });
        assert_eq!(result.unwrap_err().code, EX_NULL_POINTER);
        let (result, _, _) = run(&f, PRIV_APP, &mut started, |ops| {
            ops.start(Some(CLIENT), &own(OP_CAMERA, APP, "app"), NO_CHAIN)
        });
        assert_eq!(
            result.unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
    }

    /// A tag the package does not declare is dropped only where invalid
    /// tags are rejected; the proxy's declaring it suffices.
    #[test]
    fn attribution_tags_the_package_does_not_declare() {
        let mut f = fake();
        f.attributions.insert("app", vec!["declared"]);
        f.attributions.insert("priv", vec!["proxied"]);
        let tagged = |tag| Unchecked {
            tag: Some(tag),
            ..own(OP_READ_CLIPBOARD, APP, "app")
        };
        let tag_of = |f: &Fake, op: &Unchecked<'_>| {
            run(f, SYSTEM_UID, &mut Started::default(), |ops| {
                ops.note_unchecked(op)
            })
            .0
            .unwrap()
            .tag
        };
        assert_eq!(
            tag_of(&f, &tagged("undeclared")).as_deref(),
            Some("undeclared")
        );
        f.invalid_tags_rejected = true;
        assert_eq!(tag_of(&f, &tagged("undeclared")), None);
        assert_eq!(tag_of(&f, &tagged("declared")).as_deref(), Some("declared"));
        let proxied = Unchecked {
            proxy: Some((PRIV_APP, "priv")),
            proxy_tag: None,
            ..tagged("proxied")
        };
        assert_eq!(tag_of(&f, &proxied).as_deref(), Some("proxied"));
    }

    fn source(
        uid: i32,
        package: &str,
        next: Option<AttributionSourceState>,
    ) -> AttributionSourceState {
        AttributionSourceState {
            pid: -1,
            uid,
            device_id: DEVICE_ID_DEFAULT,
            package: Some(package.into()),
            tag: None,
            token: None,
            renounced: None,
            next: next.map(|next| vec![Some(next)]),
        }
    }

    /// A proxy without UPDATE_APP_OPS_STATS starts the op for itself (its
    /// skip not honored) and for the proxied, untrusted and collected for
    /// itself; a proxied that could not start it starts nothing.
    #[test]
    fn proxy_starts() {
        let mut f = fake();
        let mut started = Started::default();
        started.watch(
            Watch::Started,
            WATCHER,
            &[OP_CAMERA],
            INVALID_UID,
            SYSTEM_UID,
            || Binder::Handle(WATCHER),
        );
        let chain = source(PRIV_APP, "priv", Some(source(APP, "app", None)));
        let attribution = ProxyAttribution {
            start_if_mode_default: false,
            proxy_flags: 0x1,
            proxied_flags: 0x4,
            chain_id: 9,
        };
        let (synced, effects, _) = run(&f, PRIV_APP, &mut started, |ops| {
            ops.start_proxy(
                Some(CLIENT),
                OP_CAMERA,
                &chain,
                true,
                attribution,
                true,
                Some("why"),
            )
        });
        assert_eq!(
            synced.unwrap(),
            SyncNoted::new(MODE_ALLOWED, OP_CAMERA, None, Some("app"))
        );
        let heard: Vec<_> = effects
            .notices
            .iter()
            .map(|n| (n.uid, &n.heard, n.attribution_flags))
            .collect();
        let started_with = |flags| Heard::Started {
            flags,
            mode: MODE_ALLOWED,
            start_type: START_TYPE_STARTED,
        };
        let proxy = started_with(OP_FLAG_UNTRUSTED_PROXY);
        let proxied = started_with(OP_FLAG_UNTRUSTED_PROXIED);
        assert_eq!(heard, [(PRIV_APP, &proxy, 0x1), (APP, &proxied, 0x4)]);
        let collected = |uid, package: &str, message: &str| Collected {
            uid,
            package: package.into(),
            op: OP_CAMERA,
            tag: None,
            message: message.into(),
        };
        assert_eq!(
            effects.collected,
            [
                collected(PRIV_APP, "priv", "proxy why"),
                collected(APP, "app", "why")
            ]
        );
        assert!(is_active(&f, &mut started, OP_CAMERA, PRIV_APP, "priv"));
        assert!(is_active(&f, &mut started, OP_CAMERA, APP, "app"));
        let (result, _, ended) = run(&f, PRIV_APP, &mut started, |ops| {
            ops.finish_proxy(Some(CLIENT), OP_CAMERA, &chain, true)
        });
        result.unwrap();
        assert_eq!(ended, [CLIENT, CLIENT]);
        f.uid_modes.insert((APP, "android:camera"), MODE_ERRORED);
        let (synced, effects, _) = run(&f, PRIV_APP, &mut started, |ops| {
            ops.start_proxy(
                Some(CLIENT),
                OP_CAMERA,
                &chain,
                false,
                attribution,
                false,
                None,
            )
        });
        assert_eq!(synced.unwrap().mode, MODE_ERRORED);
        assert!(effects.notices.is_empty());
        assert!(!is_active(&f, &mut started, OP_CAMERA, PRIV_APP, "priv"));
    }

    /// A trusted proxy's skip is honored: only the proxied's op is noted.
    #[test]
    fn proxy_notes() {
        let mut f = fake();
        let chain = source(PRIV_APP, "priv", Some(source(APP, "app", None)));
        let note = |f: &Fake, skip| {
            run(f, PRIV_APP, &mut Started::default(), |ops| {
                ops.note_proxy(OP_CAMERA, &chain, skip, false, None)
            })
        };
        let (noted, effects, _) = note(&f, true);
        assert_eq!(
            noted.unwrap(),
            SyncNoted::new(MODE_ALLOWED, OP_CAMERA, None, Some("app"))
        );
        assert_eq!(effects.collected.len(), 1);
        f.uid_modes
            .insert((PRIV_APP, "android:camera"), MODE_IGNORED);
        assert_eq!(note(&f, true).0.unwrap().mode, MODE_IGNORED);
        f.granted.insert((PRIV_APP, UPDATE_APP_OPS_STATS));
        let (noted, effects, _) = note(&f, true);
        assert_eq!(noted.unwrap().mode, MODE_ALLOWED);
        assert!(effects.collected.is_empty());
    }
}
