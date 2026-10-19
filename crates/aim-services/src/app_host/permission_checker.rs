//! `permission_checker`: PermissionManagerService's
//! `PermissionCheckerService` (`IPermissionChecker`) for the app host, at
//! `android-16.0.0_r1` (ADR 0013): `checkPermission`, `checkOp` and
//! `finishDataDelivery` of an attribution chain (`AttributionSourceState`
//! and its `next`), which `PermissionChecker` asks for permissions with an
//! app op. Every uid in the chain must hold the permission (as the native
//! `permissionmgr` answers `checkUidPermission`, without a renounced one);
//! a runtime permission's op must be allowed and an app op permission's op
//! decides, falling back to the permission at its default mode. The op is
//! checked for a preflight (evaluated, not raw: #1595), noted for data
//! delivery and started for `startDataDelivery` by the app host's `appops`
//! as the caller asks it, not collected for the app (#1592): a
//! proxy op where a link proxies for the next or where the caller may not
//! blame the accessor alone, a started chain attributed (its id and each
//! link's accessor, intermediary or receiver flags), and each started link
//! registered until its delivery finishes or its token dies. A datasource
//! vets only the chain after it. Trusted (registered) attribution sources,
//! a virtual device's permissions and Health Connect's are not implemented
//! (#1527).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, EX_SECURITY, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_permission_ipermissionchecker as checker;
use aim_service_aidl::{ReadParcelable, read_string_list, read_typed_list};

use super::appops_service::{self, ATTRIBUTION_CHAIN_ID_NONE, ATTRIBUTION_FLAG_TRUSTED, SYSTEM_UID};
use super::display::parcelable;
use super::permission_manager::PermissionManager;
use super::{AppHost, reply};
use crate::package::apps_filter::NotModelled;
use crate::package::query::Query;

/// `PermissionCheckerManager.PERMISSION_GRANTED`, `PERMISSION_SOFT_DENIED`,
/// `PERMISSION_HARD_DENIED`.
pub(super) const GRANTED: i32 = 0;
const SOFT_DENIED: i32 = 1;
pub(super) const HARD_DENIED: i32 = 2;
/// `AppOpsManager.OP_NONE`, `MODE_ALLOWED`, `MODE_IGNORED`,
/// `MODE_ERRORED`, `MODE_DEFAULT`, `MODE_FOREGROUND`.
const OP_NONE: i32 = -1;
const MODE_ALLOWED: i32 = 0;
const MODE_IGNORED: i32 = 1;
const MODE_ERRORED: i32 = 2;
const MODE_DEFAULT: i32 = 3;
const MODE_FOREGROUND: i32 = 4;
/// `AppOpsManager.ATTRIBUTION_FLAGS_NONE`, `ATTRIBUTION_FLAG_ACCESSOR`,
/// `ATTRIBUTION_FLAG_INTERMEDIARY`, `ATTRIBUTION_FLAG_RECEIVER`.
const ATTRIBUTION_FLAGS_NONE: i32 = 0;
const ATTRIBUTION_FLAG_ACCESSOR: i32 = 0x1;
const ATTRIBUTION_FLAG_INTERMEDIARY: i32 = 0x2;
const ATTRIBUTION_FLAG_RECEIVER: i32 = 0x4;
/// `PermissionInfo.PROTECTION_MASK_BASE`, `PROTECTION_DANGEROUS`,
/// `PROTECTION_FLAG_APPOP`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_FLAG_APPOP: i32 = 0x40;
/// `Context.DEVICE_ID_DEFAULT`.
const DEVICE_ID_DEFAULT: i32 = 0;
const RENOUNCE_PERMISSIONS: &str = "android.permission.RENOUNCE_PERMISSIONS";
const UPDATE_APP_OPS_STATS: &str = "android.permission.UPDATE_APP_OPS_STATS";
/// `HealthConnectManager.HEALTH_PERMISSION_PREFIX`.
const HEALTH_PERMISSION_PREFIX: &str = "android.permission.health.";
const RANGING: &str = "android.permission.flags.ranging_permission_enabled";
const BODY_SENSORS: &str = "android.permission.flags.replace_body_sensor_permission_enabled";
const XR: &str = "android.xr.xr_manifest_entries";
const SYSTEM_PREFERENCES: &str = "com.android.settingslib.flags.write_system_preference_permission_enabled";
/// `AppOpsManager.sPermToOp`: the permissions of `RUNTIME_PERMISSION_OPS`,
/// `APP_OP_PERMISSION_PACKAGE_OPS` and `APP_OP_PERMISSION_UID_OPS` in
/// `sAppOpInfos`, with the aconfig flag a permission needs.
const PERMISSION_OPS: &[(&str, i32, Option<&str>)] = &[
    ("android.permission.READ_CONTACTS", 4, None),
    ("android.permission.WRITE_CONTACTS", 5, None),
    ("android.permission.GET_ACCOUNTS", 62, None),
    ("android.permission.READ_CALENDAR", 8, None),
    ("android.permission.WRITE_CALENDAR", 9, None),
    ("android.permission.SEND_SMS", 20, None),
    ("android.permission.RECEIVE_SMS", 16, None),
    ("android.permission.READ_SMS", 14, None),
    ("android.permission.RECEIVE_WAP_PUSH", 19, None),
    ("android.permission.RECEIVE_MMS", 18, None),
    ("android.permission.READ_CELL_BROADCASTS", 57, None),
    ("android.permission.READ_EXTERNAL_STORAGE", 59, None),
    ("android.permission.WRITE_EXTERNAL_STORAGE", 60, None),
    ("android.permission.ACCESS_MEDIA_LOCATION", 90, None),
    ("android.permission.ACCESS_COARSE_LOCATION", 0, None),
    ("android.permission.ACCESS_FINE_LOCATION", 1, None),
    ("android.permission.READ_PHONE_STATE", 51, None),
    ("android.permission.READ_PHONE_NUMBERS", 65, None),
    ("android.permission.CALL_PHONE", 13, None),
    ("android.permission.READ_CALL_LOG", 6, None),
    ("android.permission.WRITE_CALL_LOG", 7, None),
    ("com.android.voicemail.permission.ADD_VOICEMAIL", 52, None),
    ("android.permission.USE_SIP", 53, None),
    ("android.permission.PROCESS_OUTGOING_CALLS", 54, None),
    ("android.permission.ANSWER_PHONE_CALLS", 69, None),
    ("android.permission.ACCEPT_HANDOVER", 74, None),
    ("android.permission.RECORD_AUDIO", 27, None),
    ("android.permission.CAMERA", 26, None),
    ("android.permission.BODY_SENSORS", 56, None),
    ("android.permission.ACTIVITY_RECOGNITION", 79, None),
    ("android.permission.READ_MEDIA_AUDIO", 81, None),
    ("android.permission.READ_MEDIA_VIDEO", 83, None),
    ("android.permission.READ_MEDIA_IMAGES", 85, None),
    ("android.permission.READ_MEDIA_VISUAL_USER_SELECTED", 123, None),
    ("android.permission.BLUETOOTH_SCAN", 77, None),
    ("android.permission.BLUETOOTH_CONNECT", 111, None),
    ("android.permission.BLUETOOTH_ADVERTISE", 114, None),
    ("android.permission.UWB_RANGING", 112, None),
    ("android.permission.NEARBY_WIFI_DEVICES", 116, None),
    ("android.permission.RANGING", 151, Some(RANGING)),
    ("android.permission.POST_NOTIFICATIONS", 11, None),
    ("android.permission.health.READ_HEART_RATE", 149, Some(BODY_SENSORS)),
    ("android.permission.health.READ_SKIN_TEMPERATURE", 150, Some(BODY_SENSORS)),
    ("android.permission.health.READ_OXYGEN_SATURATION", 152, Some(BODY_SENSORS)),
    ("android.permission.EYE_TRACKING_COARSE", 156, Some(XR)),
    ("android.permission.EYE_TRACKING_FINE", 157, Some(XR)),
    ("android.permission.FACE_TRACKING", 158, Some(XR)),
    ("android.permission.HAND_TRACKING", 159, Some(XR)),
    ("android.permission.HEAD_TRACKING", 160, Some(XR)),
    ("android.permission.SCENE_UNDERSTANDING_COARSE", 161, Some(XR)),
    ("android.permission.SCENE_UNDERSTANDING_FINE", 162, Some(XR)),
    ("android.permission.ACCESS_NOTIFICATIONS", 25, None),
    ("android.permission.SYSTEM_ALERT_WINDOW", 24, None),
    ("android.permission.WRITE_SETTINGS", 23, None),
    ("android.permission.PACKAGE_USAGE_STATS", 43, None),
    ("android.permission.REQUEST_INSTALL_PACKAGES", 66, None),
    ("android.permission.FOREGROUND_SERVICE", 76, None),
    ("android.permission.SMS_FINANCIAL_TRANSACTIONS", 80, None),
    ("android.permission.MANAGE_IPSEC_TUNNELS", 75, None),
    ("android.permission.INSTANT_APP_FOREGROUND_SERVICE", 68, None),
    ("android.permission.LOADER_USAGE_STATS", 95, None),
    ("android.permission.MANAGE_EXTERNAL_STORAGE", 92, None),
    ("android.permission.INTERACT_ACROSS_PROFILES", 93, None),
    ("android.permission.MANAGE_ONGOING_CALLS", 103, None),
    ("android.permission.USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER", 105, None),
    ("android.permission.SCHEDULE_EXACT_ALARM", 107, None),
    ("android.permission.MANAGE_MEDIA", 110, None),
    ("android.permission.TURN_SCREEN_ON", 61, None),
    ("android.permission.FOREGROUND_SERVICE_SPECIAL_USE", 127, None),
    ("android.permission.CAPTURE_CONSENTLESS_BUGREPORT_ON_USERDEBUG_BUILD", 131, None),
    ("android.permission.USE_FULL_SCREEN_INTENT", 133, None),
    ("android.permission.RECEIVE_SANDBOX_TRIGGER_AUDIO", 136, None),
    ("android.permission.MEDIA_ROUTING_CONTROL", 139, None),
    ("android.permission.READ_SYSTEM_GRAMMATICAL_GENDER", 143, None),
    ("android.permission.WRITE_SYSTEM_PREFERENCES", 153, Some(SYSTEM_PREFERENCES)),
];

