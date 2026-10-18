//! `permission_checker`: PermissionManagerService's
//! `PermissionCheckerService` (`IPermissionChecker`) for the app host, at
//! `android-16.0.0_r1` (ADR 0013): `checkPermission` and `checkOp` of an
//! attribution chain (`AttributionSourceState` and its `next`), which
//! `PermissionChecker` asks for permissions with an app op. Every uid in
//! the chain must hold the permission (as the native `permissionmgr`
//! answers `checkUidPermission`, without a renounced one); a runtime
//! permission's op must be allowed and an app op permission's op decides,
//! falling back to the permission at its default mode; the op is checked
//! raw for a preflight and noted for data delivery by the app host's
//! `appops`, which errs for an op it does not decide. A datasource vets
//! only the chain after it, and only the accessor's own op is noted.
//! Started ops (`startDataDelivery`, `finishDataDelivery` of an op),
//! proxy ops, trusted (registered) attribution sources, a virtual
//! device's permissions and Health Connect's are not implemented (#1527).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_permission_ipermissionchecker as checker;
use aim_service_aidl::{ReadParcelable, read_string_list, read_typed_list};

use super::display::parcelable;
use super::permission_manager::PermissionManager;
use super::{AppHost, appops_service, reply};
use crate::package::apps_filter::NotModelled;
use crate::package::query::Query;

/// `PermissionCheckerManager.PERMISSION_GRANTED`, `PERMISSION_SOFT_DENIED`,
/// `PERMISSION_HARD_DENIED`.
const GRANTED: i32 = 0;
const SOFT_DENIED: i32 = 1;
const HARD_DENIED: i32 = 2;
/// `AppOpsManager.OP_NONE`, `MODE_ALLOWED`, `MODE_IGNORED`,
/// `MODE_ERRORED`, `MODE_DEFAULT`.
const OP_NONE: i32 = -1;
const MODE_ALLOWED: i32 = 0;
const MODE_IGNORED: i32 = 1;
const MODE_ERRORED: i32 = 2;
const MODE_DEFAULT: i32 = 3;
/// `PermissionInfo.PROTECTION_MASK_BASE`, `PROTECTION_DANGEROUS`,
/// `PROTECTION_FLAG_APPOP`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_FLAG_APPOP: i32 = 0x40;
/// `Context.DEVICE_ID_DEFAULT`.
const DEVICE_ID_DEFAULT: i32 = 0;
const RENOUNCE_PERMISSIONS: &str = "android.permission.RENOUNCE_PERMISSIONS";
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

