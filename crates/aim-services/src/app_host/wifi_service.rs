//! `wifi`: WifiServiceImpl for the app host (`android-16.0.0_r1`), over
//! the Mac's Wi-Fi (`aim_host_wifi`): its radio is Android's Wi-Fi state;
//! the verbose logging level and scanning always available are what the
//! shared config store (`WifiConfigStore.xml`, `WifiSettingsConfigStore`)
//! persisted; with the radio off there is no connection (`new WifiInfo()`)
//! and no scan result (the Mac does not scan with its radio off), given to
//! a caller `enforceCanAccessScanResults` lets read them. The saved
//! networks (`getConfiguredNetworks`) are the config stores' as
//! `WifiConfigManager` loads them (`wifi_config`), with the Wi-Fi resources
//! APK's MAC randomization and auto-upgrade settings; there is no carrier
//! app without telephony. The rest is not implemented (#1686).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_SECURITY, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_location_ilocationmanager as lm;
use aim_service_aidl::android_net_wifi_iwifimanager as wm;

use super::appops_service::MODE_ALLOWED;
use super::settings::Table;
use super::wifi_config::{self, LoadError, SECURITY_TYPE_OWE, SECURITY_TYPE_SAE, SecurityParams, WifiConfiguration};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::list_slice::Slice;
use crate::package::parse::resources::{Selected, TYPE_FIRST_INT, TYPE_REFERENCE, Table as ResourceTable};
use crate::package::query::Query;

const ACCESS_WIFI_STATE: &str = "android.permission.ACCESS_WIFI_STATE";
const NETWORK_SETTINGS: &str = "android.permission.NETWORK_SETTINGS";
const NETWORK_SETUP_WIZARD: &str = "android.permission.NETWORK_SETUP_WIZARD";
const NETWORK_MANAGED_PROVISIONING: &str = "android.permission.NETWORK_MANAGED_PROVISIONING";
const NETWORK_STACK: &str = "android.permission.NETWORK_STACK";
/// The permissions that read scan results without location access
/// (`enforceCanAccessScanResults`).
const SCAN_BYPASS: [&str; 6] = [
    NETWORK_SETTINGS,
    NETWORK_SETUP_WIZARD,
    NETWORK_MANAGED_PROVISIONING,
    NETWORK_STACK,
    "android.permission.MAINLINE_NETWORK_STACK",
    "android.permission.RADIO_SCAN_WITHOUT_LOCATION",
];
/// `Process.ROOT_UID`, `SYSTEM_UID`, `WIFI_UID`, `SHELL_UID`,
/// `INVALID_UID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const WIFI_UID: i32 = 1010;
const SHELL_UID: i32 = 2000;
const INVALID_UID: i32 = -1;
/// `PackageManager.SIGNATURE_MATCH`.
const SIGNATURE_MATCH: i32 = 0;
/// `ApplicationInfo.FLAG_SYSTEM`, `FLAG_UPDATED_SYSTEM_APP`.
const FLAG_SYSTEM: i32 = 1 << 0;
const FLAG_UPDATED_SYSTEM_APP: i32 = 1 << 7;
/// `WifiContext`'s resources APK, of an app in the Wi-Fi APEX.
const RESOURCES_APK_ACTION: &str = "com.android.server.wifi.intent.action.SERVICE_WIFI_RESOURCES_APK";
const WIFI_APEX: &str = "/apex/com.android.wifi/";
const WIFI_CONFIGURATION: &str = "android.net.wifi.WifiConfiguration";
const ACCESS_FINE_LOCATION: &str = "android.permission.ACCESS_FINE_LOCATION";
const ACCESS_COARSE_LOCATION: &str = "android.permission.ACCESS_COARSE_LOCATION";
const PEERS_MAC_ADDRESS: &str = "android.permission.PEERS_MAC_ADDRESS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
/// `AppOpsManager.OP_COARSE_LOCATION`, `OP_FINE_LOCATION`, `OP_WIFI_SCAN`.
const OP_COARSE_LOCATION: i32 = 0;
const OP_FINE_LOCATION: i32 = 1;
const OP_WIFI_SCAN: i32 = 10;
/// `Build.VERSION_CODES.Q`.
const Q: i32 = 29;
/// `Settings.Secure.LOCATION_MODE_ON`.
const LOCATION_MODE_ON: &str = "3";
/// The user the app host runs (`getCurrentUser`).
const CURRENT_USER: i32 = 0;
const SCAN_RESULT: &str = "android.net.wifi.ScanResult";