/// `AttributionSourceState`, a structured parcelable.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributionSourceState {
    pub pid: i32,
    pub uid: i32,
    pub device_id: i32,
    pub package: Option<String>,
    pub tag: Option<String>,
    pub token: Option<Binder>,
    pub renounced: Option<Vec<Option<String>>>,
    pub next: Option<Vec<Option<AttributionSourceState>>>,
}

impl ReadParcelable for AttributionSourceState {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        parcelable(r, |r| {
            Ok(AttributionSourceState {
                pid: r.read_i32()?,
                uid: r.read_i32()?,
                device_id: r.read_i32()?,
                package: r.read_string16()?,
                tag: r.read_string16()?,
                token: r.read_binder()?,
                renounced: read_string_list(r)?,
                next: read_typed_list(r)?,
            })
        })
    }
}

/// The chain `AttributionSource.getNext` walks: each source's first
/// `next`.
fn chain(source: &AttributionSourceState) -> Result<Vec<&AttributionSourceState>, Exception> {
    let mut chain = vec![source];
    let mut current = source;
    while let Some(next) = current.next.as_deref().and_then(<[_]>::first) {
        current = next.as_ref().ok_or_else(|| Exception::new(EX_NULL_POINTER, "next"))?;
        chain.push(current);
    }
    Ok(chain)
}

/// How a link of a chain is attributed in its started op
/// (`resolveProxyAttributionFlags`, `resolveProxiedAttributionFlags`):
/// the proxy's and the proxied's `ATTRIBUTION_FLAG_*` and the chain id.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Attribution {
    proxy_flags: i32,
    proxied_flags: i32,
    chain_id: i32,
}

impl Attribution {
    const NONE: Attribution =
        Attribution { proxy_flags: ATTRIBUTION_FLAGS_NONE, proxied_flags: ATTRIBUTION_FLAGS_NONE, chain_id: ATTRIBUTION_CHAIN_ID_NONE };
}

/// `sAttributionChainIds`.
static ATTRIBUTION_CHAIN_IDS: AtomicI32 = AtomicI32::new(0);

/// `getAttributionChainId`: a new id for a start of a chain of more than
/// one source.
fn attribution_chain_id(start_data_delivery: bool, chain: &[&AttributionSourceState]) -> i32 {
    if !start_data_delivery || chain.len() < 2 {
        return ATTRIBUTION_CHAIN_ID_NONE;
    }
    let id = ATTRIBUTION_CHAIN_IDS.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if id < 0 {
        ATTRIBUTION_CHAIN_IDS.store(1, Ordering::Relaxed);
        return 1;
    }
    id
}

/// `resolveAttributionFlags` of the link `at` (none past the chain's end)
/// of a chain of `len` sources: the accessor, an intermediary or the
/// receiver, trusted or not.
fn attribution_flags(len: usize, at: Option<usize>, from_datasource: bool, self_access: bool, trusted: bool, for_proxy: bool) -> i32 {
    let Some(at) = at else { return ATTRIBUTION_FLAGS_NONE };
    let trusted = if trusted { ATTRIBUTION_FLAG_TRUSTED } else { ATTRIBUTION_FLAGS_NONE };
    if for_proxy {
        if self_access || (!from_datasource && at == 0) {
            return trusted | ATTRIBUTION_FLAG_ACCESSOR;
        }
    } else if self_access {
        return trusted | ATTRIBUTION_FLAG_RECEIVER;
    } else if from_datasource && at == 1 {
        return trusted | ATTRIBUTION_FLAG_ACCESSOR;
    } else if at + 1 >= len {
        return trusted | ATTRIBUTION_FLAG_RECEIVER;
    }
    if from_datasource && at == 0 {
        return ATTRIBUTION_FLAGS_NONE;
    }
    trusted | ATTRIBUTION_FLAG_INTERMEDIARY
}

/// The message of a proxy op's SecurityException as `performOpTransaction`
/// rethrows it: the op, the source and its next two links.
fn security_message(op: i32, source: &AttributionSourceState, e: &Exception) -> String {
    let next = |s: &AttributionSourceState| s.next.as_deref().and_then(<[_]>::first).cloned().flatten();
    let name = |s: Option<&AttributionSourceState>| s.and_then(|s| s.package.clone()).unwrap_or_else(|| "null".into());
    let uid = |s: Option<&AttributionSourceState>| s.map_or(-1, |s| s.uid);
    let after = next(source);
    let mut message = format!(
        "Security exception for op {op} with source {}:{}, {}:{}",
        source.uid,
        name(Some(source)),
        uid(after.as_ref()),
        name(after.as_ref())
    );
    if let Some(after) = &after {
        let last = next(after);
        message.push_str(&format!(", {}:{}", name(last.as_ref()), uid(last.as_ref())));
    }
    format!("{message}:{}", e.message)
}

/// What the checks ask of the rest of the system server, as the caller
/// of `permission_checker` asks it (`AppOpsManager` in its binder call).
trait System {
    /// The protection level `getPermissionInfo(permission, 0)` returns,
    /// none for a permission not defined.
    fn protection(&self, permission: &str) -> Result<Option<i32>, Exception>;
    /// `Context.checkPermission(permission, INVALID_PID, uid)`.
    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception>;
    /// `getPackagesForUid`.
    fn packages_for_uid(&self, uid: i32) -> Result<Vec<String>, Exception>;
    /// `checkOpNoThrow`, also for `unsafeCheckOpRawNoThrow` (#1595).
    fn check_op(&self, op: i32, uid: i32, package: &str) -> Result<i32, Exception>;
    /// `noteOpNoThrow` of a source.
    fn note_op(&self, op: i32, source: &AttributionSourceState) -> Result<i32, Exception>;
    /// `noteProxyOpNoThrow` of a source and its next.
    fn note_proxy_op(&self, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<i32, Exception>;
    /// `startOpNoThrow` of a source by the client `token`, not at the
    /// default mode, attributed with the proxy's flags.
    fn start_op(
        &self,
        token: Option<Binder>,
        op: i32,
        source: &AttributionSourceState,
        attribution: &Attribution,
    ) -> Result<i32, Exception>;
    /// `startProxyOpNoThrow` of a source and its next by the client
    /// `token`.
    fn start_proxy_op(
        &self,
        token: Option<Binder>,
        op: i32,
        source: &AttributionSourceState,
        skip_proxy: bool,
        attribution: &Attribution,
    ) -> Result<i32, Exception>;
    /// `finishOp` of a source by the client `token`.
    fn finish_op(&self, token: Option<Binder>, op: i32, source: &AttributionSourceState) -> Result<(), Exception>;
    /// `finishProxyOp` of a source and its next by the client `token`.
    fn finish_proxy_op(&self, token: Option<Binder>, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<(), Exception>;
    /// `sRunningAttributionSources.put` of a `RegisteredAttribution`: the
    /// death of `source`'s token finishes `op`'s data delivery to it.
    fn register(&self, op: i32, source: &AttributionSourceState, from_datasource: bool) -> Result<(), Exception>;
    /// `sRunningAttributionSources.remove` of `token`, unregistered.
    fn unregister(&self, token: Option<&Binder>) -> Result<(), Exception>;
    /// An aconfig flag of the image.
    fn flag(&self, name: &str) -> Result<bool, Exception>;
    /// What is not implemented (#1527).
    fn unimplemented(&self, what: &str) -> Exception;
}

/// `PermissionCheckerService`'s checks over `system`.
struct Checks<'a, S: System>(&'a S);

impl<S: System> Checks<'_, S> {
    /// `AppOpsManager.permissionToOpCode`.
    fn permission_to_op(&self, permission: &str) -> Result<i32, Exception> {
        for (name, op, flag) in PERMISSION_OPS {
            if *name == permission && flag.map_or(Ok(true), |f| self.0.flag(f))? {
                return Ok(*op);
            }
        }
        if permission.starts_with(HEALTH_PERMISSION_PREFIX) {
            return Err(self.0.unimplemented("Health Connect permissions"));
        }
        Ok(OP_NONE)
    }

