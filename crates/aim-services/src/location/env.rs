//! What the location service consults outside itself, as the original's
//! injector (`LocationManagerService.SystemInjector` and its helpers)
//! consults it: permissions and app ops, the location settings, users,
//! the foreground state of apps, platform compat changes, and pending
//! intents to send. Each is the binder form of the original's call, made
//! as system_server makes it (the system uid, the "android" package).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Parcel};
use aim_service_aidl::{
    WriteParcelable, android_app_iactivitymanager as am, android_os_iusermanager as um,
    com_android_internal_app_iappopsservice as appops,
    com_android_internal_compat_iplatformcompat as compat,
};

use super::parcels::{Intent, Location, PackageTagsList, Value, write_bundle};
use crate::settings::Settings;
use crate::system::{MODE_ALLOWED, System};

pub const SYSTEM_UID: i32 = 1000;
/// `UserHandle.USER_ALL`, `USER_CURRENT`, `USER_NULL`.
pub const USER_ALL: i32 = -1;
pub const USER_CURRENT: i32 = -2;
pub const USER_NULL: i32 = -10000;
const PER_USER_RANGE: i32 = 100_000;

/// `LocationPermissions.PERMISSION_*`.
pub const PERMISSION_NONE: i32 = 0;
pub const PERMISSION_COARSE: i32 = 1;
pub const PERMISSION_FINE: i32 = 2;

pub const ACCESS_FINE_LOCATION: &str = "android.permission.ACCESS_FINE_LOCATION";
pub const ACCESS_COARSE_LOCATION: &str = "android.permission.ACCESS_COARSE_LOCATION";

/// `LocationManager.DELIVER_HISTORICAL_LOCATIONS`.
const DELIVER_HISTORICAL_LOCATIONS: i64 = 73144566;

/// Settings the service reads (`Settings.Secure`, `Settings.Global`,
/// `DeviceConfig`'s `location` namespace).
pub const LOCATION_MODE: &str = "location_mode";
const LOCATION_COARSE_ACCURACY_M: &str = "locationCoarseAccuracy";
const LOCATION_PACKAGE_DENYLIST: &str = "locationPackagePrefixBlacklist";
const LOCATION_PACKAGE_ALLOWLIST: &str = "locationPackagePrefixWhitelist";
const LOCATION_BACKGROUND_THROTTLE_INTERVAL_MS: &str = "location_background_throttle_interval_ms";
const LOCATION_BACKGROUND_THROTTLE_PACKAGE_WHITELIST: &str =
    "location_background_throttle_package_whitelist";
const LOCATION_BACKGROUND_THROTTLE_PROXIMITY_ALERT_INTERVAL_MS: &str =
    "location_background_throttle_proximity_alert_interval_ms";
pub const NAMESPACE_LOCATION: &str = "location";
const IGNORE_SETTINGS_ALLOWLIST: &str = "ignore_settings_allowlist";
const ADAS_SETTINGS_ALLOWLIST: &str = "adas_settings_allowlist";
const DEFAULT_BACKGROUND_THROTTLE_INTERVAL_MS: i64 = 30 * 60 * 1000;
const DEFAULT_COARSE_LOCATION_ACCURACY_M: f32 = 2000.0;

/// `ActivityManager.PROCESS_STATE_*` the foreground cut is made at.
const PROCESS_STATE_IMPORTANT_FOREGROUND: i32 = 6;
const PROCESS_STATE_NONEXISTENT: i32 = 20;

/// `BroadcastOptions`' bundle keys and the values location deliveries
/// set: `setDontSendToRestrictedApps(true)`,
/// `setPendingIntentBackgroundActivityLaunchAllowed(false)` and, for a
/// location, a temporary allowlist to start a foreground service
/// (`TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED`,
/// `REASON_LOCATION_PROVIDER`, 10 s).
const KEY_PENDING_INTENT_BACKGROUND_ACTIVITY_ALLOWED: &str =
    "android.pendingIntent.backgroundActivityAllowed";
const MODE_BACKGROUND_ACTIVITY_START_DENIED: i32 = 2;
const KEY_FLAGS: &str = "android:broadcast.flags";
const FLAG_DONT_SEND_TO_RESTRICTED_APPS: i32 = 1;
const KEY_TEMPORARY_APP_ALLOWLIST_DURATION: &str =
    "android:broadcast.temporaryAppAllowlistDuration";
