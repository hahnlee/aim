//! The saved networks of the app host's `wifi` (packages/modules/Wifi at
//! `android-16.0.0_r1`): `WifiConfigManager.loadFromStore` over the shared
//! and user `WifiConfigStore.xml` (`NetworkListStoreData.parseNetwork`,
//! `XmlUtil.WifiConfigurationXmlUtil`, `NetworkSelectionStatusXmlUtil`,
//! `IpConfigurationXmlUtil`, `WifiConfigurationUtil.validate`), the copies
//! `getSavedNetworks` hands out (`createExternalWifiConfiguration`),
//! `convertMultiTypeConfigsToLegacyConfigs`, and `WifiConfiguration`'s
//! parcel. Open, WPA/WPA2/WPA3-Personal, OWE and WAPI-PSK networks over
//! DHCP are modelled; enterprise, WEP, Passpoint, DPP, static IP and proxy
//! settings, encrypted credentials, linked subscriptions, BSSID allow lists,
//! vendor data, merged duplicates and networks without a persisted
//! randomized MAC address are not implemented (#1686).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_android_xml::{Element, Node, TEXT};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

/// `WifiConfiguration.KeyMgmt`.
mod key_mgmt {
    pub const NONE: u32 = 0;
    pub const WPA_PSK: u32 = 1;
    pub const WPA_EAP: u32 = 2;
    pub const IEEE8021X: u32 = 3;
    pub const WPA2_PSK: u32 = 4;
    pub const OSEN: u32 = 5;
    pub const FT_PSK: u32 = 6;
    pub const FT_EAP: u32 = 7;
    pub const SAE: u32 = 8;
    pub const OWE: u32 = 9;
    pub const SUITE_B_192: u32 = 10;
    pub const WPA_PSK_SHA256: u32 = 11;
    pub const WPA_EAP_SHA256: u32 = 12;
    pub const WAPI_PSK: u32 = 13;
    pub const WAPI_CERT: u32 = 14;
    pub const FILS_SHA256: u32 = 15;
    pub const FILS_SHA384: u32 = 16;
    pub const DPP: u32 = 17;
    /// `KeyMgmt.strings`.
    pub const STRINGS: [&str; 18] = [
        "NONE", "WPA_PSK", "WPA_EAP", "IEEE8021X", "WPA2_PSK", "OSEN", "FT_PSK", "FT_EAP", "SAE", "OWE",
        "SUITE_B_192", "WPA_PSK_SHA256", "WPA_EAP_SHA256", "WAPI_PSK", "WAPI_CERT", "FILS_SHA256", "FILS_SHA384",
        "DPP",
    ];
}

/// `WifiConfiguration.Protocol`, `PairwiseCipher`, `GroupCipher`,
/// `SuiteBCipher`.
const PROTOCOL_WPA: u32 = 0;
const PROTOCOL_RSN: u32 = 1;
const PROTOCOL_WAPI: u32 = 3;
const PAIRWISE_TKIP: u32 = 1;
const PAIRWISE_CCMP: u32 = 2;
const PAIRWISE_GCMP_256: u32 = 3;
const PAIRWISE_SMS4: u32 = 4;
const PAIRWISE_GCMP_128: u32 = 5;
const GROUP_WEP40: u32 = 0;
const GROUP_WEP104: u32 = 1;
const GROUP_TKIP: u32 = 2;
const GROUP_CCMP: u32 = 3;
const GROUP_GCMP_256: u32 = 5;
const GROUP_SMS4: u32 = 6;
const GROUP_GCMP_128: u32 = 7;
const SUITE_B_ECDHE_ECDSA: u32 = 0;
const SUITE_B_ECDHE_RSA: u32 = 1;
/// The lengths of `KeyMgmt`, `Protocol`, `AuthAlgorithm`, `GroupCipher`
/// and `PairwiseCipher`'s `strings` (`validateBitSets`).
const PROTOCOL_COUNT: u32 = 4;
const AUTH_ALGORITHM_COUNT: u32 = 4;
const GROUP_CIPHER_COUNT: u32 = 8;
const PAIRWISE_CIPHER_COUNT: u32 = 6;

/// `WifiConfiguration.SECURITY_TYPE_*`.
pub const SECURITY_TYPE_OPEN: i32 = 0;
const SECURITY_TYPE_WEP: i32 = 1;
pub const SECURITY_TYPE_PSK: i32 = 2;
const SECURITY_TYPE_EAP: i32 = 3;
pub const SECURITY_TYPE_SAE: i32 = 4;
const SECURITY_TYPE_EAP_WPA3_ENTERPRISE_192_BIT: i32 = 5;
pub const SECURITY_TYPE_OWE: i32 = 6;
const SECURITY_TYPE_WAPI_PSK: i32 = 7;
const SECURITY_TYPE_WAPI_CERT: i32 = 8;
const SECURITY_TYPE_EAP_WPA3_ENTERPRISE: i32 = 9;
const SECURITY_TYPE_OSEN: i32 = 10;
const SECURITY_TYPE_DPP: i32 = 13;
/// `SECURITY_TYPE_NUM`.
const SECURITY_TYPE_NUM: i32 = SECURITY_TYPE_DPP;

/// `WifiConfiguration.Status`.
const STATUS_CURRENT: i32 = 0;
const STATUS_ENABLED: i32 = 2;
/// `WifiConfiguration.RANDOMIZATION_*`.
pub const RANDOMIZATION_NONE: i32 = 0;
const RANDOMIZATION_PERSISTENT: i32 = 1;
const RANDOMIZATION_AUTO: i32 = 3;
/// `NetworkSelectionStatus`.
const NETWORK_SELECTION_STATUS: [&str; 3] =
    ["NETWORK_SELECTION_ENABLED", "NETWORK_SELECTION_TEMPORARY_DISABLED", "NETWORK_SELECTION_PERMANENTLY_DISABLED"];
const NETWORK_SELECTION_ENABLED: i32 = 0;
const NETWORK_SELECTION_TEMPORARY_DISABLED: i32 = 1;
const NETWORK_SELECTION_PERMANENTLY_DISABLED: i32 = 2;
const DISABLED_NONE: i32 = 0;
const DISABLED_TRANSITION_DISABLE_INDICATION: i32 = 13;
const NETWORK_SELECTION_DISABLED_MAX: usize = 16;
/// `DISABLE_REASON_INFOS`' persisted strings, by reason.
const DISABLE_REASONS: [&str; NETWORK_SELECTION_DISABLED_MAX] = [
    "NETWORK_SELECTION_ENABLE",
    // A space ends it, as persisted.
    "NETWORK_SELECTION_DISABLED_ASSOCIATION_REJECTION ",
    "NETWORK_SELECTION_DISABLED_AUTHENTICATION_FAILURE",
    "NETWORK_SELECTION_DISABLED_DHCP_FAILURE",
    "NETWORK_SELECTION_DISABLED_NO_INTERNET_TEMPORARY",
    "NETWORK_SELECTION_DISABLED_AUTHENTICATION_NO_CREDENTIALS",
    "NETWORK_SELECTION_DISABLED_NO_INTERNET_PERMANENT",
    "NETWORK_SELECTION_DISABLED_BY_WIFI_MANAGER",
    "NETWORK_SELECTION_DISABLED_BY_WRONG_PASSWORD",
    "NETWORK_SELECTION_DISABLED_AUTHENTICATION_NO_SUBSCRIPTION",
    "NETWORK_SELECTION_DISABLED_AUTHENTICATION_PRIVATE_EAP_ERROR",
    "NETWORK_SELECTION_DISABLED_NETWORK_NOT_FOUND",
    "NETWORK_SELECTION_DISABLED_CONSECUTIVE_FAILURES",
    "NETWORK_SELECTION_DISABLED_TRANSITION_DISABLE_INDICATION",
    "NETWORK_SELECTION_DISABLED_UNWANTED_LOW_RSSI",
    "NETWORK_SELECTION_DISABLED_REPEATED_NUD_FAILURES",
];
/// `CONNECT_CHOICE_EXISTS`, `CONNECT_CHOICE_NOT_EXISTS`.
const CONNECT_CHOICE_EXISTS: i32 = 1;
const CONNECT_CHOICE_NOT_EXISTS: i32 = -1;
/// `WifiInfo.DEFAULT_MAC_ADDRESS`.
const DEFAULT_MAC_ADDRESS: u64 = 0x0200_0000_0000;
/// `WifiConfigurationUtil`'s lengths.
const SSID_PLAINTEXT_MAX_LEN: usize = 32 + 2;
const SSID_HEX_MAX_LEN: usize = 64;
const PSK_ASCII_MIN_LEN: usize = 8 + 2;
const SAE_ASCII_MIN_LEN: usize = 1 + 2;
const PSK_SAE_ASCII_MAX_LEN: usize = 63 + 2;
const PSK_SAE_HEX_LEN: usize = 64;
const MAX_STRING_LENGTH: usize = 512;
/// `WifiConfigManager.PASSWORD_MASK`.
const PASSWORD_MASK: &str = "*";