    /// A source of the default device: the app host has no virtual
    /// devices.
    fn default_device(&self, source: &AttributionSourceState) -> Result<(), Exception> {
        if source.device_id == DEVICE_ID_DEFAULT {
            return Ok(());
        }
        Err(self.0.unimplemented("a virtual device's permissions"))
    }

    /// `AttributionSource.isTrusted`: a source with a token that
    /// `isRegisteredAttributionSource`.
    fn trusted(&self, source: &AttributionSourceState) -> Result<bool, Exception> {
        match source.token {
            None => Ok(false),
            Some(_) => Err(self.0.unimplemented("trusted attribution sources (isRegisteredAttributionSource)")),
        }
    }

    /// `checkPermission` of one source: its uid holds the permission and
    /// did not renounce it. The app host runs no hotword detection
    /// service, whose isolated uid would hold its audio and camera ones.
    fn holds(&self, permission: &str, source: &AttributionSourceState) -> Result<bool, Exception> {
        self.default_device(source)?;
        let granted = self.0.holds(source.uid, permission)?;
        let renounced = source.renounced.iter().flatten().any(|p| p.as_deref() == Some(permission));
        if granted && renounced && self.0.holds(source.uid, RENOUNCE_PERMISSIONS)? {
            return Ok(false);
        }
        Ok(granted)
    }

    /// `resolvePackageName`: the source's package, else the uid's first,
    /// else `AppOpsManager.resolvePackageName`'s for a special uid.
    fn resolve_package(&self, source: &AttributionSourceState) -> Result<Option<String>, Exception> {
        if let Some(package) = &source.package {
            return Ok(Some(package.clone()));
        }
        if let Some(package) = self.0.packages_for_uid(source.uid)?.into_iter().next() {
            return Ok(Some(package));
        }
        Ok(match source.uid {
            0 => Some("root"),
            2000 => Some("com.android.shell"),
            1013 => Some("media"),
            1041 => Some("audioserver"),
            1047 => Some("cameraserver"),
            1000 => Some("android"),
            _ => None,
        }
        .map(String::from))
    }

    /// The attribution of a start by the link `i` of `chain` with the id
    /// `chain_id`: trusted where the chain's start is and the link's
    /// sources are registered (`isLinkTrusted`, asked only where a chain
    /// is attributed, `isTrusted` reading nothing else).
    #[allow(clippy::too_many_arguments)]
    fn link_attribution(
        &self,
        chain: &[&AttributionSourceState],
        i: usize,
        from_datasource: bool,
        self_access: bool,
        skip_current_checks: bool,
        chain_start_trusted: bool,
        chain_id: i32,
    ) -> Result<Attribution, Exception> {
        if chain_id == ATTRIBUTION_CHAIN_ID_NONE {
            return Ok(Attribution::NONE);
        }
        let next = chain.get(i + 1);
        let trusted = chain_start_trusted
            && (self.trusted(chain[i])? || i == 0)
            && match next {
                None => true,
                Some(next) => self.trusted(next)?,
            };
        let len = chain.len();
        let proxy_flags = if skip_current_checks {
            ATTRIBUTION_FLAGS_NONE
        } else {
            attribution_flags(len, Some(i), from_datasource, self_access, trusted, true)
        };
        let proxied_flags = attribution_flags(len, next.map(|_| i + 1), from_datasource, self_access, trusted, false);
        Ok(Attribution { proxy_flags, proxied_flags, chain_id })
    }

    /// `checkPermission`: an unknown permission is hard denied; app op and
    /// runtime permissions have their own checks, any other must be held
    /// by each source (but a datasource).
    fn check_permission(
        &self,
        permission: &str,
        chain: &[&AttributionSourceState],
        for_data_delivery: bool,
        start_data_delivery: bool,
        from_datasource: bool,
        attributed_op: i32,
    ) -> Result<i32, Exception> {
        let Some(protection) = self.0.protection(permission)? else { return Ok(HARD_DENIED) };
        if protection & PROTECTION_FLAG_APPOP != 0 {
            return self.check_app_op_permission(permission, chain, for_data_delivery, from_datasource);
        }
        if protection & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS {
            return self.check_runtime_permission(
                permission,
                chain,
                for_data_delivery,
                start_data_delivery,
                from_datasource,
                attributed_op,
            );
        }
        if !from_datasource && !self.holds(permission, chain[0])? {
            return Ok(HARD_DENIED);
        }
        if chain.len() > 1 {
            return self.check_permission(permission, &chain[1..], for_data_delivery, start_data_delivery, false, attributed_op);
        }
        Ok(GRANTED)
    }

    /// `checkAppOpPermission`: each link's op decides; at its default mode
    /// the permission does, of the first source and of the next.
    fn check_app_op_permission(
        &self,
        permission: &str,
        chain: &[&AttributionSourceState],
        for_data_delivery: bool,
        from_datasource: bool,
    ) -> Result<i32, Exception> {
        let op = self.permission_to_op(permission)?;
        if op < 0 {
            return Ok(HARD_DENIED);
        }
        for i in 0.. {
            let skip_current_checks = from_datasource || i > 0;
            let next = chain.get(i + 1);
            if !(from_datasource && i == 0) && next.is_some() && !self.trusted(chain[i])? {
                return Ok(HARD_DENIED);
            }
            let single_receiver = from_datasource && i == 0 && next.is_some() && chain.len() == 2;
            let self_access = single_receiver || next.is_none();
            let mode = self.perform_op_transaction(
                chain[0].token,
                op,
                &chain[i..],
                for_data_delivery,
                false,
                skip_current_checks,
                self_access,
                single_receiver,
                OP_NONE,
                &Attribution::NONE,
            )?;
            match mode {
                MODE_IGNORED | MODE_ERRORED => return Ok(HARD_DENIED),
                MODE_DEFAULT => {
                    if !skip_current_checks && !self.holds(permission, chain[0])? {
                        return Ok(HARD_DENIED);
                    }
                    if let Some(next) = next
                        && !self.holds(permission, next)?
                    {
                        return Ok(HARD_DENIED);
                    }
                }
                _ => {}
            }
            if chain.len() <= i + 2 {
                break;
            }
        }
        Ok(GRANTED)
    }