const KEY_TEMPORARY_APP_ALLOWLIST_TYPE: &str = "android:broadcast.temporaryAppAllowlistType";
const KEY_TEMPORARY_APP_ALLOWLIST_REASON_CODE: &str =
    "android:broadcast.temporaryAppAllowlistReasonCode";
const KEY_TEMPORARY_APP_ALLOWLIST_REASON: &str = "android:broadcast.temporaryAppAllowlistReason";
const TEMPORARY_APP_ALLOWLIST_DURATION_MS: i64 = 10 * 1000;
const TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED: i32 = 0;
const REASON_LOCATION_PROVIDER: i32 = 312;

/// `CallerIdentity`.
#[derive(Clone, Debug)]
pub struct Identity {
    pub uid: i32,
    pub pid: i32,
    pub package: String,
    pub attribution_tag: Option<String>,
    pub listener_id: Option<String>,
}

impl Identity {
    pub fn user_id(&self) -> i32 {
        self.uid / PER_USER_RANGE
    }
}

impl PartialEq for Identity {
    /// `CallerIdentity.equals`: a listener id matches any when either has
    /// none.
    fn eq(&self, o: &Identity) -> bool {
        self.uid == o.uid
            && self.pid == o.pid
            && self.package == o.package
            && self.attribution_tag == o.attribution_tag
            && match (&self.listener_id, &o.listener_id) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

impl std::fmt::Display for Identity {
    /// `CallerIdentity.toString` without the listener id's hash.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.uid, self.package)?;
        if let Some(tag) = &self.attribution_tag {
            let tag = tag.strip_prefix(self.package.as_str()).unwrap_or(tag);
            write!(f, "[{tag}]")?;
        }
        Ok(())
    }
}

/// The allowlists `SystemConfig` holds (`allow-unthrottled-location`,
/// `allow-ignore-location-settings`, `allow-adas-location-settings`),
/// handed over by the system_server bridge.
#[derive(Clone, Default)]
pub struct SystemConfig {
    pub unthrottled: BTreeSet<String>,
    pub ignore_settings: PackageTagsList,
    pub adas: PackageTagsList,
}

/// What the users are, once the bridge tells of their changes
/// (`UserInfoHelper`'s listeners).
pub struct Users {
    pub current: i32,
    pub running: Vec<i32>,
    pub visible: HashMap<i32, bool>,
}

/// `PowerManager.LOCATION_MODE_*`.
pub const LOCATION_MODE_NO_CHANGE: i32 = 0;
pub const LOCATION_MODE_GPS_DISABLED_WHEN_SCREEN_OFF: i32 = 1;
pub const LOCATION_MODE_ALL_DISABLED_WHEN_SCREEN_OFF: i32 = 2;
pub const LOCATION_MODE_FOREGROUND_ONLY: i32 = 3;
pub const LOCATION_MODE_THROTTLE_REQUESTS_WHEN_SCREEN_OFF: i32 = 4;

/// The location power save mode and whether the screen is interactive,
/// as the bridge reports them (`LocationPowerSaveModeHelper`,
/// `ScreenInteractiveHelper`: not interactive until a screen broadcast
/// says so).
#[derive(Clone, Copy, Default)]
pub struct Power {
    pub mode: i32,
    pub interactive: bool,
}

pub struct Env {
    pub process: Arc<LocalProcess>,
    pub system: Arc<System>,
    pub settings: Arc<Settings>,
    /// The service's pid: the identity of its own clients.
    pub pid: i32,
    /// The token app ops started by the service are held with
    /// (`AttributionSource.getToken()` of its context).
    app_ops_token: Binder,
    /// The identities of the providers (`LocationManagerInternal.isProvider`).
    provider_identities: Mutex<Vec<Identity>>,
    pub system_config: Mutex<SystemConfig>,
    /// Kept while the bridge reports their changes; asked each time
    /// without it.
    pub users: Mutex<Option<Users>>,
    pub power: Mutex<Power>,
}

impl Env {
    pub fn new(process: Arc<LocalProcess>, system: Arc<System>, settings: Arc<Settings>) -> Env {
        let app_ops_token = process.add_service(Arc::new(Token));
        Env {
            process,
            system,
            settings,
            pid: std::process::id() as i32,
            app_ops_token,
            provider_identities: Mutex::new(Vec::new()),
            system_config: Mutex::new(SystemConfig::default()),
            users: Mutex::new(None),
            power: Mutex::new(Power::default()),
        }
    }