/// A `BitSet` of the small indices the Wi-Fi sets use.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bits(u64);

impl Bits {
    fn of(bits: &[u32]) -> Bits {
        Bits(bits.iter().fold(0, |b, i| b | 1 << i))
    }

    fn get(self, i: u32) -> bool {
        self.0 & 1 << i != 0
    }

    fn cardinality(self) -> u32 {
        self.0.count_ones()
    }

    /// `BitSet.valueOf(bytes)`: little-endian bits.
    fn from_bytes(bytes: &[u8]) -> Result<Bits, LoadError> {
        let mut v = 0u64;
        for (i, b) in bytes.iter().enumerate() {
            if *b == 0 {
                continue;
            }
            if i >= 8 {
                return Err(LoadError::Unmodelled("a bit set past 64 bits"));
            }
            v |= u64::from(*b) << (8 * i);
        }
        Ok(Bits(v))
    }

    /// `validateBitSet`: no bit at or past `length`.
    fn within(self, length: u32) -> bool {
        self.0 >> length == 0
    }

    /// `writeBitSet`: its cardinality, then its set bits in order.
    fn write(self, p: &mut Parcel) {
        p.write_i32(self.cardinality() as i32);
        for i in 0..64 {
            if self.get(i) {
                p.write_i32(i as i32);
            }
        }
    }
}

/// Why a store or a network of it did not load.
#[derive(Debug, PartialEq)]
pub enum LoadError {
    /// A `RuntimeException` parsing a network: `parseNetworkList` skips it.
    Skip(String),
    /// An `XmlPullParserException`: the read of the stores fails.
    Corrupt(String),
    /// What the app host does not model.
    Unmodelled(&'static str),
}

/// `SecurityParams`.
#[derive(Clone, Debug, PartialEq)]
pub struct SecurityParams {
    pub kind: i32,
    pub enabled: bool,
    key_mgmt: Bits,
    protocols: Bits,
    auth: Bits,
    pairwise: Bits,
    group: Bits,
    group_mgmt: Bits,
    suite_b: Bits,
    require_pmf: bool,
    sae_h2e_only: bool,
    sae_pk_only: bool,
    pub added_by_auto_upgrade: bool,
}

impl SecurityParams {
    /// `createSecurityParamsBySecurityType` of the types modelled.
    fn of(kind: i32) -> Result<SecurityParams, LoadError> {
        use key_mgmt::*;
        let (key_mgmt, protocols, pairwise, group, require_pmf) = match kind {
            SECURITY_TYPE_OPEN => (Bits::of(&[NONE]), Bits::of(&[PROTOCOL_RSN, PROTOCOL_WPA]), Bits::default(), Bits::default(), false),
            SECURITY_TYPE_PSK => (
                Bits::of(&[WPA_PSK]),
                Bits::of(&[PROTOCOL_RSN, PROTOCOL_WPA]),
                Bits::of(&[PAIRWISE_CCMP, PAIRWISE_TKIP]),
                Bits::of(&[GROUP_CCMP, GROUP_TKIP, GROUP_WEP40, GROUP_WEP104]),
                false,
            ),
            SECURITY_TYPE_SAE => (
                Bits::of(&[SAE]),
                Bits::of(&[PROTOCOL_RSN]),
                Bits::of(&[PAIRWISE_CCMP, PAIRWISE_GCMP_128, PAIRWISE_GCMP_256]),
                Bits::of(&[GROUP_CCMP, GROUP_GCMP_128, GROUP_GCMP_256]),
                true,
            ),
            SECURITY_TYPE_OWE => (
                Bits::of(&[OWE]),
                Bits::of(&[PROTOCOL_RSN]),
                Bits::of(&[PAIRWISE_CCMP, PAIRWISE_GCMP_128, PAIRWISE_GCMP_256]),
                Bits::of(&[GROUP_CCMP, GROUP_GCMP_128, GROUP_GCMP_256]),
                true,
            ),
            SECURITY_TYPE_WAPI_PSK => {
                (Bits::of(&[WAPI_PSK]), Bits::of(&[PROTOCOL_WAPI]), Bits::of(&[PAIRWISE_SMS4]), Bits::of(&[GROUP_SMS4]), false)
            }
            SECURITY_TYPE_WEP => return Err(LoadError::Unmodelled("WEP networks")),
            SECURITY_TYPE_EAP | SECURITY_TYPE_EAP_WPA3_ENTERPRISE | SECURITY_TYPE_EAP_WPA3_ENTERPRISE_192_BIT
            | SECURITY_TYPE_WAPI_CERT | SECURITY_TYPE_OSEN | 11 | 12 => {
                return Err(LoadError::Unmodelled("enterprise and Passpoint networks"));
            }
            SECURITY_TYPE_DPP => return Err(LoadError::Unmodelled("DPP networks")),
            _ => return Err(LoadError::Skip(format!("unknown security type {kind}"))),
        };
        Ok(SecurityParams {
            kind,
            enabled: true,
            key_mgmt,
            protocols,
            auth: Bits::default(),
            pairwise,
            group,
            group_mgmt: Bits::default(),
            suite_b: Bits::default(),
            require_pmf,
            sae_h2e_only: false,
            sae_pk_only: false,
            added_by_auto_upgrade: false,
        })
    }

    /// `isOpenSecurityType`.
    fn is_open(&self) -> bool {
        matches!(self.kind, SECURITY_TYPE_OPEN | SECURITY_TYPE_OWE)
    }
}

impl WriteParcelable for SecurityParams {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.kind);
        p.write_i32(self.enabled as i32);
        for bits in [self.key_mgmt, self.protocols, self.auth, self.pairwise, self.group, self.group_mgmt, self.suite_b] {
            bits.write(p);
        }
        p.write_i32(self.require_pmf as i32);
        p.write_i32(self.added_by_auto_upgrade as i32);
        p.write_i32(self.sae_h2e_only as i32);
        p.write_i32(self.sae_pk_only as i32);
    }
}

/// `WifiConfiguration.NetworkSelectionStatus`, as persisted.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkSelectionStatus {
    pub status: i32,
    pub disable_reason: i32,
    pub counters: [i32; NETWORK_SELECTION_DISABLED_MAX],
    connect_choice: Option<String>,
    connect_choice_rssi: i32,
    has_ever_connected: bool,
    never_detected_captive_portal: bool,
    ever_validated_internet_access: bool,
}

impl NetworkSelectionStatus {
    /// `writeToParcel`: no disable times, BSSID or candidate params are
    /// kept across a load.
    fn write(&self, p: &mut Parcel) {
        p.write_i32(self.status);
        p.write_i32(self.disable_reason);
        for counter in self.counters {
            p.write_i32(counter);
        }
        p.write_i64(-1); // INVALID_NETWORK_SELECTION_DISABLE_TIMESTAMP
        p.write_i64(-1);
        p.write_string16(None); // network selection BSSID
        match &self.connect_choice {
            Some(choice) => {
                p.write_i32(CONNECT_CHOICE_EXISTS);
                p.write_string16(Some(choice));
                p.write_i32(self.connect_choice_rssi);
            }
            None => p.write_i32(CONNECT_CHOICE_NOT_EXISTS),
        }
        p.write_i32(self.has_ever_connected as i32);
        p.write_i32(self.never_detected_captive_portal as i32);
        p.write_i32(self.ever_validated_internet_access as i32);
        p.write_string16(None); // candidate security params
        p.write_string16(None); // last used security params
    }
}

/// A saved `WifiConfiguration`, as far as its parcel carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct WifiConfiguration {
    pub network_id: i32,
    pub status: i32,
    pub selection: NetworkSelectionStatus,
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub pre_shared_key: Option<String>,
    priority: i32,
    deletion_priority: i32,
    hidden_ssid: bool,
    require_pmf: bool,
    key_mgmt: Bits,
    protocols: Bits,
    auth: Bits,
    pairwise: Bits,
    group: Bits,
    group_mgmt: Bits,
    suite_b: Bits,
    pub security: Vec<SecurityParams>,
    default_gw_mac_address: Option<String>,
    validated_internet_access: bool,
    pub ephemeral: bool,
    trusted: bool,
    oem_paid: bool,
    oem_private: bool,
    carrier_merged: bool,
    metered_hint: bool,
    repeater_enabled: bool,
    metered_override: i32,
    use_external_scores: bool,
    pub creator_uid: i32,
    last_connect_uid: i32,
    last_update_uid: i32,
    pub creator_name: Option<String>,
    last_update_name: Option<String>,
    allow_autojoin: bool,
    no_internet_access_expected: bool,
    pub shared: bool,
    pub randomized_mac: u64,
    pub mac_randomization_setting: i32,
    send_dhcp_hostname: bool,
    pub randomized_mac_last_modified_ms: i64,
    carrier_id: i32,
    subscription_id: i32,
    restricted: bool,
    dpp_configurator: bool,
    wifi7_enabled: bool,
}