    /// `checkRuntimePermission`: each source (but a datasource) holds the
    /// permission and the op allows it, an ignored op soft denying it. A
    /// permission without an op (background location) needs only the
    /// grants. Each link that starts its delivery is registered, and a
    /// link that fails to start finishes the previous one's.
    fn check_runtime_permission(
        &self,
        permission: &str,
        chain: &[&AttributionSourceState],
        for_data_delivery: bool,
        start_data_delivery: bool,
        from_datasource: bool,
        attributed_op: i32,
    ) -> Result<i32, Exception> {
        let op = self.permission_to_op(permission)?;
        let chain_id = attribution_chain_id(start_data_delivery, chain);
        let chain_start_trusted = chain_id == ATTRIBUTION_CHAIN_ID_NONE || self.holds(UPDATE_APP_OPS_STATS, chain[0])?;
        for i in 0.. {
            let skip_current_checks = from_datasource || i > 0;
            let next = chain.get(i + 1);
            if !(from_datasource && i == 0) && next.is_some() && !self.trusted(chain[i])? {
                return Ok(HARD_DENIED);
            }
            if !skip_current_checks && !self.holds(permission, chain[i])? {
                return Ok(HARD_DENIED);
            }
            if let Some(next) = next
                && !self.holds(permission, next)?
            {
                return Ok(HARD_DENIED);
            }
            if op < 0 {
                if next.is_none() {
                    break;
                }
                continue;
            }
            let single_receiver = from_datasource && i == 0 && next.is_some() && chain.len() == 2;
            let self_access = single_receiver || next.is_none();
            let attribution =
                self.link_attribution(chain, i, from_datasource, self_access, skip_current_checks, chain_start_trusted, chain_id)?;
            let mode = self.perform_op_transaction(
                chain[0].token,
                op,
                &chain[i..],
                for_data_delivery,
                start_data_delivery,
                skip_current_checks,
                self_access,
                single_receiver,
                attributed_op,
                &attribution,
            )?;
            // The previous link's started proxy op, cut where this one
            // failed.
            if start_data_delivery && mode != MODE_ALLOWED && i > 0 {
                let mut cut = chain[i - 1].clone();
                if let Some(Some(next)) = cut.next.as_mut().and_then(|next| next.first_mut()) {
                    next.next = Some(Vec::new());
                }
                self.finish_data_delivery(attributed_op, &self::chain(&cut)?, from_datasource)?;
            }
            match mode {
                MODE_ERRORED => return Ok(HARD_DENIED),
                MODE_IGNORED => return Ok(SOFT_DENIED),
                _ => {}
            }
            if start_data_delivery {
                self.0.register(op, chain[i], from_datasource)?;
            }
            if chain.len() <= i + 2 {
                break;
            }
        }
        Ok(GRANTED)
    }

    /// `checkOp`: each link's op, an ignored one soft denying it.
    fn check_op(
        &self,
        op: i32,
        chain: &[&AttributionSourceState],
        for_data_delivery: bool,
        start_data_delivery: bool,
    ) -> Result<i32, Exception> {
        if op < 0 || chain[0].package.is_none() {
            return Ok(HARD_DENIED);
        }
        let chain_id = attribution_chain_id(start_data_delivery, chain);
        let chain_start_trusted = chain_id == ATTRIBUTION_CHAIN_ID_NONE || self.holds(UPDATE_APP_OPS_STATS, chain[0])?;
        for i in 0.. {
            let next = chain.get(i + 1);
            if next.is_some() && !self.trusted(chain[i])? {
                return Ok(HARD_DENIED);
            }
            let self_access = next.is_none();
            let attribution = self.link_attribution(chain, i, false, self_access, i > 0, chain_start_trusted, chain_id)?;
            let mode = self.perform_op_transaction(
                chain[i].token,
                op,
                &chain[i..],
                for_data_delivery,
                start_data_delivery,
                i > 0,
                self_access,
                false,
                OP_NONE,
                &attribution,
            )?;
            match mode {
                MODE_ERRORED => return Ok(HARD_DENIED),
                MODE_IGNORED => return Ok(SOFT_DENIED),
                _ => {}
            }
            if chain.len() <= i + 2 {
                break;
            }
        }
        Ok(GRANTED)
    }

    /// `performOpTransaction` for `source`, the first of the rest of the
    /// chain, whose start is the client `token`: for a preflight the
    /// accessor's op raw, and with it the next's when the accessor proxies
    /// (a proxy already vetted, but a single receiver, counts as allowed);
    /// for data delivery the accessor's op noted or started, a proxy op
    /// when it proxies (or when the caller may not blame the accessor
    /// alone: a SecurityException), an attributed op instead once the
    /// permission's op does not err. A source without a package is its
    /// uid's.
    #[allow(clippy::too_many_arguments)]
    fn perform_op_transaction(
        &self,
        token: Option<Binder>,
        op: i32,
        source: &[&AttributionSourceState],
        for_data_delivery: bool,
        start_data_delivery: bool,
        skip_proxy_operation: bool,
        self_access: bool,
        single_receiver_from_datasource: bool,
        attributed_op: i32,
        attribution: &Attribution,
    ) -> Result<i32, Exception> {
        let accessor = usize::from(single_receiver_from_datasource);
        let Some(package) = self.resolve_package(source[accessor])? else { return Ok(MODE_ERRORED) };
        self.default_device(source[accessor])?;
        if !for_data_delivery {
            let mode = if skip_proxy_operation && !single_receiver_from_datasource {
                MODE_ALLOWED
            } else {
                self.0.check_op(op, source[accessor].uid, &package)?
            };
            let Some(next) = source.get(accessor + 1).filter(|_| !self_access && mode == MODE_ALLOWED) else {
                return Ok(mode);
            };
            let Some(next_package) = self.resolve_package(next)? else { return Ok(MODE_ERRORED) };
            self.default_device(next)?;
            return self.0.check_op(op, next.uid, &next_package);
        }
        let resolved = AttributionSourceState { package: Some(package), ..source[accessor].clone() };
        let mut performed = op;
        let mut checked = MODE_ALLOWED;
        if attributed_op != OP_NONE && attributed_op != op {
            checked = self.0.check_op(op, resolved.uid, resolved.package.as_deref().unwrap_or_default())?;
            if checked == MODE_ERRORED {
                return Ok(checked);
            }
            performed = attributed_op;
        }
        let skip = skip_proxy_operation;
        let mode = if self_access {
            let own = if start_data_delivery {
                self.0.start_op(token, performed, &resolved, attribution)
            } else {
                self.0.note_op(performed, &resolved)
            };
            match own {
                // The datasource blames the app and itself; a start's proxy
                // op is the attributed one, as the original passes it.
                Err(e) if e.code == EX_SECURITY && start_data_delivery => {
                    self.0.start_proxy_op(token, attributed_op, source[0], skip, attribution)?
                }
                Err(e) if e.code == EX_SECURITY => self.0.note_proxy_op(performed, source[0], skip)?,
                own => own?,
            }
        } else {
            let proxied = if start_data_delivery {
                self.0.start_proxy_op(token, performed, &resolved, skip, attribution)
            } else {
                self.0.note_proxy_op(performed, &resolved, skip)
            };
            proxied.map_err(|e| match e.code {
                EX_SECURITY => Exception::security(security_message(performed, source[0], &e)),
                _ => e,
            })?
        };
        Ok(checked.max(mode))
    }

    /// `finishDataDelivery`: no op is nothing to finish; else each link's
    /// op is finished for the chain's client and the link unregistered,
    /// until an untrusted proxy or a source of no package ends the walk.
    fn finish_data_delivery(&self, op: i32, chain: &[&AttributionSourceState], from_datasource: bool) -> Result<(), Exception> {
        if op == OP_NONE {
            return Ok(());
        }
        let token = chain[0].token;
        for i in 0.. {
            let skip_current_finish = from_datasource || i > 0;
            let next = chain.get(i + 1).copied();
            if !(from_datasource && i == 0) && next.is_some() && !self.trusted(chain[i])? {
                return Ok(());
            }
            let single_receiver = from_datasource && i == 0 && next.is_some() && chain.len() == 2;
            let self_access = single_receiver || next.is_none();
            let accessor = if single_receiver { chain[i + 1] } else { chain[i] };
            let Some(package) = self.resolve_package(accessor)? else { return Ok(()) };
            let resolved = AttributionSourceState { package: Some(package), ..accessor.clone() };
            if self_access {
                self.0.finish_op(token, op, &resolved)?;
            } else {
                self.0.finish_proxy_op(token, op, &resolved, skip_current_finish)?;
            }
            self.0.unregister(chain[i].token.as_ref())?;
            if chain.len() <= i + 2 {
                if let Some(next) = next {
                    self.0.unregister(next.token.as_ref())?;
                }
                return Ok(());
            }
        }
        Ok(())
    }
}

/// The app host's system server, as `caller` asks it with `message`: the
/// native `permissionmgr` and `appops`.
struct Host<'a> {
    host: &'a AppHost,
    permissions: &'a PermissionManager,
    caller: i32,
    message: Option<&'a str>,
}

impl Host<'_> {
    fn appops_attribution(&self, attribution: &Attribution) -> appops_service::ProxyAttribution {
        appops_service::ProxyAttribution {
            start_if_mode_default: false,
            proxy_flags: attribution.proxy_flags,
            proxied_flags: attribution.proxied_flags,
            chain_id: attribution.chain_id,
        }
    }
}