    /// `SystemClock.elapsedRealtime()`: the guest's CLOCK_BOOTTIME.
    pub fn now_ms(&self) -> i64 {
        self.now_ns() / 1_000_000
    }

    pub fn now_ns(&self) -> i64 {
        aim_hostcall::clock::boottime_ns()
    }

    /// The service's own identity (`CallerIdentity.fromContext` of its
    /// attribution context `tag`).
    pub fn own_identity(&self, tag: &str) -> Identity {
        Identity {
            uid: SYSTEM_UID,
            pid: self.pid,
            package: "android".into(),
            attribution_tag: Some(tag.into()),
            listener_id: None,
        }
    }

    // ---- permissions and app ops ----

    /// `Context.checkPermission(permission, pid, uid)`; a failure to ask
    /// counts as denied.
    pub fn check_permission(&self, permission: &str, pid: i32, uid: i32) -> bool {
        self.system
            .check_permission(permission, pid, uid)
            .unwrap_or_else(|e| {
                eprintln!("location: checkPermission({permission}): {}", e.message);
                false
            })
    }

    /// `LocationPermissions.getPermissionLevel`.
    pub fn permission_level(&self, uid: i32, pid: i32) -> i32 {
        if self.check_permission(ACCESS_FINE_LOCATION, pid, uid) {
            PERMISSION_FINE
        } else if self.check_permission(ACCESS_COARSE_LOCATION, pid, uid) {
            PERMISSION_COARSE
        } else {
            PERMISSION_NONE
        }
    }

    /// `LocationPermissionsHelper.hasLocationPermissions`.
    pub fn has_location_permissions(&self, level: i32, identity: &Identity) -> bool {
        if level == PERMISSION_NONE {
            return false;
        }
        let permission = if level == PERMISSION_FINE {
            ACCESS_FINE_LOCATION
        } else {
            ACCESS_COARSE_LOCATION
        };
        self.check_permission(permission, identity.pid, identity.uid)
            && self.check_op_no_throw(super::provider::as_app_op(level), identity)
    }

    /// `AppOpsManager.checkOpNoThrow(op, uid, package) == MODE_ALLOWED`.
    pub fn check_op_no_throw(&self, op: i32, identity: &Identity) -> bool {
        matches!(
            self.system
                .app_op(false, op, identity.uid, &identity.package, None),
            Ok(MODE_ALLOWED)
        )
    }

    /// `AppOpsManager.noteOpNoThrow(...) == MODE_ALLOWED`, noted before
    /// the access it records, with the client's listener id as its
    /// message.
    pub fn note_op_no_throw(&self, op: i32, identity: &Identity) -> bool {
        match self.system.note_op_now(
            op,
            identity.uid,
            &identity.package,
            identity.attribution_tag.as_deref(),
            identity.listener_id.as_deref().unwrap_or("location"),
        ) {
            Ok(mode) => mode == MODE_ALLOWED,
            Err(e) => {
                eprintln!("location: noteOperation({op}): {}", e.message);
                false
            }
        }
    }

    /// `AppOpsManager.noteOp(...) == MODE_ALLOWED`, which throws on
    /// `MODE_ERRORED`.
    pub fn note_op(
        &self,
        op: i32,
        identity: &Identity,
    ) -> Result<bool, aim_binder_host::parcel::Exception> {
        let mode = self.system.note_op_now(
            op,
            identity.uid,
            &identity.package,
            identity.attribution_tag.as_deref(),
            identity.listener_id.as_deref().unwrap_or("location"),
        )?;
        if mode == crate::system::MODE_ERRORED {
            return Err(aim_binder_host::parcel::Exception::security(format!(
                "{} from uid {} not allowed to perform {}",
                identity.package,
                identity.uid,
                op_name(op)
            )));
        }
        Ok(mode == MODE_ALLOWED)
    }