impl Default for WifiConfiguration {
    /// `new WifiConfiguration()`.
    fn default() -> Self {
        WifiConfiguration {
            network_id: -1,
            status: 0,
            selection: NetworkSelectionStatus {
                status: NETWORK_SELECTION_ENABLED,
                disable_reason: DISABLED_NONE,
                counters: [0; NETWORK_SELECTION_DISABLED_MAX],
                connect_choice: None,
                connect_choice_rssi: 0,
                has_ever_connected: false,
                never_detected_captive_portal: true,
                ever_validated_internet_access: false,
            },
            ssid: None,
            bssid: None,
            pre_shared_key: None,
            priority: 0,
            deletion_priority: 0,
            hidden_ssid: false,
            require_pmf: false,
            key_mgmt: Bits::default(),
            protocols: Bits::default(),
            auth: Bits::default(),
            pairwise: Bits::default(),
            group: Bits::default(),
            group_mgmt: Bits::default(),
            suite_b: Bits::default(),
            security: Vec::new(),
            default_gw_mac_address: None,
            validated_internet_access: false,
            ephemeral: false,
            trusted: true,
            oem_paid: false,
            oem_private: false,
            carrier_merged: false,
            metered_hint: false,
            repeater_enabled: false,
            metered_override: 0,
            use_external_scores: false,
            creator_uid: -1,
            last_connect_uid: 0,
            last_update_uid: -1,
            creator_name: None,
            last_update_name: None,
            allow_autojoin: true,
            no_internet_access_expected: false,
            shared: true,
            randomized_mac: DEFAULT_MAC_ADDRESS,
            mac_randomization_setting: RANDOMIZATION_AUTO,
            send_dhcp_hostname: true,
            randomized_mac_last_modified_ms: 0,
            carrier_id: -1,
            subscription_id: -1,
            restricted: false,
            dpp_configurator: false,
            wifi7_enabled: true,
        }
    }
}

impl WifiConfiguration {
    /// `updateLegacySecurityParams`: the first params' sets.
    fn update_legacy_security(&mut self) {
        let Some(p) = self.security.first() else { return };
        (self.key_mgmt, self.protocols, self.auth, self.pairwise, self.group, self.group_mgmt, self.suite_b) =
            (p.key_mgmt, p.protocols, p.auth, p.pairwise, p.group, p.group_mgmt, p.suite_b);
        self.require_pmf = p.require_pmf;
    }

    /// `setSecurityParams(SecurityParams)` and `setSecurityParams(int)`.
    pub fn set_security(&mut self, params: SecurityParams) {
        self.security = vec![params];
        self.update_legacy_security();
    }

    /// `isSecurityType`.
    pub fn is_security_type(&self, kind: i32) -> bool {
        self.security.iter().any(|p| p.kind == kind)
    }

    /// `isOpenNetwork`: no WEP key is ever modelled.
    fn is_open(&self) -> bool {
        self.security.iter().all(SecurityParams::is_open)
    }

    /// `addSecurityParams(SecurityParams)` on a personal configuration.
    fn add_security(&mut self, params: SecurityParams) -> Result<(), String> {
        if self.security.iter().any(|p| p.kind == params.kind) {
            return Err(format!("duplicate security params {}", params.kind));
        }
        if !self.security.is_empty() && params.is_open() != self.is_open() {
            return Err("an open and a non-open security type".into());
        }
        self.security.push(params);
        self.update_legacy_security();
        Ok(())
    }

    /// `convertLegacyFieldsToSecurityParamsIfNeeded`.
    fn convert_legacy_fields(&mut self) -> Result<(), LoadError> {
        use key_mgmt::*;
        if !self.security.is_empty() {
            return Ok(());
        }
        let k = self.key_mgmt;
        let kind = if k.get(WAPI_CERT) {
            SECURITY_TYPE_WAPI_CERT
        } else if k.get(WAPI_PSK) {
            SECURITY_TYPE_WAPI_PSK
        } else if k.get(SUITE_B_192) {
            SECURITY_TYPE_EAP_WPA3_ENTERPRISE_192_BIT
        } else if k.get(DPP) {
            SECURITY_TYPE_DPP
        } else if k.get(OWE) {
            SECURITY_TYPE_OWE
        } else if k.get(SAE) {
            SECURITY_TYPE_SAE
        } else if k.get(OSEN) {
            SECURITY_TYPE_OSEN
        } else if k.get(WPA2_PSK) || k.get(WPA_PSK_SHA256) || k.get(FT_PSK) {
            SECURITY_TYPE_PSK
        } else if k.get(WPA_EAP) || k.get(FT_EAP) || k.get(IEEE8021X) || k.get(WPA_EAP_SHA256) || k.get(FILS_SHA256) || k.get(FILS_SHA384) {
            SECURITY_TYPE_EAP
        } else if k.get(WPA_PSK) {
            SECURITY_TYPE_PSK
        } else {
            SECURITY_TYPE_OPEN
        };
        self.set_security(SecurityParams::of(kind)?);
        Ok(())
    }

    /// `needsPreSharedKey`.
    fn needs_pre_shared_key(&self) -> bool {
        self.security.iter().any(|p| matches!(p.kind, SECURITY_TYPE_PSK | SECURITY_TYPE_SAE | SECURITY_TYPE_WAPI_PSK))
    }

    /// `getDefaultSecurityType`, with no WEP key.
    fn default_security_type(&self) -> &'static str {
        use key_mgmt::*;
        let k = self.key_mgmt;
        let found = [WPA_PSK, WPA_EAP, OWE, SAE, SUITE_B_192, WAPI_PSK, WAPI_CERT, OSEN, DPP]
            .into_iter()
            .find(|&i| k.get(i) || (i == WPA_EAP && k.get(IEEE8021X)));
        STRINGS[found.unwrap_or(NONE) as usize]
    }

    /// `getProfileKey` of a saved network: its SSID and default security
    /// type, and its creator's user if not shared.
    pub fn profile_key(&self) -> String {
        let ssid = match self.ssid.as_deref() {
            Some(s) if !s.is_empty() && !s.starts_with('"') => s.to_lowercase(),
            s => s.unwrap_or("null").to_string(),
        };
        let mut key = ssid + self.default_security_type();
        if !self.shared {
            key += &format!("-{}", self.creator_uid.div_euclid(100_000));
        }
        key
    }

    /// `createExternalWifiConfiguration(config, true, targetUid)` at
    /// `now`, the wall clock in ms: the password masked, the randomized MAC
    /// address but to the Wi-Fi stack, the system and the creator, and no
    /// randomization where it is not supported.
    pub fn external(&self, target_uid: i32, mac_randomization_supported: bool, now: i64) -> WifiConfiguration {
        let mut c = self.clone();
        if c.pre_shared_key.as_deref().is_some_and(|k| !k.is_empty()) {
            c.pre_shared_key = Some(PASSWORD_MASK.into());
        }
        if !matches!(target_uid, 1010 | 1000) && target_uid != self.creator_uid {
            c.randomized_mac = DEFAULT_MAC_ADDRESS;
            c.randomized_mac_last_modified_ms = now;
        }
        if !mac_randomization_supported {
            c.mac_randomization_setting = RANDOMIZATION_NONE;
        }
        c
    }
}

