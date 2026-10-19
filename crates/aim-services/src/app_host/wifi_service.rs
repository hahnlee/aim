//! `wifi`: WifiServiceImpl for the app host (`android-16.0.0_r1`), over
//! the Mac's Wi-Fi (`aim_host_wifi`): its radio is Android's Wi-Fi state;
//! the verbose logging level and scanning always available are what the
//! shared config store (`WifiConfigStore.xml`, `WifiSettingsConfigStore`)
//! persisted; with the radio off there is no connection (`new WifiInfo()`).
//! The rest is not implemented (#1686).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_net_wifi_iwifimanager as wm;

use super::{AppHost, reply};

const ACCESS_WIFI_STATE: &str = "android.permission.ACCESS_WIFI_STATE";
/// `WifiManager.WIFI_STATE_DISABLED`, `WIFI_STATE_ENABLED`.
const WIFI_STATE_DISABLED: i32 = 1;
const WIFI_STATE_ENABLED: i32 = 3;
/// `WifiManager.VERBOSE_LOGGING_LEVEL_DISABLED`, `_ENABLED`.
const VERBOSE_LOGGING_LEVEL_DISABLED: i32 = 0;
const VERBOSE_LOGGING_LEVEL_ENABLED: i32 = 1;
/// The shared general store (`WifiConfigStore.STORE_FILE_SHARED_GENERAL`).
const SHARED_STORE: &str = "misc/apexdata/com.android.wifi/WifiConfigStore.xml";

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

pub struct WifiService {
    host: Arc<AppHost>,
}

impl WifiService {
    pub fn new(host: Arc<AppHost>) -> Self {
        WifiService { host }
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