    /// `AppOpsManager.startOpNoThrow(op, uid, package, false, tag,
    /// listenerId)`.
    pub fn start_op(&self, op: i32, identity: &Identity) {
        let args = appops::StartOperation {
            client_id: Some(self.app_ops_token),
            code: op,
            uid: identity.uid,
            package_name: Some(identity.package.clone()),
            attribution_tag: identity.attribution_tag.clone(),
            start_if_mode_default: false,
            should_collect_async_noted_op: false,
            message: identity.listener_id.clone(),
            should_collect_message: false,
            attribution_flags: 0,
            attribution_chain_id: -1,
        };
        if let Err(e) = self.system.call(
            "appops",
            appops::START_OPERATION,
            |p| args.write(p),
            appops::read_start_operation_reply::<super::parcels::Unread>,
        ) {
            eprintln!("location: startOperation({op}): {}", e.message);
        }
    }

    /// `AppOpsManager.finishOp(op, uid, package, tag)`.
    pub fn finish_op(&self, op: i32, identity: &Identity) {
        let args = appops::FinishOperation {
            client_id: Some(self.app_ops_token),
            code: op,
            uid: identity.uid,
            package_name: Some(identity.package.clone()),
            attribution_tag: identity.attribution_tag.clone(),
        };
        if let Err(e) = self.system.call(
            "appops",
            appops::FINISH_OPERATION,
            |p| args.write(p),
            appops::read_finish_operation_reply,
        ) {
            eprintln!("location: finishOperation({op}): {}", e.message);
        }
    }

    /// `AppOpsManager.setUserRestrictionForUser(op, restricted, token,
    /// excluded, user)`.
    pub fn set_user_restriction(
        &self,
        op: i32,
        restricted: bool,
        token: Binder,
        excluded: Option<&PackageTagsList>,
        user: i32,
    ) {
        let args = appops::SetUserRestriction {
            code: op,
            restricted,
            token: Some(token),
            user_handle: user,
            excluded_package_tags: excluded.cloned(),
        };
        if let Err(e) = self.system.call(
            "appops",
            appops::SET_USER_RESTRICTION,
            |p| args.write(p),
            appops::read_set_user_restriction_reply,
        ) {
            eprintln!("location: setUserRestriction({op}): {}", e.message);
        }
    }

    /// `CompatChanges.isChangeEnabled(DELIVER_HISTORICAL_LOCATIONS, uid)`.
    pub fn deliver_historical_locations(&self, uid: i32) -> bool {
        self.change_enabled(DELIVER_HISTORICAL_LOCATIONS, uid)
    }

    /// `CompatChanges.isChangeEnabled(change, uid)`: the platform compat
    /// service's answer, with its overrides.
    pub fn change_enabled(&self, change: i64, uid: i32) -> bool {
        let args = compat::IsChangeEnabledByUid {
            change_id: change,
            uid,
        };
        self.system
            .call(
                "platform_compat",
                compat::IS_CHANGE_ENABLED_BY_UID,
                |p| args.write(p),
                compat::read_is_change_enabled_by_uid_reply,
            )
            .unwrap_or_else(|e| {
                eprintln!("location: isChangeEnabledByUid({change}): {}", e.message);
                true
            })
    }

    // ---- users and apps ----

    /// `ActivityManagerInternal.getCurrentUserId()`.
    pub fn current_user(&self) -> i32 {
        if let Some(users) = &*self.users.lock().unwrap() {
            return users.current;
        }
        self.ask_current_user()
    }

    pub fn ask_current_user(&self) -> i32 {
        self.system
            .call(
                "activity",
                am::GET_CURRENT_USER_ID,
                |p| am::GetCurrentUserId {}.write(p),
                am::read_get_current_user_id_reply,
            )
            .unwrap_or(USER_NULL)
    }

    /// `IActivityManager.getRunningUserIds()`.
    pub fn running_users(&self) -> Vec<i32> {
        if let Some(users) = &*self.users.lock().unwrap() {
            return users.running.clone();
        }
        self.ask_running_users()
    }