/// No scan result, of which a list is written.
enum NoScanResult {}

impl WriteParcelable for NoScanResult {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}
/// `WifiManager.WIFI_STATE_DISABLED`, `WIFI_STATE_ENABLED`.
const WIFI_STATE_DISABLED: i32 = 1;
const WIFI_STATE_ENABLED: i32 = 3;
/// `WifiManager.VERBOSE_LOGGING_LEVEL_DISABLED`, `_ENABLED`.
const VERBOSE_LOGGING_LEVEL_DISABLED: i32 = 0;
const VERBOSE_LOGGING_LEVEL_ENABLED: i32 = 1;
/// The shared general store (`WifiConfigStore.STORE_FILE_SHARED_GENERAL`).
const SHARED_STORE: &str = "misc/apexdata/com.android.wifi/WifiConfigStore.xml";
/// The current user's store (`STORE_FILE_USER_GENERAL`).
const USER_STORE: &str = "misc_ce/0/apexdata/com.android.wifi/WifiConfigStore.xml";

/// `WifiInfo()`, the info of no connection, as `writeToParcel` writes it.
struct NoConnection;

impl WriteParcelable for NoConnection {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(-1); // network id
        p.write_i32(-127); // INVALID_RSSI
        p.write_i32(-1); // LINK_SPEED_UNKNOWN
        p.write_i32(0); // tx link speed
        p.write_i32(0); // rx link speed
        p.write_i32(-1); // UNKNOWN_FREQUENCY
        p.write_i32(0); // no IP address (writeByte)
        p.write_i32(0); // no SSID
        p.write_string16(None); // BSSID
        p.write_string16(Some("02:00:00:00:00:00")); // DEFAULT_MAC_ADDRESS
        for _ in 0..6 {
            p.write_i32(0); // metered hint, ephemeral, trusted, OEM paid, OEM private, carrier merged
        }
        p.write_i32(0); // score
        p.write_i32(1); // usable
        for _ in 0..4 {
            p.write_i64(0); // tx success, retries, bad, rx success
            p.write_i64(0.0f64.to_bits() as i64); // their rates
        }
        p.write_string16(Some("UNINITIALIZED")); // SupplicantState
        p.write_i32(0); // OSU AP
        for _ in 0..3 {
            p.write_string16(None); // requesting package, FQDN, provider friendly name
        }
        p.write_i32(0); // Wi-Fi standard
        p.write_i32(0); // max supported tx link speed
        p.write_i32(0); // max supported rx link speed
        p.write_string16(None); // Passpoint unique id
        p.write_i32(-1); // INVALID_SUBSCRIPTION_ID
        p.write_i32(-1); // no information elements
        p.write_i32(0); // IS_PRIMARY_FALSE
        p.write_i32(-1); // security type
        p.write_i32(0); // restricted
        p.write_string16(None); // network key
        p.write_string16(None); // no AP MLD MAC address (writeParcelable)
        p.write_i32(-1); // INVALID_MLO_LINK_ID
        p.write_i32(0); // no affiliated MLO links
        p.write_i32(0); // TID-to-link mapping negotiation
        p.write_i32(0); // no vendor data
    }
}

/// A `WifiSettingsConfigStore` boolean as the shared store persisted it
/// (`<Settings><map name="Values">`), else its default.
fn store_bool(root: &aim_android_xml::Element, key: &str, default: bool) -> Result<bool, String> {
    let values = root
        .children()
        .filter(|e| e.name == "Settings")
        .flat_map(|e| e.children())
        .find(|e| e.name == "map" && e.string("name").as_deref() == Some("Values"));
    let Some(entry) = values.and_then(|v| v.children().find(|e| e.string("name").as_deref() == Some(key))) else {
        return Ok(default);
    };
    Ok(entry.bool("value")?.unwrap_or(default))
}