/// `WifiConfiguration.writeToParcel`.
impl WriteParcelable for WifiConfiguration {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.network_id);
        p.write_i32(self.status);
        self.selection.write(p);
        p.write_string16(self.ssid.as_deref());
        p.write_string16(self.bssid.as_deref());
        p.write_i32(0); // apBand: AP_BAND_2GHZ
        p.write_i32(0); // apChannel
        p.write_string16(None); // FQDN
        p.write_string16(None); // providerFriendlyName
        p.write_i32(0); // isHomeProviderNetwork
        p.write_i32(0); // no roaming consortium ids
        p.write_string16(self.pre_shared_key.as_deref());
        for _ in 0..4 {
            p.write_string16(None); // wepKeys
        }
        p.write_i32(0); // wepTxKeyIndex
        p.write_i32(self.priority);
        p.write_i32(self.deletion_priority);
        p.write_i32(self.hidden_ssid as i32);
        p.write_i32(self.require_pmf as i32);
        p.write_string16(None); // updateIdentifier
        for bits in [self.key_mgmt, self.protocols, self.auth, self.pairwise, self.group, self.group_mgmt, self.suite_b] {
            bits.write(p);
        }
        p.write_i32(self.security.len() as i32);
        for params in &self.security {
            p.write_string16(Some("android.net.wifi.SecurityParams"));
            params.write_to(p);
        }
        // `new WifiEnterpriseConfig()`.
        p.write_string16(Some("android.net.wifi.WifiEnterpriseConfig"));
        crate::bundle::write(p, &[]);
        p.write_i32(-1); // Eap.NONE
        p.write_i32(0); // Phase2.NONE
        p.write_i32(0); // no CA certificates
        p.write_string16(None); // no client private key
        p.write_i32(0); // no client certificates
        p.write_string16(None); // key chain alias
        for _ in 0..2 {
            p.write_i32(0); // app installed key and cert, CA cert
        }
        p.write_i32(0); // OCSP_NONE
        p.write_i32(0); // trust on first use
        p.write_i32(0); // user approved no CA cert
        p.write_i32(0); // TLS_V1_0
        p.write_i32(0); // TOFU_DIALOG_STATE_UNSPECIFIED
        p.write_i32(0); // TOFU_STATE_NOT_ENABLED
        // `IpConfiguration` of DHCP without a proxy.
        p.write_string16(Some("android.net.IpConfiguration"));
        p.write_string16(Some("DHCP"));
        p.write_string16(Some("NONE"));
        p.write_string16(None);
        p.write_string16(None);
        p.write_string16(None); // dhcpServer
        p.write_string16(self.default_gw_mac_address.as_deref());
        p.write_i32(self.validated_internet_access as i32);
        p.write_i32(0); // isLegacyPasspointConfig
        p.write_i32(self.ephemeral as i32);
        p.write_i32(self.trusted as i32);
        p.write_i32(self.oem_paid as i32);
        p.write_i32(self.oem_private as i32);
        p.write_i32(self.carrier_merged as i32);
        p.write_i32(0); // fromWifiNetworkSuggestion
        p.write_i32(0); // fromWifiNetworkSpecifier
        p.write_i32(self.metered_hint as i32);
        p.write_i32(self.repeater_enabled as i32);
        p.write_i32(self.metered_override);
        p.write_i32(self.use_external_scores as i32);
        p.write_i32(self.creator_uid);
        p.write_i32(self.last_connect_uid);
        p.write_i32(self.last_update_uid);
        p.write_string16(self.creator_name.as_deref());
        p.write_string16(self.last_update_name.as_deref());
        p.write_i32(0); // numScorerOverride
        p.write_i32(0); // numScorerOverrideAndSwitchedNetwork
        p.write_i32(0); // numAssociation
        p.write_i32(self.allow_autojoin as i32);
        p.write_i32(0); // numNoInternetAccessReports
        p.write_i32(self.no_internet_access_expected as i32);
        p.write_i32(self.shared as i32);
        p.write_string16(None); // Passpoint management object tree
        p.write_i32(0); // RECENT_FAILURE_NONE
        p.write_i64(0); // its time
        p.write_string16(Some("android.net.MacAddress"));
        p.write_i64(self.randomized_mac as i64);
        p.write_i32(self.mac_randomization_setting);
        p.write_i32(self.send_dhcp_hostname as i32);
        p.write_i32(0); // osu
        p.write_i64(0); // randomizedMacExpirationTimeMs
        p.write_i64(self.randomized_mac_last_modified_ms);
        p.write_i32(self.carrier_id);
        p.write_string16(None); // Passpoint unique id
        p.write_i32(self.subscription_id);
        p.write_i32(self.restricted as i32);
        p.write_string16(None); // subscription group
        p.write_i32(-1); // no BSSID allow list
        p.write_i32(self.dpp_configurator as i32);
        for _ in 0..4 {
            p.write_i32(0); // the DPP private EC key, connector, C-sign and net access keys
        }
        p.write_i32(0); // isCurrentlyConnected
        p.write_i32(0); // user selected
        p.write_i32(0); // pre-shared key changed
        p.write_i32(0); // no encrypted pre-shared key
        p.write_i32(0); // nor its IV
        p.write_i32(0); // IP provisioning timed out
        p.write_i32(0); // no vendor data
        p.write_i32(self.wifi7_enabled as i32);
    }
}