impl System for Host<'_> {
    fn protection(&self, permission: &str) -> Result<Option<i32>, Exception> {
        self.permissions.protection_level(permission)
    }

    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        self.permissions.holds(uid, permission)
    }

    fn packages_for_uid(&self, uid: i32) -> Result<Vec<String>, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: 1000 };
        let packages = query
            .packages_for_uid(uid)
            .map_err(|NotModelled(e)| Exception::new(EX_UNSUPPORTED_OPERATION, format!("permission_checker: {e}")))?;
        Ok(packages.into_iter().flatten().flatten().collect())
    }

    fn check_op(&self, op: i32, uid: i32, package: &str) -> Result<i32, Exception> {
        appops_service::check_from(self.host, self.caller, op, uid, Some(package))
    }

    fn note_op(&self, op: i32, source: &AttributionSourceState) -> Result<i32, Exception> {
        appops_service::note_from(self.host, self.caller, op, source.uid, source.package.as_deref(), source.tag.as_deref())
    }

    fn note_proxy_op(&self, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<i32, Exception> {
        appops_service::note_proxy(self.host, self.caller, op, source, skip_proxy, self.message)
    }

    fn start_op(
        &self,
        token: Option<Binder>,
        op: i32,
        source: &AttributionSourceState,
        attribution: &Attribution,
    ) -> Result<i32, Exception> {
        let attribution =
            appops_service::Attribution { start_if_mode_default: false, flags: attribution.proxy_flags, chain_id: attribution.chain_id };
        let (package, tag) = (source.package.as_deref(), source.tag.as_deref());
        appops_service::start(self.host, self.caller, token, op, source.uid, package, tag, attribution)
    }

    fn start_proxy_op(
        &self,
        token: Option<Binder>,
        op: i32,
        source: &AttributionSourceState,
        skip_proxy: bool,
        attribution: &Attribution,
    ) -> Result<i32, Exception> {
        let attribution = self.appops_attribution(attribution);
        appops_service::start_proxy(self.host, self.caller, token, op, source, skip_proxy, attribution, self.message)
    }

    fn finish_op(&self, token: Option<Binder>, op: i32, source: &AttributionSourceState) -> Result<(), Exception> {
        let (package, tag) = (source.package.as_deref(), source.tag.as_deref());
        appops_service::finish(self.host, self.caller, token, op, source.uid, package, tag)
    }

    fn finish_proxy_op(&self, token: Option<Binder>, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<(), Exception> {
        appops_service::finish_proxy(self.host, self.caller, token, op, source, skip_proxy)
    }

    /// A token of the app host's own does not die; another's death finishes
    /// the delivery once, as the system server.
    fn register(&self, op: i32, source: &AttributionSourceState, from_datasource: bool) -> Result<(), Exception> {
        let token = source.token.ok_or_else(|| Exception::new(EX_NULL_POINTER, "token"))?;
        let mut running = self.host.running_attributions.lock().unwrap();
        let id = running.next;
        running.next += 1;
        let link = match token {
            Binder::Handle(handle) => {
                let strong = self.host.process.strong(handle);
                let (me, source) = (self.host.me.clone(), source.clone());
                let death = self.host.process.link_to_death(
                    &strong,
                    Box::new(move || {
                        let Some(host) = me.upgrade() else { return };
                        if host.running_attributions.lock().unwrap().registered.remove(&id).is_none() {
                            return;
                        }
                        let permissions = PermissionManager::new(host.clone());
                        let system = Host { host: &host, permissions: &permissions, caller: SYSTEM_UID, message: None };
                        if let Err(e) =
                            self::chain(&source).and_then(|chain| Checks(&system).finish_data_delivery(op, &chain, from_datasource))
                        {
                            eprintln!("app host: permission_checker finishDataDelivery at a death: {e:?}");
                        }
                    }),
                );
                Some((strong, death))
            }
            Binder::Local(_) => None,
        };
        running.registered.insert(id, link);
        running.by_token.insert(token, id);
        Ok(())
    }

    fn unregister(&self, token: Option<&Binder>) -> Result<(), Exception> {
        let token = token.ok_or_else(|| Exception::new(EX_NULL_POINTER, "token"))?;
        let mut running = self.host.running_attributions.lock().unwrap();
        if let Some(id) = running.by_token.remove(token)
            && let Some(Some((strong, death))) = running.registered.remove(&id)
        {
            self.host.process.clear_death(&strong, death);
        }
        Ok(())
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("permission_checker", &format!("{what} (#1527)"))
    }
}

/// The attribution sources whose data delivery started, each a
/// `RegisteredAttribution`: the latest of each token, and each one not
/// yet finished with the link to its token's death (one a later one
/// replaced stays linked, as the original's does).
#[derive(Default)]
pub(super) struct Running {
    by_token: HashMap<Binder, u64>,
    registered: HashMap<u64, Option<(Strong, u64)>>,
    next: u64,
}

/// `PermissionChecker.checkPermissionForPreflight` of `permission` for
/// `package` of `uid`, as the system asks it: granted?
pub(super) fn preflight(host: &AppHost, permission: &str, uid: i32, package: &str) -> Result<bool, Exception> {
    let me = host.me.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "app host gone"))?;
    let permissions = PermissionManager::new(me.clone());
    let system = Host { host: &me, permissions: &permissions, caller: SYSTEM_UID, message: None };
    let source = AttributionSourceState {
        pid: -1,
        uid,
        device_id: DEVICE_ID_DEFAULT,
        package: Some(package.to_string()),
        tag: None,
        token: None,
        renounced: None,
        next: None,
    };
    Ok(Checks(&system).check_permission(permission, &[&source], false, false, false, OP_NONE)? == GRANTED)
}

/// `PermissionManager.checkPermissionForDataDelivery` of `permission` for
/// the chain of `source`, as the system asks it: granted?
pub(super) fn data_delivery(host: &AppHost, permission: &str, source: &AttributionSourceState) -> Result<bool, Exception> {
    let me = host.me.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "app host gone"))?;
    let permissions = PermissionManager::new(me.clone());
    let system = Host { host: &me, permissions: &permissions, caller: SYSTEM_UID, message: None };
    Ok(Checks(&system).check_permission(permission, &chain(source)?, true, false, false, OP_NONE)? == GRANTED)
}

/// `PermissionManager.checkPermissionForDataDeliveryFromDataSource` of
/// `permission` for a system service, a datasource, on behalf of the chain
/// of `source`: the result (`PERMISSION_GRANTED`, `_SOFT_DENIED`,
/// `_HARD_DENIED`).
pub(super) fn data_delivery_from_datasource(host: &AppHost, permission: &str, source: &AttributionSourceState) -> Result<i32, Exception> {
    let me = host.me.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "app host gone"))?;
    let permissions = PermissionManager::new(me.clone());
    let system = Host { host: &me, permissions: &permissions, caller: SYSTEM_UID, message: None };
    let datasource = AttributionSourceState {
        pid: -1,
        uid: SYSTEM_UID,
        device_id: DEVICE_ID_DEFAULT,
        package: Some("android".into()),
        tag: None,
        token: None,
        renounced: None,
        next: None,
    };
    let mut links = vec![&datasource];
    links.extend(chain(source)?);
    Checks(&system).check_permission(permission, &links, true, false, true, OP_NONE)
}

/// `ForegroundServiceTypePolicy.RegularPermission.checkPermission` of
/// `permission` for `package` of `uid`: its preflight check
/// (`PermissionChecker.checkPermissionForPreflight`), then its op, raw,
/// if it has one; an op the caller holds only in the foreground grants it
/// while-in-use permissions (`allow_while_in_use`, the type enforcement
/// flag at its default), as does a soft denial of an ignored one. The app
/// host has no virtual devices to try the permission on.
pub(super) fn fgs_type_permission(
    host: &Arc<AppHost>,
    permission: &str,
    uid: i32,
    package: &str,
    allow_while_in_use: bool,
) -> Result<bool, Exception> {
    let permissions = PermissionManager::new(host.clone());
    let system = Host { host, permissions: &permissions, caller: SYSTEM_UID, message: None };
    let checks = Checks(&system);
    let source = AttributionSourceState {
        pid: -1,
        uid,
        device_id: DEVICE_ID_DEFAULT,
        package: Some(package.to_string()),
        tag: None,
        token: None,
        renounced: None,
        next: None,
    };
    let result = checks.check_permission(permission, &[&source], false, false, false, OP_NONE)?;
    if result == HARD_DENIED {
        return Ok(false);
    }
    let op = checks.permission_to_op(permission)?;
    if op == OP_NONE {
        return Ok(result == GRANTED);
    }
    Ok(match system.check_op(op, uid, package)? {
        MODE_ALLOWED => true,
        MODE_DEFAULT => result == GRANTED,
        MODE_FOREGROUND => allow_while_in_use,
        MODE_IGNORED => allow_while_in_use && result == SOFT_DENIED,
        _ => false,
    })
}

pub struct PermissionChecker {
    host: Arc<AppHost>,
    permissions: Arc<PermissionManager>,
}