    pub fn ask_running_users(&self) -> Vec<i32> {
        self.system
            .call(
                "activity",
                am::GET_RUNNING_USER_IDS,
                |p| am::GetRunningUserIds {}.write(p),
                am::read_get_running_user_ids_reply,
            )
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// `UserManagerInternal.isUserVisible(userId)`.
    pub fn user_visible(&self, user: i32) -> bool {
        if let Some(visible) = self
            .users
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|u| u.visible.get(&user).copied())
        {
            return visible;
        }
        let visible = self.ask_user_visible(user);
        if let Some(users) = &mut *self.users.lock().unwrap() {
            users.visible.insert(user, visible);
        }
        visible
    }

    fn ask_user_visible(&self, user: i32) -> bool {
        let args = um::IsUserVisible { user_id: user };
        self.system
            .call(
                "user",
                um::IS_USER_VISIBLE,
                |p| args.write(p),
                um::read_is_user_visible_reply,
            )
            .unwrap_or(false)
    }

    /// `AppForegroundHelper.isAppForeground(uid)`: its importance at most
    /// `IMPORTANCE_FOREGROUND_SERVICE`.
    pub fn foreground(&self, uid: i32) -> bool {
        let args = am::GetUidProcessState {
            uid,
            calling_package: Some("android".into()),
        };
        let state = self
            .system
            .call(
                "activity",
                am::GET_UID_PROCESS_STATE,
                |p| args.write(p),
                am::read_get_uid_process_state_reply,
            )
            .unwrap_or(PROCESS_STATE_NONEXISTENT);
        is_foreground(state)
    }

    // ---- settings ----

    /// `SettingsHelper.isLocationEnabled(userId)`.
    pub fn location_enabled(&self, user: i32) -> bool {
        secure_int(&self.settings, LOCATION_MODE, 0, user) != 0
    }

    /// `SettingsHelper.getCoarseLocationAccuracyM()`.
    pub fn coarse_accuracy_m(&self) -> f32 {
        self.settings
            .secure(LOCATION_COARSE_ACCURACY_M, 0)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_COARSE_LOCATION_ACCURACY_M)
    }

    /// `SettingsHelper.getBackgroundThrottleIntervalMs()`.
    pub fn background_throttle_interval_ms(&self) -> i64 {
        global_long(
            &self.settings,
            LOCATION_BACKGROUND_THROTTLE_INTERVAL_MS,
            DEFAULT_BACKGROUND_THROTTLE_INTERVAL_MS,
        )
    }

    /// `SettingsHelper.getBackgroundThrottleProximityAlertIntervalMs()`.
    pub fn background_throttle_proximity_alert_interval_ms(&self) -> i64 {
        global_long(
            &self.settings,
            LOCATION_BACKGROUND_THROTTLE_PROXIMITY_ALERT_INTERVAL_MS,
            DEFAULT_BACKGROUND_THROTTLE_INTERVAL_MS,
        )
    }

    /// `SettingsHelper.getBackgroundThrottlePackageWhitelist()`.
    pub fn background_throttle_package_whitelist(&self) -> BTreeSet<String> {
        let mut set = self.system_config.lock().unwrap().unthrottled.clone();
        if let Ok(Some(v)) = self
            .settings
            .global(LOCATION_BACKGROUND_THROTTLE_PACKAGE_WHITELIST)
        {
            set.extend(java_split(&v, ',').into_iter().map(String::from));
        }
        set
    }

    /// `SettingsHelper.isLocationPackageBlacklisted(userId, package)`.
    pub fn package_blacklisted(&self, user: i32, package: &str) -> bool {
        let list = |name| -> Vec<String> {
            self.settings
                .secure(name, user)
                .ok()
                .flatten()
                .map(|v| java_split(&v, ',').into_iter().map(String::from).collect())
                .unwrap_or_default()
        };
        let deny = list(LOCATION_PACKAGE_DENYLIST);
        if deny.is_empty() {
            return false;
        }
        if list(LOCATION_PACKAGE_ALLOWLIST)
            .iter()
            .any(|p| package.starts_with(p.as_str()))
        {
            return false;
        }
        deny.iter().any(|p| package.starts_with(p.as_str()))
    }