/// The Wi-Fi resources it reads (`WifiContext.getResources`).
struct WifiResources {
    /// `config_wifi_connected_mac_randomization_supported`.
    mac_randomization_supported: bool,
    /// `config_wifiOweUpgradeEnabled`, `config_wifiSaeUpgradeEnabled`.
    owe_upgrade: bool,
    sae_upgrade: bool,
    /// `config_oemPrivilegedWifiAdminPackages`.
    oem_admins: Vec<String>,
}

pub struct WifiService {
    host: Arc<AppHost>,
    resources: OnceLock<WifiResources>,
    /// `WifiConfigManager`'s networks once loaded from the stores.
    networks: OnceLock<Vec<WifiConfiguration>>,
}

impl WifiService {
    pub fn new(host: Arc<AppHost>) -> Self {
        WifiService { host, resources: OnceLock::new(), networks: OnceLock::new() }
    }

    /// `enforceAccessPermission`.
    fn enforce_access(&self, uid: i32) -> Result<(), Exception> {
        if self.host.check_permission(Some(ACCESS_WIFI_STATE), uid)? != 0 {
            return Err(Exception::security(format!(
                "WifiService: Neither user {uid} nor current process has {ACCESS_WIFI_STATE}."
            )));
        }
        Ok(())
    }

    /// `mVerboseLoggingLevel` once the store is loaded
    /// (`WIFI_VERBOSE_LOGGING_ENABLED`, false by default).
    fn verbose_logging_level(&self) -> Result<i32, Exception> {
        let enabled = self.store("wifi_verbose_logging_enabled", false)?;
        Ok(if enabled { VERBOSE_LOGGING_LEVEL_ENABLED } else { VERBOSE_LOGGING_LEVEL_DISABLED })
    }