impl PermissionChecker {
    pub fn new(host: Arc<AppHost>, permissions: Arc<PermissionManager>) -> Self {
        PermissionChecker { host, permissions }
    }
}

/// `Objects.requireNonNull` of an argument.
fn non_null<T>(value: Option<T>, name: &str) -> Result<T, Exception> {
    value.ok_or_else(|| Exception::new(EX_NULL_POINTER, name))
}

impl Service for PermissionChecker {
    fn descriptor(&self) -> &str {
        checker::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let caller = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            checker::CHECK_PERMISSION => {
                let a = checker::CheckPermission::<AttributionSourceState>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let system = Host { host: &self.host, permissions: &self.permissions, caller, message: a.message.as_deref() };
                let checks = Checks(&system);
                (|| {
                    let permission = non_null(a.permission.as_deref(), "permission")?;
                    let chain = chain(non_null(a.attribution_source.as_ref(), "attributionSource")?)?;
                    let (start, from_datasource) = (a.start_data_delivery, a.from_datasource);
                    let result =
                        checks.check_permission(permission, &chain, a.for_data_delivery, start, from_datasource, a.attributed_op)?;
                    // Finishes any op started before a link failed.
                    if start && result == HARD_DENIED {
                        let op = match a.attributed_op {
                            OP_NONE => checks.permission_to_op(permission)?,
                            op => op,
                        };
                        checks.finish_data_delivery(op, &chain, from_datasource)?;
                    }
                    Ok(result)
                })()
                .map(|result| checker::write_check_permission_reply(&mut p, result))
            }
            checker::CHECK_OP => {
                let a = checker::CheckOp::<AttributionSourceState>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let system = Host { host: &self.host, permissions: &self.permissions, caller, message: a.message.as_deref() };
                let checks = Checks(&system);
                (|| {
                    let chain = chain(non_null(a.attribution_source.as_ref(), "attributionSource")?)?;
                    let result = checks.check_op(a.op, &chain, a.for_data_delivery, a.start_data_delivery)?;
                    if a.start_data_delivery && result != GRANTED {
                        checks.finish_data_delivery(a.op, &chain, false)?;
                    }
                    Ok(result)
                })()
                .map(|result| checker::write_check_op_reply(&mut p, result))
            }
            checker::FINISH_DATA_DELIVERY => {
                let a = checker::FinishDataDelivery::<AttributionSourceState>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let system = Host { host: &self.host, permissions: &self.permissions, caller, message: None };
                non_null(a.attribution_source.as_ref(), "attributionSource")
                    .and_then(chain)
                    .and_then(|chain| Checks(&system).finish_data_delivery(a.op, &chain, a.from_datasource))
                    .map(|()| checker::write_finish_data_delivery_reply(&mut p))
            }
            code => Err(self.host.unimplemented("permission_checker", code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use super::*;

    const CAMERA: &str = "android.permission.CAMERA";
    const OP_CAMERA: i32 = 26;
    const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";
    const OP_RECORD_AUDIO: i32 = 27;
    const MANAGE_EXTERNAL_STORAGE: &str = "android.permission.MANAGE_EXTERNAL_STORAGE";
    const OP_MANAGE_EXTERNAL_STORAGE: i32 = 92;
    const BACKGROUND_LOCATION: &str = "android.permission.ACCESS_BACKGROUND_LOCATION";
    const INTERNET: &str = "android.permission.INTERNET";
    const PROVIDER: i32 = 10_001;
    const APP: i32 = 10_002;
    const OTHER: i32 = 10_003;

    /// What the checks asked of appops and the running sources: an op
    /// noted, proxy-noted (the proxy's uid, the proxied's, the skip),
    /// started, proxy-started, finished or proxy-finished, a source
    /// registered (the op, the uid) or a token unregistered.
    #[derive(Debug, PartialEq)]
    enum Asked {
        Note(i32, i32, String),
        NoteProxy(i32, i32, i32, bool),
        Start(i32, i32, Attribution),
        StartProxy(i32, i32, i32, bool, Attribution),
        Finish(i32, i32),
        FinishProxy(i32, i32, i32, bool),
        Register(i32, i32),
        Unregister(Binder),
    }

    /// A system server: the permissions' protections, each uid's grants,
    /// each (op, uid)'s mode, the uids the caller may not blame alone
    /// (a SecurityException of their own op, or of a proxy op they
    /// proxy), and what was asked.
    #[derive(Default)]
    struct Fake {
        protections: HashMap<&'static str, i32>,
        grants: HashMap<i32, Vec<&'static str>>,
        modes: HashMap<(i32, i32), i32>,
        blamed: HashSet<i32>,
        asked: RefCell<Vec<Asked>>,
    }

    impl Fake {
        fn mode(&self, op: i32, uid: i32) -> i32 {
            self.modes.get(&(op, uid)).copied().unwrap_or(MODE_ALLOWED)
        }

        fn own(&self, uid: i32) -> Result<(), Exception> {
            match self.blamed.contains(&uid) {
                true => Err(Exception::security("denied")),
                false => Ok(()),
            }
        }

        fn next_uid(source: &AttributionSourceState) -> i32 {
            source.next.as_deref().and_then(<[_]>::first).cloned().flatten().map_or(-1, |n| n.uid)
        }

        fn proxy_mode(&self, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<i32, Exception> {
            self.own(source.uid)?;
            match skip_proxy {
                false if self.mode(op, source.uid) != MODE_ALLOWED => Ok(self.mode(op, source.uid)),
                _ => Ok(self.mode(op, Self::next_uid(source))),
            }
        }
    }

    impl System for Fake {
        fn protection(&self, permission: &str) -> Result<Option<i32>, Exception> {
            Ok(self.protections.get(permission).copied())
        }

        fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
            Ok(self.grants.get(&uid).is_some_and(|g| g.contains(&permission)))
        }

        fn packages_for_uid(&self, uid: i32) -> Result<Vec<String>, Exception> {
            Ok(vec![format!("uid.{uid}")])
        }

        fn check_op(&self, op: i32, uid: i32, _: &str) -> Result<i32, Exception> {
            Ok(self.mode(op, uid))
        }

        fn note_op(&self, op: i32, source: &AttributionSourceState) -> Result<i32, Exception> {
            self.own(source.uid)?;
            self.asked.borrow_mut().push(Asked::Note(op, source.uid, source.package.clone().unwrap()));
            Ok(self.mode(op, source.uid))
        }

        fn note_proxy_op(&self, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<i32, Exception> {
            let mode = self.proxy_mode(op, source, skip_proxy)?;
            self.asked.borrow_mut().push(Asked::NoteProxy(op, source.uid, Self::next_uid(source), skip_proxy));
            Ok(mode)
        }

        fn start_op(
            &self,
            _: Option<Binder>,
            op: i32,
            source: &AttributionSourceState,
            attribution: &Attribution,
        ) -> Result<i32, Exception> {
            self.own(source.uid)?;
            self.asked.borrow_mut().push(Asked::Start(op, source.uid, *attribution));
            Ok(self.mode(op, source.uid))
        }

        fn start_proxy_op(
            &self,
            _: Option<Binder>,
            op: i32,
            source: &AttributionSourceState,
            skip_proxy: bool,
            attribution: &Attribution,
        ) -> Result<i32, Exception> {
            let mode = self.proxy_mode(op, source, skip_proxy)?;
            self.asked.borrow_mut().push(Asked::StartProxy(op, source.uid, Self::next_uid(source), skip_proxy, *attribution));
            Ok(mode)
        }

        fn finish_op(&self, _: Option<Binder>, op: i32, source: &AttributionSourceState) -> Result<(), Exception> {
            self.asked.borrow_mut().push(Asked::Finish(op, source.uid));
            Ok(())
        }

        fn finish_proxy_op(&self, _: Option<Binder>, op: i32, source: &AttributionSourceState, skip_proxy: bool) -> Result<(), Exception> {
            self.asked.borrow_mut().push(Asked::FinishProxy(op, source.uid, Self::next_uid(source), skip_proxy));
            Ok(())
        }

        fn register(&self, op: i32, source: &AttributionSourceState, _: bool) -> Result<(), Exception> {
            source.token.ok_or_else(|| Exception::new(EX_NULL_POINTER, "token"))?;
            self.asked.borrow_mut().push(Asked::Register(op, source.uid));
            Ok(())
        }

        fn unregister(&self, token: Option<&Binder>) -> Result<(), Exception> {
            let token = token.ok_or_else(|| Exception::new(EX_NULL_POINTER, "token"))?;
            self.asked.borrow_mut().push(Asked::Unregister(*token));
            Ok(())
        }

        fn flag(&self, _: &str) -> Result<bool, Exception> {
            Ok(false)
        }

        fn unimplemented(&self, what: &str) -> Exception {
            Exception::new(EX_UNSUPPORTED_OPERATION, what)
        }
    }

    fn fake() -> Fake {
        Fake {
            protections: HashMap::from([
                (CAMERA, PROTECTION_DANGEROUS),
                (RECORD_AUDIO, PROTECTION_DANGEROUS),
                (BACKGROUND_LOCATION, PROTECTION_DANGEROUS),
                (MANAGE_EXTERNAL_STORAGE, 2 | PROTECTION_FLAG_APPOP),
                (INTERNET, 0),
            ]),
            grants: HashMap::from([
                (PROVIDER, vec![INTERNET]),
                (APP, vec![CAMERA, RECORD_AUDIO, BACKGROUND_LOCATION, INTERNET]),
                (OTHER, vec![RECORD_AUDIO]),
            ]),
            ..Fake::default()
        }
    }

    fn source(uid: i32, package: &str) -> AttributionSourceState {
        AttributionSourceState {
            pid: -1,
            uid,
            device_id: 0,
            package: Some(package.into()),
            tag: None,
            token: None,
            renounced: None,
            next: None,
        }
    }

    /// A source with its token, as an app's `AttributionSource` has.
    fn tokened(uid: i32, package: &str) -> AttributionSourceState {
        AttributionSourceState { token: Some(Binder::Handle(uid as u32)), ..source(uid, package) }
    }

    /// `ContentProvider.checkPermission`'s chain: the provider, then its
    /// caller.
    fn from_provider(caller: AttributionSourceState) -> AttributionSourceState {
        AttributionSourceState { next: Some(vec![Some(caller)]), ..tokened(PROVIDER, "provider") }
    }

    fn check(system: &Fake, permission: &str, source: &AttributionSourceState, from_datasource: bool) -> Result<i32, Exception> {
        Checks(system).check_permission(permission, &chain(source).unwrap(), true, false, from_datasource, OP_NONE)
    }

    fn start(system: &Fake, permission: &str, source: &AttributionSourceState, from_datasource: bool) -> Result<i32, Exception> {
        Checks(system).check_permission(permission, &chain(source).unwrap(), true, true, from_datasource, OP_NONE)
    }

    fn noted(op: i32, uid: i32, package: &str) -> Asked {
        Asked::Note(op, uid, package.into())
    }

    /// `AttributionSourceState` as the Java backend writes it: its size,
    /// pid, uid, deviceId, package, tag, token, renounced permissions and
    /// `next`, each element of which is one too.
    #[test]
    fn reads_the_parcel_the_java_backend_writes() {
        fn structured(p: &mut Parcel, fields: impl FnOnce(&mut Parcel)) {
            let start = p.position();
            p.write_i32(0);
            fields(p);
            p.set_i32_at(start, (p.position() - start) as i32);
        }
        let mut p = Parcel::new();
        p.write_i32(1);
        structured(&mut p, |p| {
            p.write_i32(-1);
            p.write_i32(PROVIDER);
            p.write_i32(0);
            p.write_string16(Some("provider"));
            p.write_string16(None);
            p.write_binder(Some(Binder::Local(0x1000)));
            p.write_i32(-1);
            p.write_i32(1);
            p.write_i32(1);
            structured(p, |p| {
                p.write_i32(4242);
                p.write_i32(APP);
                p.write_i32(0);
                p.write_string16(Some("app"));
                p.write_string16(Some("tag"));
                p.write_binder(None);
                p.write_i32(1);
                p.write_string16(Some(CAMERA));
                p.write_i32(-1);
                // A field of a later version, skipped by the size.
                p.write_i32(7);
            });
        });
        p.write_i32(9);
        let mut r = p.reader();
        let state = aim_service_aidl::read_typed::<AttributionSourceState>(&mut r).unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 9);
        assert_eq!(state.token, Some(Binder::Local(0x1000)));
        assert_eq!(state.renounced, None);
        let app =
            AttributionSourceState { pid: 4242, tag: Some("tag".into()), renounced: Some(vec![Some(CAMERA.into())]), ..source(APP, "app") };
        assert_eq!(state.next, Some(vec![Some(app.clone())]));
        assert_eq!(chain(&state).unwrap(), [&state, &app]);
        let broken = AttributionSourceState { next: Some(vec![None]), ..source(APP, "app") };
        assert_eq!(chain(&broken).unwrap_err().code, EX_NULL_POINTER);
    }

    /// CtsPermissionTestCases' `PermissionCheckerTest`-style checks of
    /// one source: a granted runtime permission with its op allowed is
    /// granted and noted; an ignored op soft denies it; an errored op or a
    /// permission not held hard denies it; an unknown permission is hard
    /// denied.
    #[test]
    fn runtime_permissions_of_one_source() {
        let mut system = fake();
        let app = source(APP, "app");
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), GRANTED);
        assert_eq!(*system.asked.borrow(), [noted(OP_CAMERA, APP, "app")]);
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), SOFT_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_ERRORED);
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), HARD_DENIED);
        assert_eq!(check(&system, CAMERA, &source(OTHER, "other"), false).unwrap(), HARD_DENIED);
        assert_eq!(check(&system, "no.such.permission", &app, false).unwrap(), HARD_DENIED);
        // A preflight checks the op without noting it.
        system.asked.borrow_mut().clear();
        let checks = Checks(&system);
        assert_eq!(checks.check_permission(RECORD_AUDIO, &[&app], false, false, false, OP_NONE).unwrap(), GRANTED);
        assert!(system.asked.borrow().is_empty());
        // Background location has no op: the grant decides.
        assert_eq!(check(&system, BACKGROUND_LOCATION, &app, false).unwrap(), GRANTED);
    }

    /// A renounced permission is held only without RENOUNCE_PERMISSIONS.
    #[test]
    fn renounced_permissions() {
        let mut system = fake();
        let app = AttributionSourceState { renounced: Some(vec![Some(CAMERA.into())]), ..source(APP, "app") };
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), GRANTED);
        system.grants.get_mut(&APP).unwrap().push(RENOUNCE_PERMISSIONS);
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), HARD_DENIED);
    }

    /// A provider's chain from the datasource: the provider is not
    /// vetted, its caller holds the permission and its op is noted for
    /// the caller, the single receiver; a provider that may not blame its
    /// caller alone notes a proxy op of both.
    #[test]
    fn datasource_chains() {
        let mut system = fake();
        let chain = from_provider(source(APP, "app"));
        assert_eq!(check(&system, CAMERA, &chain, true).unwrap(), GRANTED);
        assert_eq!(*system.asked.borrow(), [noted(OP_CAMERA, APP, "app")]);
        assert_eq!(check(&system, CAMERA, &from_provider(source(OTHER, "other")), true).unwrap(), HARD_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(check(&system, CAMERA, &chain, true).unwrap(), SOFT_DENIED);
        // A caller without a package is its uid's.
        system.modes.clear();
        system.asked.borrow_mut().clear();
        let anonymous = from_provider(AttributionSourceState { package: None, ..source(APP, "app") });
        assert_eq!(check(&system, CAMERA, &anonymous, true).unwrap(), GRANTED);
        assert_eq!(*system.asked.borrow(), [noted(OP_CAMERA, APP, &format!("uid.{APP}"))]);
        // A permission without an op: each source after the datasource
        // holds it.
        assert_eq!(check(&system, INTERNET, &chain, true).unwrap(), GRANTED);
        assert_eq!(check(&system, INTERNET, &source(OTHER, "other"), false).unwrap(), HARD_DENIED);
        system.asked.borrow_mut().clear();
        system.blamed.insert(APP);
        assert_eq!(check(&system, CAMERA, &chain, true).unwrap(), GRANTED);
        assert_eq!(*system.asked.borrow(), [Asked::NoteProxy(OP_CAMERA, PROVIDER, APP, true)]);
    }

    /// Not from a datasource, a proxy must be trusted: one without a
    /// token is not, so the chain is hard denied.
    #[test]
    fn untrusted_proxies_are_hard_denied() {
        let system = fake();
        let proxied = AttributionSourceState { next: Some(vec![Some(source(OTHER, "other"))]), ..source(APP, "app") };
        assert_eq!(check(&system, RECORD_AUDIO, &proxied, false).unwrap(), HARD_DENIED);
        let checks = Checks(&system);
        assert_eq!(checks.check_op(OP_RECORD_AUDIO, &chain(&proxied).unwrap(), true, false).unwrap(), HARD_DENIED);
        let trusted = AttributionSourceState { token: Some(Binder::Handle(3)), ..proxied };
        assert!(check(&system, RECORD_AUDIO, &trusted, false).is_err());
    }

    /// From a datasource, the first link proxies for the rest of the
    /// chain; an intermediary without a registered token ends it.
    #[test]
    fn proxied_chains_from_a_datasource() {
        let system = fake();
        let middle = AttributionSourceState { next: Some(vec![Some(source(APP, "app"))]), ..source(OTHER, "other") };
        let chain = from_provider(middle);
        assert_eq!(check(&system, RECORD_AUDIO, &chain, true).unwrap(), HARD_DENIED);
        assert_eq!(*system.asked.borrow(), [Asked::NoteProxy(OP_RECORD_AUDIO, PROVIDER, OTHER, true)]);
        // A proxy op the caller may not make is rethrown with the chain.
        let mut system = fake();
        system.blamed.insert(PROVIDER);
        let denied = check(&system, RECORD_AUDIO, &chain, true).unwrap_err();
        assert_eq!(denied.code, EX_SECURITY);
        assert_eq!(denied.message, "Security exception for op 27 with source 10001:provider, 10003:other, app:10002:denied");
    }

    /// An app op permission: the op decides, at its default mode the
    /// permission does.
    #[test]
    fn app_op_permissions() {
        let mut system = fake();
        let app = source(APP, "app");
        assert_eq!(check(&system, MANAGE_EXTERNAL_STORAGE, &app, false).unwrap(), GRANTED);
        system.modes.insert((OP_MANAGE_EXTERNAL_STORAGE, APP), MODE_IGNORED);
        assert_eq!(check(&system, MANAGE_EXTERNAL_STORAGE, &app, false).unwrap(), HARD_DENIED);
        system.modes.insert((OP_MANAGE_EXTERNAL_STORAGE, APP), MODE_DEFAULT);
        assert_eq!(check(&system, MANAGE_EXTERNAL_STORAGE, &app, false).unwrap(), HARD_DENIED);
        system.grants.get_mut(&APP).unwrap().push(MANAGE_EXTERNAL_STORAGE);
        assert_eq!(check(&system, MANAGE_EXTERNAL_STORAGE, &app, false).unwrap(), GRANTED);
    }

    /// `checkOp`: no package or no op is hard denied; the mode decides.
    #[test]
    fn check_op() {
        let mut system = fake();
        let app = source(APP, "app");
        let anonymous = AttributionSourceState { package: None, ..source(APP, "app") };
        let checks =
            |system: &Fake, op: i32, s: &AttributionSourceState| Checks(system).check_op(op, &chain(s).unwrap(), false, false).unwrap();
        assert_eq!(checks(&system, OP_CAMERA, &app), GRANTED);
        assert_eq!(checks(&system, OP_NONE, &app), HARD_DENIED);
        assert_eq!(checks(&system, OP_CAMERA, &anonymous), HARD_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(checks(&system, OP_CAMERA, &app), SOFT_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_ERRORED);
        assert_eq!(checks(&system, OP_CAMERA, &app), HARD_DENIED);
    }

    /// `startDataDelivery` of one source starts its op, unattributed, and
    /// registers it; `finishDataDelivery` finishes the op and unregisters
    /// the token. An ignored op is soft denied and not registered.
    #[test]
    fn started_delivery_of_one_source() {
        let mut system = fake();
        let app = tokened(APP, "app");
        assert_eq!(start(&system, CAMERA, &app, false).unwrap(), GRANTED);
        let checks = Checks(&system);
        checks.finish_data_delivery(OP_CAMERA, &[&app], false).unwrap();
        let token = app.token.unwrap();
        let expected = [
            Asked::Start(OP_CAMERA, APP, Attribution::NONE),
            Asked::Register(OP_CAMERA, APP),
            Asked::Finish(OP_CAMERA, APP),
            Asked::Unregister(token),
        ];
        assert_eq!(*system.asked.borrow(), expected);
        assert!(checks.finish_data_delivery(OP_NONE, &[&app], false).is_ok());
        system.asked.borrow_mut().clear();
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(start(&system, CAMERA, &app, false).unwrap(), SOFT_DENIED);
        assert_eq!(*system.asked.borrow(), [Asked::Start(OP_CAMERA, APP, Attribution::NONE)]);
        // A source without a token cannot be registered (a null key).
        system.modes.clear();
        assert_eq!(start(&system, CAMERA, &source(APP, "app"), false).unwrap_err().code, EX_NULL_POINTER);
    }

    /// A provider's started delivery to its caller: a chain, attributed to
    /// the caller as its untrusted receiver, the provider registered; a
    /// provider that may not blame its caller alone starts a proxy op of
    /// the attributed op, as the original passes it. Finishing unregisters
    /// both.
    #[test]
    fn started_delivery_from_a_datasource() {
        let mut system = fake();
        let chain = from_provider(tokened(APP, "app"));
        assert_eq!(start(&system, CAMERA, &chain, true).unwrap(), GRANTED);
        let asked = std::mem::take(&mut *system.asked.borrow_mut());
        let Asked::Start(OP_CAMERA, APP, attribution) = asked[0] else { panic!("{asked:?}") };
        assert_eq!((attribution.proxy_flags, attribution.proxied_flags), (ATTRIBUTION_FLAGS_NONE, ATTRIBUTION_FLAG_RECEIVER));
        assert_ne!(attribution.chain_id, ATTRIBUTION_CHAIN_ID_NONE);
        assert_eq!(asked[1..], [Asked::Register(OP_CAMERA, PROVIDER)]);
        Checks(&system).finish_data_delivery(OP_CAMERA, &self::chain(&chain).unwrap(), true).unwrap();
        let unregistered = [Asked::Unregister(Binder::Handle(PROVIDER as u32)), Asked::Unregister(Binder::Handle(APP as u32))];
        assert_eq!(system.asked.borrow()[0], Asked::Finish(OP_CAMERA, APP));
        assert_eq!(system.asked.borrow()[1..], unregistered);
        system.asked.borrow_mut().clear();
        system.blamed.insert(APP);
        assert_eq!(start(&system, CAMERA, &chain, true).unwrap(), GRANTED);
        assert!(matches!(system.asked.borrow()[0], Asked::StartProxy(OP_NONE, PROVIDER, APP, true, _)));
    }

    /// `resolveAttributionFlags`: the accessor, intermediaries and the
    /// receiver of a chain of three, and of one from a datasource.
    #[test]
    fn attribution_flags_of_links() {
        let trusted = ATTRIBUTION_FLAG_TRUSTED;
        assert_eq!(attribution_flags(3, Some(0), false, false, true, true), trusted | ATTRIBUTION_FLAG_ACCESSOR);
        assert_eq!(attribution_flags(3, Some(1), false, false, true, false), trusted | ATTRIBUTION_FLAG_INTERMEDIARY);
        assert_eq!(attribution_flags(3, Some(2), false, false, false, false), ATTRIBUTION_FLAG_RECEIVER);
        assert_eq!(attribution_flags(3, Some(1), true, false, false, false), ATTRIBUTION_FLAG_ACCESSOR);
        assert_eq!(attribution_flags(3, Some(0), true, false, true, true), ATTRIBUTION_FLAGS_NONE);
        assert_eq!(attribution_flags(2, Some(1), true, true, false, false), ATTRIBUTION_FLAG_RECEIVER);
        assert_eq!(attribution_flags(2, None, false, false, true, false), ATTRIBUTION_FLAGS_NONE);
        assert_eq!(attribution_chain_id(false, &[]), ATTRIBUTION_CHAIN_ID_NONE);
    }

    /// A virtual device's permissions are not implemented.
    #[test]
    fn not_implemented() {
        let system = fake();
        let virtual_device = AttributionSourceState { device_id: 5, ..source(APP, "app") };
        assert!(check(&system, CAMERA, &virtual_device, false).is_err());
    }

    /// `permissionToOpCode`: the table, feature-flagged entries off with
    /// their flag, Health Connect's other permissions not implemented.
    #[test]
    fn permission_to_op() {
        let system = fake();
        let checks = Checks(&system);
        assert_eq!(checks.permission_to_op(CAMERA).unwrap(), OP_CAMERA);
        assert_eq!(checks.permission_to_op("android.permission.WRITE_SETTINGS").unwrap(), 23);
        assert_eq!(checks.permission_to_op(INTERNET).unwrap(), OP_NONE);
        assert_eq!(checks.permission_to_op("android.permission.RANGING").unwrap(), OP_NONE);
        assert!(checks.permission_to_op("android.permission.health.READ_STEPS").is_err());
    }
}