    /// `SettingsHelper.getIgnoreSettingsAllowlist()`.
    pub fn ignore_settings_allowlist(&self) -> PackageTagsList {
        let base = self.system_config.lock().unwrap().ignore_settings.clone();
        self.package_tags_setting(IGNORE_SETTINGS_ALLOWLIST, base)
    }

    /// `SettingsHelper.getAdasAllowlist()`.
    pub fn adas_allowlist(&self) -> PackageTagsList {
        let base = self.system_config.lock().unwrap().adas.clone();
        self.package_tags_setting(ADAS_SETTINGS_ALLOWLIST, base)
    }

    /// `PackageTagsListSetting.getValue()`: the base values and the
    /// `DeviceConfig` value, `package;tag;tag,...` (`null` for no tag,
    /// `*` for every tag).
    fn package_tags_setting(&self, name: &str, mut list: PackageTagsList) -> PackageTagsList {
        let Ok(Some(setting)) = self.settings.config(NAMESPACE_LOCATION, name) else {
            return list;
        };
        for package_and_tags in java_split(&setting, ',') {
            if package_and_tags.is_empty() {
                continue;
            }
            let parts = java_split(package_and_tags, ';');
            let package = parts[0];
            if parts.len() == 1 {
                list.add_all(package);
                continue;
            }
            for tag in &parts[1..] {
                match *tag {
                    "*" => list.add_all(package),
                    "null" => list.add(package, None),
                    tag => list.add(package, Some(tag)),
                }
            }
        }
        list
    }

    // ---- providers ----

    pub fn set_provider_identities(&self, identities: Vec<Identity>) {
        *self.provider_identities.lock().unwrap() = identities;
    }

    /// `LocationManagerInternal.isProvider(null, identity)`: whether the
    /// identity is a provider's. Every provider here is visible to every
    /// caller (none requires a permission).
    pub fn is_provider(&self, _provider: Option<&str>, identity: &Identity) -> bool {
        self.provider_identities
            .lock()
            .unwrap()
            .iter()
            .any(|i| i == identity)
    }

    // ---- pending intents ----

    /// `PendingIntent.send(context, 0, intent, null, null, null, options)`
    /// with a location delivery's options; false if it was cancelled.
    pub fn send_locations_intent(&self, pi: &Strong, locations: &[Location]) -> bool {
        let intent = Intent {
            action: None,
            flags: 0,
            extras: super::provider::location_extras(locations),
        };
        self.send_pending_intent(pi, &intent, true)
    }

    /// `PendingIntent.send` with the don't-send-to-restricted-apps
    /// options of flush and enabled deliveries.
    pub fn send_intent(&self, pi: &Strong, intent: &Intent<'_>) -> bool {
        self.send_pending_intent(pi, intent, false)
    }

    fn send_pending_intent(&self, pi: &Strong, intent: &Intent<'_>, allowlist: bool) -> bool {
        struct Options(bool);
        impl WriteParcelable for Options {
            fn write_to(&self, p: &mut Parcel) {
                let mut entries = vec![
                    (
                        KEY_PENDING_INTENT_BACKGROUND_ACTIVITY_ALLOWED,
                        Value::Int(MODE_BACKGROUND_ACTIVITY_START_DENIED),
                    ),
                    (KEY_FLAGS, Value::Int(FLAG_DONT_SEND_TO_RESTRICTED_APPS)),
                ];
                if self.0 {
                    entries.extend([
                        (
                            KEY_TEMPORARY_APP_ALLOWLIST_DURATION,
                            Value::Long(TEMPORARY_APP_ALLOWLIST_DURATION_MS),
                        ),
                        (
                            KEY_TEMPORARY_APP_ALLOWLIST_TYPE,
                            Value::Int(TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED),
                        ),
                        (
                            KEY_TEMPORARY_APP_ALLOWLIST_REASON_CODE,
                            Value::Int(REASON_LOCATION_PROVIDER),
                        ),
                        (KEY_TEMPORARY_APP_ALLOWLIST_REASON, Value::String("")),
                    ]);
                }
                write_bundle(p, &entries);
            }
        }
        let args = am::SendIntentSender {
            caller: None,
            target: Some(pi.binder()),
            whitelist_token: None,
            code: 0,
            intent: Some(intent),
            resolved_type: None,
            finished_receiver: None,
            required_permission: None,
            options: Some(Options(allowlist)),
        };
        match self.system.call(
            "activity",
            am::SEND_INTENT_SENDER,
            |p| args.write(p),
            am::read_send_intent_sender_reply,
        ) {
            // ActivityManager.START_CANCELED and other failures are
            // negative: PendingIntent.CanceledException.
            Ok(code) => code >= 0,
            Err(e) => {
                eprintln!("location: sendIntentSender: {}", e.message);
                false
            }
        }
    }