    /// A shared store boolean (`mSettingsConfigStore.get`).
    fn store(&self, key: &str, default: bool) -> Result<bool, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("wifi: {SHARED_STORE}: {e}"));
        match std::fs::read(self.host.data()?.join(SHARED_STORE)) {
            Ok(bytes) => store_bool(&aim_android_xml::read(&bytes).map_err(state)?, key, default).map_err(state),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
            Err(e) => Err(state(e.to_string())),
        }
    }

    /// `getConnectionInfo`: none while the radio is off; the Mac's
    /// connection is not implemented.
    fn connection_info(&self, uid: i32, package: Option<&str>) -> Result<NoConnection, Exception> {
        self.enforce_access(uid)?;
        // `WifiPermissionsUtil.checkPackage`.
        let package = package.unwrap_or_default();
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!("Package {package} does not belong to {uid}")));
        }
        if self.wifi_state() != WIFI_STATE_ENABLED {
            return Ok(NoConnection);
        }
        Err(self.host.unimplemented_named("wifi", "the Mac's Wi-Fi connection (#1686)"))
    }

    fn held(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `WifiPermissionsUtil.isLocationModeEnabled`: the location service's
    /// answer for the current user, else its `LOCATION_MODE` setting.
    fn location_mode_enabled(&self) -> Result<bool, Exception> {
        let mut data = Parcel::new();
        lm::IsLocationEnabledForUser { user_id: CURRENT_USER }.write(&mut data);
        let asked = {
            let _system = aim_binder_host::local::clear_calling_identity();
            super::display::transact_service(&self.host.process, "location", lm::IS_LOCATION_ENABLED_FOR_USER, &data)
        }
        .and_then(|reply| {
            lm::read_is_location_enabled_for_user_reply(&mut reply.reader()).map_err(|s| s.to_string())?.map_err(|e| format!("{e:?}"))
        });
        match asked {
            Ok(enabled) => Ok(enabled),
            Err(e) => {
                eprintln!("app host: wifi: Failure to get location mode via API, falling back to settings: {e}");
                Ok(self.host.inputs()?.settings.get(Table::Secure, "location_mode").as_deref() == Some(LOCATION_MODE_ON))
            }
        }
    }

    /// `noteAppOpAllowed`.
    fn note_allowed(&self, op: i32, package: &str, uid: i32) -> Result<bool, Exception> {
        Ok(super::appops_service::note(&self.host, op, uid, Some(package))? == MODE_ALLOWED)
    }

    /// `checkCallersLocationPermission(.., coarseForTargetSdkLessThanQ: true)`.
    fn location_permitted(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        let target_sdk = self.host.inputs()?.packages.application_info(package, 0).map(|ai| ai.target_sdk_version);
        let before_q = target_sdk.is_some_and(|sdk| sdk < Q);
        if !self.held(if before_q { ACCESS_COARSE_LOCATION } else { ACCESS_FINE_LOCATION }, uid)? {
            return Ok(false);
        }
        if self.note_allowed(OP_FINE_LOCATION, package, uid)? {
            return Ok(true);
        }
        if before_q {
            return self.note_allowed(OP_COARSE_LOCATION, package, uid);
        }
        Ok(false)
    }

    /// `WifiPermissionsUtil.enforceCanAccessScanResults`.
    fn enforce_can_access_scan_results(&self, package: Option<&str>, uid: i32) -> Result<(), Exception> {
        let package = package.ok_or_else(|| Exception::security(format!("Checking UID {uid} but Package Name is Null")))?;
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!("Package {package} does not belong to {uid}")));
        }
        for permission in SCAN_BYPASS {
            if self.held(permission, uid)? {
                return Ok(());
            }
        }
        if !self.location_mode_enabled()? {
            return Err(Exception::security("Location mode is disabled for the device"));
        }
        let peers = self.held(PEERS_MAC_ADDRESS, uid)?;
        if !peers && !self.location_permitted(package, uid)? {
            return Err(Exception::security(format!("UID {uid} has no location permission")));
        }
        if !self.note_allowed(OP_WIFI_SCAN, package, uid)? {
            return Err(Exception::security(format!("UID {uid} has no wifi scan permission")));
        }
        // `doesUidBelongToUser`: the app host's one user has no profiles.
        let current = uid % PER_USER_RANGE as i32 == 1000 || self.held(NETWORK_SETTINGS, uid)? || uid / PER_USER_RANGE as i32 == CURRENT_USER;
        if !current && !self.held(INTERACT_ACROSS_USERS_FULL, uid)? {
            return Err(Exception::security(format!("UID {uid} profile not permitted")));
        }
        Ok(())
    }

    /// `getScanResults`: none for a caller denied them; while the radio is
    /// off, `ScanRequestProxy` has none.
    fn scan_results(&self, uid: i32, package: Option<&str>) -> Result<Option<Slice<NoScanResult>>, Exception> {
        self.enforce_access(uid)?;
        match self.enforce_can_access_scan_results(package, uid) {
            Ok(()) => {}
            Err(e) if e.code == EX_SECURITY => {
                eprintln!("app host: wifi: Permission violation - getScanResults not allowed for uid={uid}, packageName={}, reason={}", package.unwrap_or("null"), e.message);
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
        if self.wifi_state() == WIFI_STATE_ENABLED {
            return Err(self.host.unimplemented_named("wifi", "the Mac's scan results (#1686)"));
        }
        Ok(Some(Slice::new(self.host.process.clone(), SCAN_RESULT, Vec::new())))
    }

    /// The resources of `ServiceWifiResources`, the system's activity of
    /// its action in the Wi-Fi APEX, over the framework's.
    fn resources(&self) -> Result<&WifiResources, Exception> {
        if let Some(resources) = self.resources.get() {
            return Ok(resources);
        }
        let Some((package, apk)) = super::safety_center::resources_apk(&self.host, "wifi", RESOURCES_APK_ACTION, WIFI_APEX)? else {
            return Err(Exception::new(EX_ILLEGAL_STATE, "wifi: Attempted to fetch resources before Wifi Resources APK is loaded!"));
        };
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("wifi: {}: {e}", apk.display()));
        let apk_file = aim_apps::apk::Apk::open(&apk).map_err(|e| state(e.to_string()))?;
        let table = match apk_file.file_if_present("resources.arsc").map_err(|e| state(e.to_string()))? {
            Some(bytes) => ResourceTable::parse(&bytes).map_err(|e| state(e.to_string()))?,
            None => return Err(state("no resources.arsc".into())),
        };
        let inputs = self.host.inputs()?;
        let resources = inputs.packages.package_resources(&table);
        // `Resources.getBoolean`: any integer, nonzero is true.
        let boolean = |name: &str| -> Result<bool, Exception> {
            let id = resources.identifier(&package, "bool", name).ok_or_else(|| state(format!("no bool/{name}")))?;
            let mut value = Selected { kind: TYPE_REFERENCE, data: id, table: None, resid: 0, flags: 0 };
            resources.resolve(&mut value);
            if value.kind < TYPE_FIRST_INT {
                return Err(state(format!("bool/{name} is not an integer")));
            }
            Ok(value.data != 0)
        };
        let admins = "config_oemPrivilegedWifiAdminPackages";
        let oem_admins = resources
            .identifier(&package, "array", admins)
            .and_then(|id| resources.string_array(id))
            .ok_or_else(|| state(format!("no array/{admins}")))?;
        let loaded = WifiResources {
            mac_randomization_supported: boolean("config_wifi_connected_mac_randomization_supported")?,
            owe_upgrade: boolean("config_wifiOweUpgradeEnabled")?,
            sae_upgrade: boolean("config_wifiSaeUpgradeEnabled")?,
            oem_admins: oem_admins.into_iter().flatten().collect(),
        };
        Ok(self.resources.get_or_init(|| loaded))
    }

    /// `WifiConfigManager.loadFromStore`: the shared then the user store's
    /// networks; a store that does not parse loses them all.
    fn networks(&self) -> Result<&[WifiConfiguration], Exception> {
        if let Some(networks) = self.networks.get() {
            return Ok(networks);
        }
        let unmodelled = |what: &str| self.host.unimplemented_named("wifi", &format!("{what} (#1686)"));
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        let creator = |uid: i32| query.name_for_uid(uid).map_err(|e| LoadError::Unmodelled(e.0));
        let data = self.host.data()?;
        let read = |store: &str| -> Result<Vec<WifiConfiguration>, LoadError> {
            match std::fs::read(data.join(store)) {
                Ok(bytes) => {
                    let root = aim_android_xml::read(&bytes).map_err(LoadError::Corrupt)?;
                    wifi_config::read_network_list(&root, &creator)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(LoadError::Corrupt(format!("{store}: {e}"))),
            }
        };
        let networks = match read(SHARED_STORE).and_then(|shared| wifi_config::load(shared, read(USER_STORE)?)) {
            Ok(networks) => networks,
            Err(LoadError::Corrupt(e)) => {
                eprintln!("app host: wifi: XML deserialization of store failed. All saved networks are lost!: {e}");
                Vec::new()
            }
            Err(LoadError::Skip(e)) => return Err(Exception::new(EX_ILLEGAL_STATE, format!("wifi: {e}"))),
            Err(LoadError::Unmodelled(what)) => return Err(unmodelled(what)),
        };
        Ok(self.networks.get_or_init(|| networks))
    }

    /// `isPrivileged`: a network settings, setup wizard, stack or managed
    /// provisioning app, or one signed with the platform key.
    fn privileged(&self, uid: i32) -> Result<bool, Exception> {
        for permission in [NETWORK_SETTINGS, NETWORK_SETUP_WIZARD, NETWORK_STACK, NETWORK_MANAGED_PROVISIONING] {
            if self.held(permission, uid)? {
                return Ok(true);
            }
        }
        self.signed_with_platform_key(uid)
    }

    /// `checkSignatures(uid, SYSTEM_UID) == SIGNATURE_MATCH`.
    fn signed_with_platform_key(&self, uid: i32) -> Result<bool, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        let matched = query.check_uid_signatures_captured(uid, SYSTEM_UID, false).map_err(|e| self.host.unimplemented_named("wifi", e.0))??;
        Ok(matched == SIGNATURE_MATCH)
    }

    /// `isDeviceOrProfileOwner`: the app host has no owner.
    fn device_or_profile_owner(&self, package: Option<&str>) -> Result<bool, Exception> {
        if package.is_some() && self.host.device_policies()?.owned() {
            return Err(self.host.unimplemented_named("wifi", "a device or profile owner (#1686)"));
        }
        Ok(false)
    }

    /// `isAdmin`: a device or profile owner, or an OEM privileged admin.
    fn admin(&self, uid: i32, package: Option<&str>) -> Result<bool, Exception> {
        if package.is_none() {
            return Ok(false);
        }
        if self.device_or_profile_owner(package)? {
            return Ok(true);
        }
        let admins = &self.resources()?.oem_admins;
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        let packages = query.packages_for_uid(uid).map_err(|e| self.host.unimplemented_named("wifi", e.0))?;
        if !packages.unwrap_or_default().iter().flatten().any(|p| admins.contains(p)) {
            return Ok(false);
        }
        self.signed_with_platform_key(uid)
    }

    /// `isTargetSdkLessThanQOrPrivileged`: the app host's one user is not
    /// a guest.
    fn target_sdk_less_than_q_or_privileged(&self, package: Option<&str>, uid: i32) -> Result<bool, Exception> {
        let inputs = self.host.inputs()?;
        let info = package.and_then(|p| inputs.packages.application_info(p, 0));
        if info.as_ref().is_some_and(|ai| ai.target_sdk_version < Q) || self.privileged(uid)? || self.admin(uid, package)? {
            return Ok(true);
        }
        Ok(info.is_some_and(|ai| ai.flags & (FLAG_SYSTEM | FLAG_UPDATED_SYSTEM_APP) != 0))
    }

    /// `getConfiguredNetworks`: no carrier app without telephony.
    fn configured_networks(
        &self,
        uid: i32,
        package: Option<&str>,
        caller_networks_only: bool,
    ) -> Result<Slice<WifiConfiguration>, Exception> {
        self.enforce_access(uid)?;
        let none = || Slice::new(self.host.process.clone(), WIFI_CONFIGURATION, Vec::new());
        if uid != SHELL_UID && uid != ROOT_UID {
            // `WifiPermissionsUtil.checkPackage`.
            let name = package.ok_or_else(|| Exception::security(format!("Checking UID {uid} but Package Name is Null")))?;
            if !self.host.owns_package(uid, name)? {
                return Err(Exception::security(format!("Package {name} does not belong to {uid}")));
            }
            if !caller_networks_only {
                let checked = {
                    let _system = aim_binder_host::local::clear_calling_identity();
                    self.enforce_can_access_scan_results(package, uid)
                };
                match checked {
                    Ok(()) => {}
                    Err(e) if e.code == EX_SECURITY => {
                        eprintln!("app host: wifi: Permission violation - getConfiguredNetworks not allowed for uid={uid}, packageName={name}, reason={}", e.message);
                        return Ok(none());
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        let owner = self.device_or_profile_owner(package)?;
        let privileged = self.privileged(uid)?;
        if caller_networks_only && !owner && !privileged {
            return Err(Exception::security("Not a DO, PO, carrier or privileged app"));
        }
        let allowed = self.target_sdk_less_than_q_or_privileged(package, uid)?;
        if !allowed {
            eprintln!("app host: wifi: getConfiguredNetworks not allowed for uid={uid}");
            return Ok(none());
        }
        // `getSavedNetworks(targetConfigUid)`: MAC addresses for the
        // privileged, else for none.
        let target = if privileged { WIFI_UID } else { INVALID_UID };
        let resources = self.resources()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let saved: Vec<WifiConfiguration> = self
            .networks()?
            .iter()
            .filter(|c| !c.ephemeral)
            .map(|c| c.external(target, resources.mac_randomization_supported, now))
            .collect();
        // `shouldOmitAutoUpgradeParams`.
        let omit = |p: &SecurityParams| {
            p.added_by_auto_upgrade
                && match p.kind {
                    SECURITY_TYPE_SAE => !resources.sae_upgrade,
                    SECURITY_TYPE_OWE => !resources.owe_upgrade,
                    _ => false,
                }
        };
        let configs = if caller_networks_only {
            let own: Vec<WifiConfiguration> = saved.into_iter().filter(|c| c.creator_uid == uid).collect();
            wifi_config::legacy_configs(&own, true, &omit)
        } else {
            wifi_config::legacy_configs(&saved, false, &omit)
        };
        Ok(Slice::new(self.host.process.clone(), WIFI_CONFIGURATION, configs))
    }

    /// `ActiveModeWarden.getWifiState`: the Mac's radio, on or off.
    fn wifi_state(&self) -> i32 {
        if aim_host_wifi::wifi().is_some_and(|w| w.powered) { WIFI_STATE_ENABLED } else { WIFI_STATE_DISABLED }
    }
}

impl Service for WifiService {
    fn descriptor(&self) -> &str {
        wm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            wm::GET_WIFI_ENABLED_STATE => {
                wm::GetWifiEnabledState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_access(uid).map(|()| wm::write_get_wifi_enabled_state_reply(&mut p, self.wifi_state()))
            }
            wm::GET_VERBOSE_LOGGING_LEVEL => {
                wm::GetVerboseLoggingLevel::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.verbose_logging_level().map(|level| wm::write_get_verbose_logging_level_reply(&mut p, level))
            }
            wm::IS_SCAN_ALWAYS_AVAILABLE => {
                wm::IsScanAlwaysAvailable::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `WifiSettingsStore.isScanAlwaysAvailableToggleEnabled`.
                self.enforce_access(uid)
                    .and_then(|()| self.store("wifi_scan_always_enabled", false))
                    .map(|on| wm::write_is_scan_always_available_reply(&mut p, on))
            }
            wm::GET_CONNECTION_INFO => {
                let a = wm::GetConnectionInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.connection_info(uid, a.calling_package.as_deref()).map(|info| wm::write_get_connection_info_reply(&mut p, Some(&info)))
            }
            wm::GET_SCAN_RESULTS => {
                let a = wm::GetScanResults::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.scan_results(uid, a.calling_package.as_deref()).map(|r| wm::write_get_scan_results_reply(&mut p, r.as_ref()))
            }
            wm::GET_CONFIGURED_NETWORKS => {
                let a = wm::GetConfiguredNetworks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.configured_networks(uid, a.package_name.as_deref(), a.caller_networks_only)
                    .map(|r| wm::write_get_configured_networks_reply(&mut p, Some(&r)))
            }
            code => {
                let name = wm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("wifi", &format!("{name} (#1686)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `WifiInfo.CREATOR` reads no connection's info back to its
    /// defaults, field by field, to the end.
    #[test]
    fn no_connection_parcels_as_a_new_wifi_info() {
        let mut p = Parcel::new();
        NoConnection.write_to(&mut p);
        let mut r = p.reader();
        let ints = |r: &mut aim_binder_host::parcel::Reader<'_>, n: usize| (0..n).map(|_| r.read_i32().unwrap()).collect::<Vec<_>>();
        assert_eq!(ints(&mut r, 8), [-1, -127, -1, 0, 0, -1, 0, 0]);
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("02:00:00:00:00:00"));
        assert_eq!(ints(&mut r, 8), [0, 0, 0, 0, 0, 0, 0, 1]);
        for _ in 0..8 {
            assert_eq!(r.read_i64().unwrap(), 0);
        }
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("UNINITIALIZED"));
        assert_eq!(r.read_i32().unwrap(), 0);
        for _ in 0..3 {
            assert_eq!(r.read_string16().unwrap(), None);
        }
        assert_eq!(ints(&mut r, 3), [0, 0, 0]);
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(ints(&mut r, 5), [-1, -1, 0, -1, 0]);
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(ints(&mut r, 4), [-1, 0, 0, 0]);
        assert_eq!(r.remaining(), 0);
    }

    /// The settings of the shared store as the image's data has them.
    #[test]
    fn settings_are_read_from_the_shared_store() {
        let xml = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<WifiConfigStoreData>
<int name="Version" value="3" />
<Settings>
<map name="Values">
<boolean name="wifi_scan_always_enabled" value="false" />
<boolean name="wifi_verbose_logging_enabled" value="true" />
</map>
</Settings>
</WifiConfigStoreData>"#;
        let root = aim_android_xml::read(xml).unwrap();
        assert!(store_bool(&root, "wifi_verbose_logging_enabled", false).unwrap());
        assert!(!store_bool(&root, "wifi_scan_always_enabled", true).unwrap());
        assert!(store_bool(&root, "absent", true).unwrap());
    }
}