/// What the checks ask of the rest of the system server.
trait System {
    /// The protection level `getPermissionInfo(permission, 0)` returns,
    /// none for a permission not defined.
    fn protection(&self, permission: &str) -> Result<Option<i32>, Exception>;
    /// `Context.checkPermission(permission, INVALID_PID, uid)`.
    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception>;
    /// `getPackagesForUid`.
    fn packages_for_uid(&self, uid: i32) -> Result<Vec<String>, Exception>;
    /// `checkOpNoThrow` and `unsafeCheckOpRawNoThrow`, which the app
    /// host's ops answer alike: none has a foreground mode.
    fn check_op(&self, op: i32, uid: i32, package: &str) -> Result<i32, Exception>;
    /// `noteOpNoThrow`.
    fn note_op(&self, op: i32, uid: i32, package: &str) -> Result<i32, Exception>;
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
            return self.check_permission(
                permission,
                &chain[1..],
                for_data_delivery,
                start_data_delivery,
                false,
                attributed_op,
            );
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
                op,
                &chain[i..],
                for_data_delivery,
                false,
                skip_current_checks,
                self_access,
                single_receiver,
                OP_NONE,
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
    /// grants.
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
            // Starting an op is not implemented, so with
            // `startDataDelivery` this fails before any op is started that
            // a denial would finish or a source's death unregister.
            let mode = self.perform_op_transaction(
                op,
                &chain[i..],
                for_data_delivery,
                start_data_delivery,
                skip_current_checks,
                self_access,
                single_receiver,
                attributed_op,
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
        for i in 0.. {
            let next = chain.get(i + 1);
            if next.is_some() && !self.trusted(chain[i])? {
                return Ok(HARD_DENIED);
            }
            let mode = self.perform_op_transaction(
                op,
                &chain[i..],
                for_data_delivery,
                start_data_delivery,
                i > 0,
                next.is_none(),
                false,
                OP_NONE,
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
    /// chain: for a preflight the accessor's op raw, and with it the
    /// next's when the accessor proxies (a proxy already vetted, but a
    /// single receiver, counts as allowed); for data delivery the op
    /// noted for the accessor, an attributed op instead once the
    /// permission's op does not err. A source without a package is its
    /// uid's.
    #[allow(clippy::too_many_arguments)]
    fn perform_op_transaction(
        &self,
        op: i32,
        source: &[&AttributionSourceState],
        for_data_delivery: bool,
        start_data_delivery: bool,
        skip_proxy_operation: bool,
        self_access: bool,
        single_receiver_from_datasource: bool,
        attributed_op: i32,
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
        let mut performed = op;
        let mut checked = MODE_ALLOWED;
        if attributed_op != OP_NONE && attributed_op != op {
            checked = self.0.check_op(op, source[accessor].uid, &package)?;
            if checked == MODE_ERRORED {
                return Ok(checked);
            }
            performed = attributed_op;
        }
        if start_data_delivery {
            return Err(self.0.unimplemented("started ops (startDataDelivery)"));
        }
        if !self_access {
            return Err(self.0.unimplemented("proxy ops (noteProxyOperation)"));
        }
        Ok(checked.max(self.0.note_op(performed, source[accessor].uid, &package)?))
    }

    /// `finishDataDelivery`: no op is nothing to finish; else the first
    /// link's op is finished, unless an untrusted proxy or a source of no
    /// package ends the walk first. The app host keeps no started ops.
    fn finish_data_delivery(
        &self,
        op: i32,
        chain: &[&AttributionSourceState],
        from_datasource: bool,
    ) -> Result<(), Exception> {
        if op == OP_NONE {
            return Ok(());
        }
        let next = chain.get(1);
        if !from_datasource && next.is_some() && !self.trusted(chain[0])? {
            return Ok(());
        }
        let single_receiver = from_datasource && next.is_some() && chain.len() == 2;
        if self.resolve_package(chain[usize::from(single_receiver)])?.is_none() {
            return Ok(());
        }
        Err(self.0.unimplemented("started ops (finishOp, finishProxyOp)"))
    }
}

/// The app host's system server: the native `permissionmgr` and `appops`.
struct Host<'a> {
    host: &'a AppHost,
    permissions: &'a PermissionManager,
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
        appops_service::mode(self.host, op, uid, Some(package))
    }

    fn note_op(&self, op: i32, uid: i32, package: &str) -> Result<i32, Exception> {
        appops_service::note(self.host, op, uid, Some(package))
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("permission_checker", &format!("{what} (#1527)"))
    }
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
        let system = Host { host: &self.host, permissions: &self.permissions };
        let checks = Checks(&system);
        let mut p = Parcel::new();
        let result = match call.code {
            checker::CHECK_PERMISSION => {
                let a = checker::CheckPermission::<AttributionSourceState>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let permission = non_null(a.permission.as_deref(), "permission")?;
                    let chain = chain(non_null(a.attribution_source.as_ref(), "attributionSource")?)?;
                    let (start, from_datasource) = (a.start_data_delivery, a.from_datasource);
                    let result = checks.check_permission(
                        permission,
                        &chain,
                        a.for_data_delivery,
                        start,
                        from_datasource,
                        a.attributed_op,
                    )?;
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
                let a = checker::FinishDataDelivery::<AttributionSourceState>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                non_null(a.attribution_source.as_ref(), "attributionSource")
                    .and_then(chain)
                    .and_then(|chain| checks.finish_data_delivery(a.op, &chain, a.from_datasource))
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
    use std::collections::HashMap;

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

    /// A system server: the permissions' protections, each uid's grants,
    /// each (op, uid)'s mode, and the ops noted.
    #[derive(Default)]
    struct Fake {
        protections: HashMap<&'static str, i32>,
        grants: HashMap<i32, Vec<&'static str>>,
        modes: HashMap<(i32, i32), i32>,
        noted: RefCell<Vec<(i32, i32, String)>>,
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
            Ok(self.modes.get(&(op, uid)).copied().unwrap_or(MODE_ALLOWED))
        }

        fn note_op(&self, op: i32, uid: i32, package: &str) -> Result<i32, Exception> {
            self.noted.borrow_mut().push((op, uid, package.to_string()));
            self.check_op(op, uid, package)
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

    /// `ContentProvider.checkPermission`'s chain: the provider, then its
    /// caller.
    fn from_provider(caller: AttributionSourceState) -> AttributionSourceState {
        AttributionSourceState { next: Some(vec![Some(caller)]), ..source(PROVIDER, "provider") }
    }

    fn check(system: &Fake, permission: &str, source: &AttributionSourceState, from_datasource: bool) -> Result<i32, Exception> {
        Checks(system).check_permission(permission, &chain(source).unwrap(), true, false, from_datasource, OP_NONE)
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
        let app = AttributionSourceState {
            pid: 4242,
            tag: Some("tag".into()),
            renounced: Some(vec![Some(CAMERA.into())]),
            ..source(APP, "app")
        };
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
        assert_eq!(*system.noted.borrow(), [(OP_CAMERA, APP, "app".to_string())]);
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), SOFT_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_ERRORED);
        assert_eq!(check(&system, CAMERA, &app, false).unwrap(), HARD_DENIED);
        assert_eq!(check(&system, CAMERA, &source(OTHER, "other"), false).unwrap(), HARD_DENIED);
        assert_eq!(check(&system, "no.such.permission", &app, false).unwrap(), HARD_DENIED);
        // A preflight checks the op without noting it.
        system.noted.borrow_mut().clear();
        let checks = Checks(&system);
        assert_eq!(checks.check_permission(RECORD_AUDIO, &[&app], false, false, false, OP_NONE).unwrap(), GRANTED);
        assert!(system.noted.borrow().is_empty());
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
    /// the caller, the single receiver.
    #[test]
    fn datasource_chains() {
        let mut system = fake();
        let chain = from_provider(source(APP, "app"));
        assert_eq!(check(&system, CAMERA, &chain, true).unwrap(), GRANTED);
        assert_eq!(*system.noted.borrow(), [(OP_CAMERA, APP, "app".to_string())]);
        assert_eq!(check(&system, CAMERA, &from_provider(source(OTHER, "other")), true).unwrap(), HARD_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(check(&system, CAMERA, &chain, true).unwrap(), SOFT_DENIED);
        // A caller without a package is its uid's.
        system.modes.clear();
        system.noted.borrow_mut().clear();
        let anonymous = from_provider(AttributionSourceState { package: None, ..source(APP, "app") });
        assert_eq!(check(&system, CAMERA, &anonymous, true).unwrap(), GRANTED);
        assert_eq!(*system.noted.borrow(), [(OP_CAMERA, APP, format!("uid.{APP}"))]);
        // A permission without an op: each source after the datasource
        // holds it.
        assert_eq!(check(&system, INTERNET, &chain, true).unwrap(), GRANTED);
        assert_eq!(check(&system, INTERNET, &source(OTHER, "other"), false).unwrap(), HARD_DENIED);
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
        let checks = |system: &Fake, op: i32, s: &AttributionSourceState| {
            Checks(system).check_op(op, &chain(s).unwrap(), false, false).unwrap()
        };
        assert_eq!(checks(&system, OP_CAMERA, &app), GRANTED);
        assert_eq!(checks(&system, OP_NONE, &app), HARD_DENIED);
        assert_eq!(checks(&system, OP_CAMERA, &anonymous), HARD_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_IGNORED);
        assert_eq!(checks(&system, OP_CAMERA, &app), SOFT_DENIED);
        system.modes.insert((OP_CAMERA, APP), MODE_ERRORED);
        assert_eq!(checks(&system, OP_CAMERA, &app), HARD_DENIED);
    }

    /// Starting and finishing ops, noting a proxy op and virtual devices
    /// are not implemented; finishing no op is nothing.
    #[test]
    fn not_implemented() {
        let system = fake();
        let checks = Checks(&system);
        let app = source(APP, "app");
        assert!(checks.check_permission(CAMERA, &[&app], true, true, false, OP_NONE).is_err());
        assert!(checks.finish_data_delivery(OP_CAMERA, &[&app], false).is_err());
        assert!(checks.finish_data_delivery(OP_NONE, &[&app], false).is_ok());
        // The provider's caller proxies for another app.
        let provider = source(PROVIDER, "provider");
        let other = source(OTHER, "other");
        let proxy = [&provider, &other, &app];
        assert!(checks.check_permission(RECORD_AUDIO, &proxy, true, false, true, OP_NONE).is_err());
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