    /// `Context.sendBroadcastAsUser(intent, user)` of the system.
    pub fn broadcast(&self, intent: &dyn Fn(&mut Parcel), user: i32) {
        struct Raw<'a>(&'a dyn Fn(&mut Parcel));
        impl WriteParcelable for Raw<'_> {
            fn write_to(&self, p: &mut Parcel) {
                (self.0)(p)
            }
        }
        struct NoBundle;
        impl WriteParcelable for NoBundle {
            fn write_to(&self, _: &mut Parcel) {}
        }
        let args = am::BroadcastIntentWithFeature::<Raw<'_>, NoBundle> {
            caller: None,
            calling_feature_id: None,
            intent: Some(Raw(intent)),
            resolved_type: None,
            result_to: None,
            result_code: -1, // Activity.RESULT_OK
            result_data: None,
            map: None,
            required_permissions: None,
            exclude_permissions: None,
            exclude_packages: None,
            app_op: -1, // AppOpsManager.OP_NONE
            options: None,
            serialized: false,
            sticky: false,
            user_id: user,
        };
        if let Err(e) = self.system.call(
            "activity",
            am::BROADCAST_INTENT_WITH_FEATURE,
            |p| args.write(p),
            am::read_broadcast_intent_with_feature_reply,
        ) {
            eprintln!("location: broadcastIntent: {}", e.message);
        }
    }
}

/// `AppOpsManager.opToName` of the ops the service notes with `noteOp`.
fn op_name(op: i32) -> &'static str {
    match op {
        58 => "MOCK_LOCATION",
        1 => "FINE_LOCATION",
        _ => "COARSE_LOCATION",
    }
}

/// `procStateToImportance(state) <= IMPORTANCE_FOREGROUND_SERVICE`.
pub fn is_foreground(process_state: i32) -> bool {
    process_state != PROCESS_STATE_NONEXISTENT && process_state < PROCESS_STATE_IMPORTANT_FOREGROUND
}

/// `Settings.Secure.getIntForUser(resolver, name, default, user)`.
fn secure_int(settings: &Settings, name: &str, default: i32, user: i32) -> i32 {
    settings
        .secure(name, user)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `Settings.Global.getLong(resolver, name, default)`.
fn global_long(settings: &Settings, name: &str, default: i64) -> i64 {
    settings
        .global(name)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `String.split(separator)`: trailing empty strings removed.
pub fn java_split(s: &str, separator: char) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split(separator).collect();
    while parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    if parts == [""] {
        parts.clear();
    }
    parts
}

/// A token binder: its identity is all it serves.
struct Token;

impl Service for Token {
    fn descriptor(&self) -> &str {
        ""
    }

    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_as_java() {
        assert_eq!(java_split("a,b,,", ','), ["a", "b"]);
        assert_eq!(java_split("a,,b", ','), ["a", "", "b"]);
        assert!(java_split("", ',').is_empty());
    }

    #[test]
    fn foreground_cut() {
        assert!(is_foreground(2)); // TOP
        assert!(is_foreground(5)); // BOUND_FOREGROUND_SERVICE
        assert!(!is_foreground(6)); // IMPORTANT_FOREGROUND
        assert!(!is_foreground(20)); // NONEXISTENT
    }

    #[test]
    fn identities_match_without_listener_ids() {
        let a = Identity {
            uid: 10_001,
            pid: 5,
            package: "p".into(),
            attribution_tag: None,
            listener_id: Some("x".into()),
        };
        let mut b = a.clone();
        b.listener_id = None;
        assert_eq!(a, b);
        b.listener_id = Some("y".into());
        assert_ne!(a, b);
        assert_eq!(a.user_id(), 0);
    }
}