/// An element's text.
fn text(e: &Element) -> String {
    e.content
        .iter()
        .filter_map(|n| match n {
            Node::Token(TEXT, Some(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

/// A value `XmlUtils.readThisValueXml` reads, of the kinds the store
/// writes.
#[derive(Debug, PartialEq)]
enum Value {
    Null,
    Str(String),
    Int(i32),
    Long(i64),
    Bool(bool),
    Bytes(Vec<u8>),
    Longs(Vec<i64>),
    Other,
}

fn value(e: &Element) -> Result<Value, LoadError> {
    let corrupt = |m: String| LoadError::Corrupt(format!("<{}>: {m}", e.name));
    let attr = |name: &str| e.string(name).ok_or_else(|| corrupt(format!("no {name}")));
    Ok(match e.name.as_str() {
        "null" => Value::Null,
        "string" => Value::Str(text(e)),
        "int" => Value::Int(attr("value")?.parse().map_err(|_| corrupt("invalid int".into()))?),
        "long" => Value::Long(attr("value")?.parse().map_err(|_| corrupt("invalid long".into()))?),
        // `Boolean.parseBoolean`.
        "boolean" => Value::Bool(attr("value")?.eq_ignore_ascii_case("true")),
        "byte-array" => {
            let hex = text(e);
            let num: usize = attr("num")?.parse().map_err(|_| corrupt("invalid num".into()))?;
            if hex.len() != num * 2 {
                return Err(corrupt("invalid byte-array length".into()));
            }
            Value::Bytes(
                (0..num).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)).collect::<Result<_, _>>().map_err(|_| corrupt("invalid hex".into()))?,
            )
        }
        "long-array" => Value::Longs(
            e.children()
                .map(|item| item.string("value").and_then(|v| v.parse().ok()).ok_or_else(|| corrupt("invalid item".into())))
                .collect::<Result<_, _>>()?,
        ),
        _ => Value::Other,
    })
}

/// A cast of a value, as `(String) value` or `(int) value` cast it: a
/// `ClassCastException`, or a `NullPointerException` unboxing a null,
/// skips the network.
fn skip(name: &str) -> LoadError {
    LoadError::Skip(format!("value {name} of an unexpected type"))
}

fn as_string(v: Value, name: &str) -> Result<Option<String>, LoadError> {
    match v {
        Value::Null => Ok(None),
        Value::Str(s) => Ok(Some(s)),
        _ => Err(skip(name)),
    }
}

fn as_int(v: Value, name: &str) -> Result<i32, LoadError> {
    match v {
        Value::Int(i) => Ok(i),
        _ => Err(skip(name)),
    }
}

fn as_bool(v: Value, name: &str) -> Result<bool, LoadError> {
    match v {
        Value::Bool(b) => Ok(b),
        _ => Err(skip(name)),
    }
}

fn as_bits(v: Value, name: &str) -> Result<Bits, LoadError> {
    match v {
        Value::Bytes(b) => Bits::from_bytes(&b),
        _ => Err(skip(name)),
    }
}

/// `parseSecurityParamsFromXml`.
fn parse_security_params(e: &Element) -> Result<Option<SecurityParams>, LoadError> {
    let mut params: Option<SecurityParams> = None;
    for child in e.children() {
        let name = child.string("name").map(|n| n.into_owned()).ok_or_else(|| LoadError::Corrupt("Missing value name".into()))?;
        let v = value(child)?;
        let missing = || LoadError::Corrupt("Missing security type.".into());
        match name.as_str() {
            "SecurityType" => params = Some(SecurityParams::of(as_int(v, &name)?)?),
            "IsEnabled" => params.as_mut().ok_or_else(|| LoadError::Skip("IsEnabled before SecurityType".into()))?.enabled = as_bool(v, &name)?,
            "SaeIsH2eOnlyMode" => params.as_mut().ok_or_else(missing)?.sae_h2e_only = as_bool(v, &name)?,
            "SaeIsPkOnlyMode" => params.as_mut().ok_or_else(missing)?.sae_pk_only = as_bool(v, &name)?,
            "IsAddedByAutoUpgrade" => params.as_mut().ok_or_else(missing)?.added_by_auto_upgrade = as_bool(v, &name)?,
            "AllowedSuiteBCiphers" => {
                let bits = as_bits(v, &name)?;
                let p = params.as_mut().ok_or_else(missing)?;
                p.suite_b = Bits(
                    (p.suite_b.0 & !(Bits::of(&[SUITE_B_ECDHE_ECDSA, SUITE_B_ECDHE_RSA]).0))
                        | (bits.0 & Bits::of(&[SUITE_B_ECDHE_ECDSA, SUITE_B_ECDHE_RSA]).0),
                );
            }
            _ => {}
        }
    }
    Ok(params)
}

/// `WifiConfigurationXmlUtil.parseFromXml(.., fromSuggestion: false)`.
fn parse_configuration(e: &Element) -> Result<WifiConfiguration, LoadError> {
    let mut c = WifiConfiguration::default();
    let mut mac_randomization_exists = false;
    let mut send_dhcp_hostname_exists = false;
    let mut dpp_keys: [Option<Vec<u8>>; 3] = [None, None, None];
    for child in e.children() {
        let Some(name) = child.string("name").map(|n| n.into_owned()) else {
            match child.name.as_str() {
                "SecurityParamsList" => {
                    let list =
                        child.children().filter(|s| s.name == "SecurityParams").map(parse_security_params).collect::<Result<Vec<_>, _>>()?;
                    let list: Vec<SecurityParams> = list.into_iter().flatten().collect();
                    if !list.is_empty() {
                        c.security = list;
                        c.update_legacy_security();
                    }
                }
                "PreSharedKey" | "WEPKeys" | "DppPrivateEcKey" | "DppConnector" | "DppCSignKey" | "DppNetAccessKey" => {
                    return Err(LoadError::Unmodelled("encrypted credentials"));
                }
                "VendorDataList" => return Err(LoadError::Unmodelled("vendor data")),
                _ => {}
            }
            continue;
        };
        let v = value(child)?;
        match name.as_str() {
            "ConfigKey" => {}
            "SSID" => c.ssid = as_string(v, &name)?,
            "BSSID" => c.bssid = as_string(v, &name)?,
            "PreSharedKey" => c.pre_shared_key = as_string(v, &name)?,
            "WEPKeys" if v != Value::Null => return Err(LoadError::Unmodelled("WEP keys")),
            "WEPKeys" => {}
            "WEPTxKeyIndex" => {
                if as_int(v, &name)? != 0 {
                    return Err(LoadError::Unmodelled("WEP keys"));
                }
            }
            "HiddenSSID" => c.hidden_ssid = as_bool(v, &name)?,
            "RequirePMF" => c.require_pmf = as_bool(v, &name)?,
            "AllowedKeyMgmt" => c.key_mgmt = as_bits(v, &name)?,
            "AllowedProtocols" => c.protocols = as_bits(v, &name)?,
            "AllowedAuthAlgos" => c.auth = as_bits(v, &name)?,
            "AllowedGroupCiphers" => c.group = as_bits(v, &name)?,
            "AllowedPairwiseCiphers" => c.pairwise = as_bits(v, &name)?,
            "AllowedGroupMgmtCiphers" => c.group_mgmt = as_bits(v, &name)?,
            "AllowedSuiteBCiphers" => c.suite_b = as_bits(v, &name)?,
            "Shared" => c.shared = as_bool(v, &name)?,
            // A network CURRENT before the reboot is restored ENABLED.
            "Status" => c.status = Some(as_int(v, &name)?).filter(|&s| s != STATUS_CURRENT).unwrap_or(STATUS_ENABLED),
            "FQDN" | "ProviderFriendlyName" if v != Value::Null => return Err(LoadError::Unmodelled("Passpoint networks")),
            "FQDN" | "ProviderFriendlyName" => {}
            // Not parceled.
            "LinkedNetworksList" | "NumRebootsSinceLastUse" | "IsMostRecentlyConnected" => {}
            "DefaultGwMacAddress" => c.default_gw_mac_address = as_string(v, &name)?,
            "ValidatedInternetAccess" => c.validated_internet_access = as_bool(v, &name)?,
            "NoInternetAccessExpected" => c.no_internet_access_expected = as_bool(v, &name)?,
            "MeteredHint" => c.metered_hint = as_bool(v, &name)?,
            "MeteredOverride" => c.metered_override = as_int(v, &name)?,
            "UseExternalScores" => c.use_external_scores = as_bool(v, &name)?,
            "CreatorUid" => c.creator_uid = as_int(v, &name)?,
            "CreatorName" => c.creator_name = as_string(v, &name)?,
            "LastUpdateUid" => c.last_update_uid = as_int(v, &name)?,
            "LastUpdateName" => c.last_update_name = as_string(v, &name)?,
            "LastConnectUid" => c.last_connect_uid = as_int(v, &name)?,
            "IsLegacyPasspointConfig" => {
                if as_bool(v, &name)? {
                    return Err(LoadError::Unmodelled("Passpoint networks"));
                }
            }
            "RoamingConsortiumOIs" => match v {
                Value::Longs(l) if l.is_empty() => {}
                Value::Longs(_) => return Err(LoadError::Unmodelled("Passpoint networks")),
                _ => return Err(skip(&name)),
            },
            "RandomizedMacAddress" => {
                let mac = as_string(v, &name)?.ok_or_else(|| LoadError::Skip("null RandomizedMacAddress".into()))?;
                c.randomized_mac = mac_from_string(&mac).ok_or_else(|| LoadError::Skip(format!("invalid MAC address {mac}")))?;
            }
            "MacRandomizationSetting" => {
                c.mac_randomization_setting = as_int(v, &name)?;
                mac_randomization_exists = true;
            }
            "SendDhcpHostname" => {
                c.send_dhcp_hostname = as_bool(v, &name)?;
                send_dhcp_hostname_exists = true;
            }
            "CarrierId" => c.carrier_id = as_int(v, &name)?,
            "SubscriptionId" => c.subscription_id = as_int(v, &name)?,
            "AutoJoinEnabled" => c.allow_autojoin = as_bool(v, &name)?,
            "Priority" => c.priority = as_int(v, &name)?,
            "DeletionPriority" => {
                let priority = as_int(v, &name)?;
                if priority < 0 {
                    return Err(LoadError::Skip("Deletion priority must be non-negative".into()));
                }
                c.deletion_priority = priority;
            }
            "Trusted" => c.trusted = as_bool(v, &name)?,
            "OemPaid" => c.oem_paid = as_bool(v, &name)?,
            "OemPrivate" => c.oem_private = as_bool(v, &name)?,
            "CarrierMerged" => c.carrier_merged = as_bool(v, &name)?,
            "IsRestricted" => c.restricted = as_bool(v, &name)?,
            "SubscriptionGroup" => return Err(LoadError::Unmodelled("subscription groups")),
            "bssidAllowList" => return Err(LoadError::Unmodelled("BSSID allow lists")),
            "RepeaterEnabled" => c.repeater_enabled = as_bool(v, &name)?,
            "DppPrivateEcKey" => match v {
                // `setDppConfigurator`: an empty key makes it a configurator.
                Value::Bytes(b) if b.is_empty() => c.dpp_configurator = true,
                Value::Bytes(_) => return Err(LoadError::Unmodelled("DPP networks")),
                Value::Null => {}
                _ => return Err(skip(&name)),
            },
            "DppConnector" | "DppCSignKey" | "DppNetAccessKey" => {
                let at = ["DppConnector", "DppCSignKey", "DppNetAccessKey"].iter().position(|n| *n == name).unwrap();
                dpp_keys[at] = match v {
                    Value::Bytes(b) => Some(b),
                    Value::Null => None,
                    _ => return Err(skip(&name)),
                };
            }
            "EnableWifi7" => c.wifi7_enabled = as_bool(v, &name)?,
            _ => {}
        }
    }
    if !mac_randomization_exists {
        c.mac_randomization_setting = RANDOMIZATION_NONE;
    }
    if c.mac_randomization_setting == RANDOMIZATION_PERSISTENT {
        c.mac_randomization_setting = RANDOMIZATION_AUTO;
    }
    c.convert_legacy_fields()?;
    if !send_dhcp_hostname_exists {
        // Legacy configs send the DHCP hostname for secure networks only.
        c.send_dhcp_hostname = !c.is_security_type(SECURITY_TYPE_OPEN) && !c.is_security_type(SECURITY_TYPE_OWE);
    }
    // `setDppConnectionKeys`: all three, or none.
    if let [Some(a), Some(b), Some(d)] = &dpp_keys
        && !(a.is_empty() && b.is_empty() && d.is_empty())
    {
        return Err(LoadError::Unmodelled("DPP networks"));
    }
    Ok(c)
}

/// `MacAddress.fromString`: six hex bytes separated by colons.
fn mac_from_string(s: &str) -> Option<u64> {
    let bytes: Vec<&str> = s.split(':').collect();
    if bytes.len() != 6 {
        return None;
    }
    bytes.iter().try_fold(0u64, |a, b| Some(a << 8 | u64::from(u8::from_str_radix(b, 16).ok()?)))
}

/// `NetworkSelectionStatusXmlUtil.parseFromXml`.
fn parse_selection_status(e: &Element) -> Result<NetworkSelectionStatus, LoadError> {
    let mut s = WifiConfiguration::default().selection;
    // Legacy configs without these tags.
    s.never_detected_captive_portal = false;
    s.ever_validated_internet_access = true;
    let (mut status, mut reason) = (String::new(), String::new());
    for child in e.children() {
        let name = child.string("name").map(|n| n.into_owned()).ok_or_else(|| LoadError::Corrupt("Missing value name".into()))?;
        let v = value(child)?;
        match name.as_str() {
            "SelectionStatus" => status = as_string(v, &name)?.unwrap_or_default(),
            "DisableReason" => reason = as_string(v, &name)?.unwrap_or_default(),
            "ConnectChoice" => s.connect_choice = as_string(v, &name)?,
            "ConnectChoiceRssi" => s.connect_choice_rssi = as_int(v, &name)?,
            "HasEverConnected" => s.has_ever_connected = as_bool(v, &name)?,
            "CaptivePortalNeverDetected" => s.never_detected_captive_portal = as_bool(v, &name)?,
            "HasEverValidatedInternetAccess" => s.ever_validated_internet_access = as_bool(v, &name)?,
            _ => {}
        }
    }
    let status = NETWORK_SELECTION_STATUS.iter().position(|n| *n == status).map(|i| i as i32);
    let reason = DISABLE_REASONS.iter().position(|n| *n == reason).map(|i| i as i32);
    // Temporary disables are not kept across reboots.
    let (status, reason) = match (status, reason) {
        (Some(s), Some(r)) if s != NETWORK_SELECTION_TEMPORARY_DISABLED => (s, r),
        _ => (NETWORK_SELECTION_ENABLED, DISABLED_NONE),
    };
    s.status = status;
    s.disable_reason = reason;
    if status == NETWORK_SELECTION_PERMANENTLY_DISABLED {
        s.counters[reason as usize] = 1;
    }
    Ok(s)
}

/// `IpConfigurationXmlUtil.parseFromXml`: DHCP (or unassigned) without a
/// proxy.
fn parse_ip_configuration(e: &Element) -> Result<(), LoadError> {
    let field = |name: &str| -> Result<String, LoadError> {
        let v = e.children().find(|c| c.string("name").as_deref() == Some(name)).ok_or_else(|| LoadError::Corrupt(format!("Missing {name}")))?;
        as_string(value(v)?, name)?.ok_or_else(|| LoadError::Skip(format!("null {name}")))
    };
    match field("IpAssignment")?.as_str() {
        "DHCP" => {}
        "STATIC" | "UNASSIGNED" => return Err(LoadError::Unmodelled("static or unassigned IP configurations")),
        other => return Err(LoadError::Skip(format!("No enum constant IpAssignment.{other}"))),
    }
    match field("ProxySettings")?.as_str() {
        "NONE" => Ok(()),
        "STATIC" | "PAC" | "UNASSIGNED" => Err(LoadError::Unmodelled("proxy settings")),
        other => Err(LoadError::Skip(format!("No enum constant ProxySettings.{other}"))),
    }
}

/// `NetworkListStoreData.parseNetwork`: the network, `creator` naming its
/// creator's uid as `getNameForUid` does.
fn parse_network(e: &Element, creator: &dyn Fn(i32) -> Result<Option<String>, LoadError>) -> Result<WifiConfiguration, LoadError> {
    let mut config = None;
    let mut status = None;
    let mut ip = false;
    for section in e.children() {
        match section.name.as_str() {
            "WifiConfiguration" if config.is_some() => return Err(LoadError::Corrupt("Detected duplicate tag for: WifiConfiguration".into())),
            "WifiConfiguration" => config = Some(parse_configuration(section)?),
            "NetworkStatus" if status.is_some() => return Err(LoadError::Corrupt("Detected duplicate tag for: NetworkStatus".into())),
            "NetworkStatus" => status = Some(parse_selection_status(section)?),
            "IpConfiguration" if ip => return Err(LoadError::Corrupt("Detected duplicate tag for: IpConfiguration".into())),
            "IpConfiguration" => {
                parse_ip_configuration(section)?;
                ip = true;
            }
            "WifiEnterpriseConfiguration" => return Err(LoadError::Unmodelled("enterprise networks")),
            _ => {}
        }
    }
    let mut c = config.ok_or_else(|| LoadError::Corrupt("XML parsing of wifi configuration failed".into()))?;
    c.convert_legacy_fields()?;
    if c.pre_shared_key.is_some() && !c.needs_pre_shared_key() {
        c.set_security(SecurityParams::of(SECURITY_TYPE_PSK)?);
    }
    // A creator uid with no name is the system's.
    match creator(c.creator_uid)? {
        None => {
            c.creator_uid = 1000;
            c.creator_name = creator(1000)?;
        }
        Some(name) => c.creator_name = Some(name),
    }
    // A missing status fails validation; an IP configuration defaults
    // to unassigned, which is not modelled.
    let Some(status) = status else { return Err(LoadError::Skip("no NetworkStatus".into())) };
    c.selection = status;
    if !ip {
        return Err(LoadError::Unmodelled("static or unassigned IP configurations"));
    }
    // `addUpgradableSecurityTypeIfNecessary`: logged when it fails.
    for (from, to) in [(SECURITY_TYPE_OPEN, SECURITY_TYPE_OWE), (SECURITY_TYPE_PSK, SECURITY_TYPE_SAE)] {
        if c.is_security_type(from) && !c.is_security_type(to) {
            let mut params = SecurityParams::of(to)?;
            params.added_by_auto_upgrade = true;
            if let Err(e) = c.add_security(params) {
                eprintln!("app host: wifi: Failed to add upgradable security type: {e}");
                break;
            }
        }
    }
    Ok(c)
}

/// `WifiSsid.fromString(ssid).getBytes().length` is not zero, or the
/// string does not convert.
fn ssid_bytes_nonempty(ssid: &str) -> bool {
    if let Some(inner) = ssid.strip_prefix('"') {
        return inner.strip_suffix('"').is_some_and(|s| !s.is_empty());
    }
    ssid.len() % 2 == 0 && !ssid.is_empty() && ssid.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `validateBssid(String)`: a unicast MAC address, or "any".
fn valid_bssid(bssid: Option<&str>) -> bool {
    match bssid {
        None => true,
        Some("any") => true,
        Some(s) => mac_from_string(s).is_some_and(|m| m >> 40 & 1 == 0),
    }
}

/// `validatePassword(.., isAdd: true, ..)`.
fn valid_password(password: Option<&str>, sae: bool, wapi: bool) -> bool {
    let Some(p) = password else { return false };
    if p.is_empty() {
        return false;
    }
    if let Some(inner) = p.strip_prefix('"') {
        let min = if sae { SAE_ASCII_MIN_LEN } else { PSK_ASCII_MIN_LEN };
        return p.is_ascii() && (min..=PSK_SAE_ASCII_MAX_LEN).contains(&p.len()) && inner.ends_with('"');
    }
    let length_ok = if wapi { p.len() <= 100 } else { p.len() == PSK_SAE_HEX_LEN };
    length_ok && p.len() % 2 == 0 && p.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `WifiConfigurationUtil.validate(config, .., VALIDATE_FOR_ADD)` of what
/// is modelled.
fn validate(c: &mut WifiConfiguration) -> bool {
    use key_mgmt::*;
    let Some(ssid) = c.ssid.as_deref() else { return false };
    let max = if ssid.starts_with('"') { SSID_PLAINTEXT_MAX_LEN } else { SSID_HEX_MAX_LEN };
    if ssid.is_empty() || ssid.chars().count() > max || !ssid_bytes_nonempty(ssid) {
        return false;
    }
    if !valid_bssid(c.bssid.as_deref()) || !valid_bssid(c.default_gw_mac_address.as_deref()) {
        return false;
    }
    if !(c.key_mgmt.within(STRINGS.len() as u32)
        && c.protocols.within(PROTOCOL_COUNT)
        && c.auth.within(AUTH_ALGORITHM_COUNT)
        && c.group.within(GROUP_CIPHER_COUNT)
        && c.pairwise.within(PAIRWISE_CIPHER_COUNT))
    {
        return false;
    }
    let k = c.key_mgmt;
    if k.cardinality() == 0 || (k.cardinality() > 1 && !(k.cardinality() <= 3 && k.get(WPA_EAP) && k.get(IEEE8021X))) {
        return false;
    }
    let mut kinds = Vec::new();
    for p in &c.security {
        if !(0..=SECURITY_TYPE_NUM).contains(&p.kind) || kinds.contains(&p.kind) {
            return false;
        }
        kinds.push(p.kind);
    }
    if c.selection.connect_choice.as_deref().is_some_and(|s| s.encode_utf16().count() > MAX_STRING_LENGTH) {
        return false;
    }
    if c.is_security_type(SECURITY_TYPE_PSK) && !valid_password(c.pre_shared_key.as_deref(), false, false)
        || c.is_security_type(SECURITY_TYPE_SAE) && !valid_password(c.pre_shared_key.as_deref(), true, false)
        || c.is_security_type(SECURITY_TYPE_WAPI_PSK) && !valid_password(c.pre_shared_key.as_deref(), false, true)
    {
        return false;
    }
    if c.pre_shared_key.is_some() && !c.needs_pre_shared_key() {
        if let Ok(params) = SecurityParams::of(SECURITY_TYPE_PSK) {
            c.set_security(params);
        }
    }
    true
}

/// The networks of a store's `<NetworkList>`: `None` without one.
pub fn read_network_list(
    root: &Element,
    creator: &dyn Fn(i32) -> Result<Option<String>, LoadError>,
) -> Result<Vec<WifiConfiguration>, LoadError> {
    let Some(list) = root.children().find(|e| e.name == "NetworkList") else { return Ok(Vec::new()) };
    let mut networks = Vec::new();
    for network in list.children().filter(|e| e.name == "Network") {
        match parse_network(network, creator) {
            Ok(c) => networks.push(c),
            Err(LoadError::Skip(e)) => eprintln!("app host: wifi: Failed to parse network config. Skipping...: {e}"),
            Err(e) => return Err(e),
        }
    }
    Ok(networks)
}

/// `loadInternalData`: the shared then the user networks, each valid one
/// numbered in turn; one with the profile key of another merges into it,
/// which is not modelled.
pub fn load(shared: Vec<WifiConfiguration>, user: Vec<WifiConfiguration>) -> Result<Vec<WifiConfiguration>, LoadError> {
    let mut networks: Vec<WifiConfiguration> = Vec::new();
    for mut c in shared.into_iter().chain(user) {
        if !validate(&mut c) {
            eprintln!("app host: wifi: Skipping malformed network from store: {:?}", c.ssid);
            continue;
        }
        if networks.iter().any(|n| n.profile_key() == c.profile_key()) {
            return Err(LoadError::Unmodelled("merging a stored network into another"));
        }
        // `generateRandomizedMacAddresses`.
        if c.randomized_mac == DEFAULT_MAC_ADDRESS {
            return Err(LoadError::Unmodelled("a network without a persisted randomized MAC address"));
        }
        c.network_id = networks.len() as i32;
        networks.push(c);
    }
    Ok(networks)
}

/// `convertMultiTypeConfigsToLegacyConfigs(configs, ignoreDisabledType)`,
/// `omit` telling the auto-upgrade params `shouldOmitAutoUpgradeParams`
/// omits.
pub fn legacy_configs(configs: &[WifiConfiguration], ignore_disabled: bool, omit: &dyn Fn(&SecurityParams) -> bool) -> Vec<WifiConfiguration> {
    let mut out = Vec::new();
    for c in configs {
        for params in &c.security {
            if ignore_disabled && !params.enabled || omit(params) {
                continue;
            }
            let mut legacy = c.clone();
            legacy.set_security(params.clone());
            if !params.enabled {
                legacy.selection.status = NETWORK_SELECTION_PERMANENTLY_DISABLED;
                legacy.selection.disable_reason = DISABLED_TRANSITION_DISABLE_INDICATION;
                legacy.selection.counters[DISABLED_TRANSITION_DISABLE_INDICATION as usize] = 1;
            }
            out.push(legacy);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared store's network as the image's first boot saved it.
    const STORE: &str = r#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<WifiConfigStoreData>
<int name="Version" value="3" />
<NetworkList>
<Network>
<WifiConfiguration>
<string name="ConfigKey">&quot;AndroidWifi&quot;NONE</string>
<string name="SSID">&quot;AndroidWifi&quot;</string>
<null name="PreSharedKey" />
<null name="WEPKeys" />
<int name="WEPTxKeyIndex" value="0" />
<boolean name="HiddenSSID" value="false" />
<boolean name="RequirePMF" value="false" />
<byte-array name="AllowedKeyMgmt" num="1">01</byte-array>
<byte-array name="AllowedProtocols" num="1">03</byte-array>
<byte-array name="AllowedAuthAlgos" num="0"></byte-array>
<byte-array name="AllowedGroupCiphers" num="0"></byte-array>
<byte-array name="AllowedPairwiseCiphers" num="0"></byte-array>
<byte-array name="AllowedGroupMgmtCiphers" num="0"></byte-array>
<byte-array name="AllowedSuiteBCiphers" num="0"></byte-array>
<boolean name="Shared" value="true" />
<boolean name="AutoJoinEnabled" value="true" />
<int name="Priority" value="0" />
<int name="DeletionPriority" value="0" />
<int name="NumRebootsSinceLastUse" value="3" />
<boolean name="RepeaterEnabled" value="false" />
<boolean name="EnableWifi7" value="true" />
<SecurityParamsList>
<SecurityParams>
<int name="SecurityType" value="0" />
<boolean name="IsEnabled" value="true" />
<boolean name="SaeIsH2eOnlyMode" value="false" />
<boolean name="SaeIsPkOnlyMode" value="false" />
<boolean name="IsAddedByAutoUpgrade" value="false" />
<byte-array name="AllowedSuiteBCiphers" num="0"></byte-array>
</SecurityParams>
<SecurityParams>
<int name="SecurityType" value="6" />
<boolean name="IsEnabled" value="true" />
<boolean name="SaeIsH2eOnlyMode" value="false" />
<boolean name="SaeIsPkOnlyMode" value="false" />
<boolean name="IsAddedByAutoUpgrade" value="true" />
<byte-array name="AllowedSuiteBCiphers" num="0"></byte-array>
</SecurityParams>
</SecurityParamsList>
<boolean name="SendDhcpHostname" value="true" />
<boolean name="Trusted" value="true" />
<boolean name="IsRestricted" value="false" />
<boolean name="OemPaid" value="false" />
<boolean name="OemPrivate" value="false" />
<boolean name="CarrierMerged" value="false" />
<null name="BSSID" />
<int name="Status" value="2" />
<null name="FQDN" />
<null name="ProviderFriendlyName" />
<null name="LinkedNetworksList" />
<null name="DefaultGwMacAddress" />
<boolean name="ValidatedInternetAccess" value="false" />
<boolean name="NoInternetAccessExpected" value="false" />
<boolean name="MeteredHint" value="false" />
<int name="MeteredOverride" value="0" />
<boolean name="UseExternalScores" value="false" />
<int name="CreatorUid" value="10181" />
<string name="CreatorName">com.google.android.googlesdksetup</string>
<int name="LastUpdateUid" value="10181" />
<string name="LastUpdateName">com.google.android.googlesdksetup</string>
<int name="LastConnectUid" value="10181" />
<boolean name="IsLegacyPasspointConfig" value="false" />
<long-array name="RoamingConsortiumOIs" num="0" />
<string name="RandomizedMacAddress">6e:53:d6:a0:fc:69</string>
<int name="MacRandomizationSetting" value="3" />
<int name="CarrierId" value="-1" />
<boolean name="IsMostRecentlyConnected" value="false" />
<int name="SubscriptionId" value="-1" />
<byte-array name="DppPrivateEcKey" num="0"></byte-array>
<byte-array name="DppConnector" num="0"></byte-array>
<byte-array name="DppCSignKey" num="0"></byte-array>
<byte-array name="DppNetAccessKey" num="0"></byte-array>
</WifiConfiguration>
<NetworkStatus>
<string name="SelectionStatus">NETWORK_SELECTION_ENABLED</string>
<string name="DisableReason">NETWORK_SELECTION_ENABLE</string>
<null name="ConnectChoice" />
<int name="ConnectChoiceRssi" value="0" />
<boolean name="HasEverConnected" value="false" />
<boolean name="CaptivePortalNeverDetected" value="true" />
<boolean name="HasEverValidatedInternetAccess" value="false" />
</NetworkStatus>
<IpConfiguration>
<string name="IpAssignment">DHCP</string>
<string name="ProxySettings">NONE</string>
</IpConfiguration>
</Network>
</NetworkList>
</WifiConfigStoreData>"#;

    fn creator(uid: i32) -> Result<Option<String>, LoadError> {
        Ok(match uid {
            10181 => Some("com.google.android.googlesdksetup".into()),
            1000 => Some("android.uid.system:1000".into()),
            _ => None,
        })
    }

    fn loaded() -> Vec<WifiConfiguration> {
        let root = aim_android_xml::read(STORE.as_bytes()).unwrap();
        load(read_network_list(&root, &creator).unwrap(), Vec::new()).unwrap()
    }

    /// `parseNetwork` and `loadInternalData` of the image's open network:
    /// its open and auto-upgrade OWE params, its persisted fields, an empty
    /// DPP key making it a configurator, network id 0.
    #[test]
    fn the_store_network_loads_as_the_original_loads_it() {
        let networks = loaded();
        assert_eq!(networks.len(), 1);
        let c = &networks[0];
        assert_eq!((c.network_id, c.status), (0, STATUS_ENABLED));
        assert_eq!(c.ssid.as_deref(), Some("\"AndroidWifi\""));
        assert_eq!(c.security.iter().map(|p| (p.kind, p.added_by_auto_upgrade)).collect::<Vec<_>>(), [(0, false), (6, true)]);
        assert_eq!((c.key_mgmt, c.protocols), (Bits::of(&[key_mgmt::NONE]), Bits::of(&[PROTOCOL_WPA, PROTOCOL_RSN])));
        assert_eq!(c.randomized_mac, 0x6e53_d6a0_fc69);
        assert_eq!(c.mac_randomization_setting, RANDOMIZATION_AUTO);
        assert!(c.dpp_configurator);
        assert_eq!(c.creator_name.as_deref(), Some("com.google.android.googlesdksetup"));
        assert_eq!(c.profile_key(), "\"AndroidWifi\"NONE");
        assert!(c.selection.never_detected_captive_portal && !c.selection.ever_validated_internet_access);
    }

    /// `convertMultiTypeConfigsToLegacyConfigs`: one config per params,
    /// the OWE one with OWE's sets, an omitted auto-upgrade left out.
    #[test]
    fn multi_type_configs_split_into_legacy_ones() {
        let networks = loaded();
        let legacy = legacy_configs(&networks, false, &|_| false);
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy[1].security.len(), 1);
        assert_eq!(legacy[1].key_mgmt, Bits::of(&[key_mgmt::OWE]));
        assert!(legacy[1].require_pmf);
        let without = legacy_configs(&networks, false, &|p| p.added_by_auto_upgrade);
        assert_eq!(without.len(), 1);
    }

    /// `WifiConfiguration.CREATOR` reads the external copy back field by
    /// field, to the end.
    #[test]
    fn configurations_parcel_as_wifi_configuration_writes_them() {
        let c = loaded()[0].external(-1, false, 42);
        let mut p = Parcel::new();
        c.write_to(&mut p);
        let mut r = p.reader();
        let int = |r: &mut aim_binder_host::parcel::Reader<'_>| r.read_i32().unwrap();
        let s = |r: &mut aim_binder_host::parcel::Reader<'_>| r.read_string16().unwrap();
        assert_eq!((int(&mut r), int(&mut r)), (0, STATUS_ENABLED));
        // NetworkSelectionStatus.
        assert_eq!((int(&mut r), int(&mut r)), (0, 0));
        for _ in 0..NETWORK_SELECTION_DISABLED_MAX {
            assert_eq!(int(&mut r), 0);
        }
        assert_eq!((r.read_i64().unwrap(), r.read_i64().unwrap()), (-1, -1));
        assert_eq!(s(&mut r), None);
        assert_eq!(int(&mut r), CONNECT_CHOICE_NOT_EXISTS);
        assert_eq!((int(&mut r), int(&mut r), int(&mut r)), (0, 1, 0));
        assert_eq!((s(&mut r), s(&mut r)), (None, None));
        assert_eq!(s(&mut r).as_deref(), Some("\"AndroidWifi\""));
        assert_eq!(s(&mut r), None); // BSSID
        assert_eq!((int(&mut r), int(&mut r)), (0, 0)); // apBand, apChannel
        assert_eq!((s(&mut r), s(&mut r), int(&mut r), int(&mut r)), (None, None, 0, 0));
        assert_eq!(s(&mut r), None); // psk
        for _ in 0..4 {
            assert_eq!(s(&mut r), None);
        }
        assert_eq!((int(&mut r), int(&mut r), int(&mut r), int(&mut r), int(&mut r)), (0, 0, 0, 0, 0));
        assert_eq!(s(&mut r), None); // updateIdentifier
        // The seven bit sets: NONE; WPA and RSN; then empty.
        assert_eq!((int(&mut r), int(&mut r)), (1, 0));
        assert_eq!((int(&mut r), int(&mut r), int(&mut r)), (2, 0, 1));
        for _ in 0..5 {
            assert_eq!(int(&mut r), 0);
        }
        assert_eq!(int(&mut r), 2); // security params
        for kind in [0, 6] {
            assert_eq!(s(&mut r).as_deref(), Some("android.net.wifi.SecurityParams"));
            assert_eq!((int(&mut r), int(&mut r)), (kind, 1));
            for _ in 0..7 {
                let n = int(&mut r);
                for _ in 0..n {
                    int(&mut r);
                }
            }
            assert_eq!((int(&mut r), int(&mut r), int(&mut r), int(&mut r)), ((kind == 6) as i32, (kind == 6) as i32, 0, 0));
        }
        assert_eq!(s(&mut r).as_deref(), Some("android.net.wifi.WifiEnterpriseConfig"));
        assert_eq!((int(&mut r), int(&mut r), int(&mut r), int(&mut r)), (0, -1, 0, 0));
        assert_eq!(s(&mut r), None);
        assert_eq!(int(&mut r), 0);
        assert_eq!(s(&mut r), None);
        for _ in 0..8 {
            assert_eq!(int(&mut r), 0);
        }
        assert_eq!(s(&mut r).as_deref(), Some("android.net.IpConfiguration"));
        assert_eq!((s(&mut r).as_deref(), s(&mut r).as_deref(), s(&mut r), s(&mut r)), (Some("DHCP"), Some("NONE"), None, None));
        assert_eq!((s(&mut r), s(&mut r)), (None, None)); // dhcpServer, default gateway
        // validatedInternetAccess .. useExternalScores.
        assert_eq!((0..13).map(|_| int(&mut r)).collect::<Vec<_>>(), [0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!((int(&mut r), int(&mut r), int(&mut r)), (10181, 10181, 10181));
        assert_eq!(s(&mut r).as_deref(), Some("com.google.android.googlesdksetup"));
        assert_eq!(s(&mut r).as_deref(), Some("com.google.android.googlesdksetup"));
        assert_eq!((0..7).map(|_| int(&mut r)).collect::<Vec<_>>(), [0, 0, 0, 1, 0, 0, 1]);
        assert_eq!(s(&mut r), None); // Passpoint management object tree
        assert_eq!((int(&mut r), r.read_i64().unwrap()), (0, 0));
        assert_eq!(s(&mut r).as_deref(), Some("android.net.MacAddress"));
        assert_eq!(r.read_i64().unwrap(), DEFAULT_MAC_ADDRESS as i64); // masked
        assert_eq!((int(&mut r), int(&mut r), int(&mut r)), (RANDOMIZATION_NONE, 1, 0));
        assert_eq!((r.read_i64().unwrap(), r.read_i64().unwrap()), (0, 42));
        assert_eq!(int(&mut r), -1); // carrierId
        assert_eq!(s(&mut r), None);
        assert_eq!((int(&mut r), int(&mut r)), (-1, 0));
        assert_eq!(s(&mut r), None); // subscription group
        assert_eq!((int(&mut r), int(&mut r)), (-1, 1)); // no allow list; a DPP configurator
        assert_eq!((0..11).map(|_| int(&mut r)).collect::<Vec<_>>(), [0; 11]);
        assert_eq!(int(&mut r), 1); // Wi-Fi 7
        assert_eq!(r.remaining(), 0);
    }

    /// The MAC address stays for the Wi-Fi stack, the system and the
    /// creator; a password is masked.
    #[test]
    fn external_copies_mask_as_the_manager_masks() {
        let mut c = loaded().remove(0);
        c.pre_shared_key = Some("\"password\"".into());
        assert_eq!(c.external(10181, true, 1).randomized_mac, 0x6e53_d6a0_fc69);
        assert_eq!(c.external(1010, true, 1).randomized_mac, 0x6e53_d6a0_fc69);
        assert_eq!(c.external(10100, true, 1).randomized_mac, DEFAULT_MAC_ADDRESS);
        assert_eq!(c.external(10100, true, 1).pre_shared_key.as_deref(), Some(PASSWORD_MASK));
        assert_eq!(c.external(10100, true, 1).mac_randomization_setting, RANDOMIZATION_AUTO);
    }
}
