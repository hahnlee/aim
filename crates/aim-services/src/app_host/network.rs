//! The connectivity module's parcelables (`platform/packages/modules/
//! Connectivity`, `framework/src/android/net` at `android-16.0.0_r1`) as
//! their `writeToParcel` writes them and their `CREATOR`s read them:
//! `Network`, `NetworkCapabilities` with the request matching
//! ConnectivityService runs on it (`satisfiedByNetworkCapabilities`),
//! `NetworkRequest`, `NetworkInfo`, `LinkProperties` with its IPv4
//! `LinkAddress`es and `RouteInfo`s, and the `Messenger` and `Message` a
//! network callback travels in (#1509).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::net::Ipv4Addr;

use aim_binder_host::parcel::{
    BAD_TYPE, BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use crate::location::parcels::{Value, write_bundle};

/// `NetworkCapabilities.NET_CAPABILITY_*`.
pub const NOT_METERED: u32 = 11;
pub const INTERNET: u32 = 12;
pub const NOT_RESTRICTED: u32 = 13;
pub const TRUSTED: u32 = 14;
pub const NOT_VPN: u32 = 15;
pub const VALIDATED: u32 = 16;
const CAPTIVE_PORTAL: u32 = 17;
pub const NOT_ROAMING: u32 = 18;
pub const FOREGROUND: u32 = 19;
pub const NOT_CONGESTED: u32 = 20;
pub const NOT_SUSPENDED: u32 = 21;
const PARTIAL_CONNECTIVITY: u32 = 24;
pub const TEMPORARILY_NOT_METERED: u32 = 25;
pub const NOT_VCN_MANAGED: u32 = 28;
const HEAD_UNIT: u32 = 32;
pub const LOCAL_NETWORK: u32 = 36;
pub const NOT_BANDWIDTH_CONSTRAINED: u32 = 37;
const ALL_VALID_CAPABILITIES: u64 = (2 << NOT_BANDWIDTH_CONSTRAINED) - 1;
/// `NON_REQUESTABLE_CAPABILITIES` (`MUTABLE_CAPABILITIES` but TRUSTED and
/// NOT_VCN_MANAGED), by name (`capabilityNameOf`), lowest first.
const NON_REQUESTABLE: &[(u32, &str)] = &[
    (VALIDATED, "VALIDATED"),
    (CAPTIVE_PORTAL, "CAPTIVE_PORTAL"),
    (NOT_ROAMING, "NOT_ROAMING"),
    (FOREGROUND, "FOREGROUND"),
    (NOT_CONGESTED, "NOT_CONGESTED"),
    (NOT_SUSPENDED, "NOT_SUSPENDED"),
    (PARTIAL_CONNECTIVITY, "PARTIAL_CONNECTIVITY"),
    (TEMPORARILY_NOT_METERED, "TEMPORARILY_NOT_METERED"),
    (HEAD_UNIT, "HEAD_UNIT"),
];
/// `DEFAULT_CAPABILITIES`, the constructor's (V and later).
const DEFAULT_CAPABILITIES: u64 =
    bits(&[NOT_RESTRICTED, TRUSTED, NOT_VPN, NOT_BANDWIDTH_CONSTRAINED]);
/// `NetworkCapabilities.TRANSPORT_*`: MIN_TRANSPORT to MAX_TRANSPORT.
pub const TRANSPORT_CELLULAR: u32 = 0;
pub const TRANSPORT_WIFI: u32 = 1;
pub const TRANSPORT_ETHERNET: u32 = 3;
const TRANSPORT_VPN: u32 = 4;
const TRANSPORT_THREAD: u32 = 9;
const ALL_VALID_TRANSPORTS: u64 = (1 << 11) - 1;
/// `NetworkCapabilitiesUtils`' capabilities of restricted networks, of
/// those forced restricted, and of unrestricted ones.
const RESTRICTED_CAPABILITIES: u64 = (1 << 31) // BIP
    | (1 << 5) // CBS
    | (1 << 2) // DUN
    | (1 << 10) // EIMS
    | (1 << 29) // ENTERPRISE
    | (1 << 3) // FOTA
    | (1 << 7) // IA
    | (1 << 4) // IMS
    | (1 << 23) // MCX
    | (1 << 8) // RCS
    | (1 << 27) // VEHICLE_INTERNAL
    | (1 << 30) // VSIM
    | (1 << 9) // XCAP
    | (1 << 33); // MMTEL
const FORCE_RESTRICTED_CAPABILITIES: u64 = (1 << 29) | (1 << 22) | (1 << 26);
const UNRESTRICTED_CAPABILITIES: u64 = (1 << 12) | 1 | (1 << 1) | (1 << 6);
/// `NET_ENTERPRISE_ID_1` to `_5`.
const ALL_VALID_ENTERPRISE_IDS: i32 = 0b11_1110;
const NET_ENTERPRISE_ID_1: i32 = 1;
/// `RES_ID_MATCH_ALL_RESERVATIONS`; `SIGNAL_STRENGTH_UNSPECIFIED`.
const RES_ID_MATCH_ALL_RESERVATIONS: i32 = -1;
const SIGNAL_STRENGTH_UNSPECIFIED: i32 = i32::MIN;
/// `Process.INVALID_UID`, `SYSTEM_UID`.
pub const INVALID_UID: i32 = -1;
const SYSTEM_UID: i32 = 1000;
/// `ConnectivityManager.TYPE_NONE`, `TYPE_ETHERNET`.
pub const TYPE_NONE: i32 = -1;
pub const TYPE_ETHERNET: i32 = 9;
/// `LinkAddress.LIFETIME_PERMANENT`; `RouteInfo.RTN_UNICAST`.
pub const LIFETIME_PERMANENT: i64 = i64::MAX;
pub const RTN_UNICAST: i32 = 1;

const fn bits(capabilities: &[u32]) -> u64 {
    let mut b = 0;
    let mut i = 0;
    while i < capabilities.len() {
        b |= 1 << capabilities[i];
        i += 1;
    }
    b
}

/// `Parcel.readParcelable`'s class name; a class it does not read is
/// `BAD_TYPE`, which the service answers as not implemented.
fn parcelable_class(r: &mut Reader<'_>) -> ParcelResult<Option<String>> {
    r.read_string16()
}

/// `writeParcelable`: the class name, then the fields; null is a null
/// class name.
fn write_parcelable(p: &mut Parcel, class: &str, value: Option<&dyn WriteParcelable>) {
    match value {
        Some(v) => {
            p.write_string16(Some(class));
            v.write_to(p);
        }
        None => p.write_string16(None),
    }
}

/// `Network`: its netId.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network(pub i32);

impl ReadParcelable for Network {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Network(r.read_i32()?))
    }
}

impl WriteParcelable for Network {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0);
    }
}

/// The network specifiers it reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Specifier {
    /// `EthernetNetworkSpecifier`: an interface; it is satisfied by an
    /// equal one (`canBeSatisfiedBy`), and is not redacted.
    Ethernet(String),
    /// `MatchAllNetworkSpecifier`, which no request may hold.
    MatchAll,
}

const ETHERNET_SPECIFIER: &str = "android.net.EthernetNetworkSpecifier";
const MATCH_ALL_SPECIFIER: &str = "android.net.MatchAllNetworkSpecifier";

impl WriteParcelable for Specifier {
    fn write_to(&self, p: &mut Parcel) {
        if let Specifier::Ethernet(name) = self {
            p.write_string16(Some(name));
        }
    }
}

impl Specifier {
    fn class(&self) -> &'static str {
        match self {
            Specifier::Ethernet(_) => ETHERNET_SPECIFIER,
            Specifier::MatchAll => MATCH_ALL_SPECIFIER,
        }
    }
}

/// `NetworkCapabilities`. It has no transport info: the one network
/// has none, and a request with one is not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkCapabilities {
    pub capabilities: u64,
    pub forbidden: u64,
    pub transports: u64,
    pub link_up_kbps: i32,
    pub link_down_kbps: i32,
    pub specifier: Option<Specifier>,
    pub signal_strength: i32,
    /// `UidRange`s (start, stop); none applies to every uid.
    pub uids: Option<Vec<(i32, i32)>>,
    pub allowed_uids: Vec<i32>,
    pub ssid: Option<String>,
    pub private_dns_broken: bool,
    pub administrator_uids: Vec<i32>,
    pub owner_uid: i32,
    pub requestor_uid: i32,
    pub requestor_package: Option<String>,
    pub sub_ids: Vec<i32>,
    pub underlying: Option<Vec<Option<Network>>>,
    pub enterprise_id: i32,
    pub reservation_id: i32,
    pub match_non_thread_local: bool,
}

impl Default for NetworkCapabilities {
    /// `new NetworkCapabilities()`.
    fn default() -> Self {
        NetworkCapabilities {
            capabilities: DEFAULT_CAPABILITIES,
            ..NetworkCapabilities::cleared()
        }
    }
}

impl NetworkCapabilities {
    /// `clearAll`: nothing at all, not even the default capabilities.
    pub fn cleared() -> Self {
        NetworkCapabilities {
            capabilities: 0,
            forbidden: 0,
            transports: 0,
            link_up_kbps: 0,
            link_down_kbps: 0,
            specifier: None,
            signal_strength: SIGNAL_STRENGTH_UNSPECIFIED,
            uids: None,
            allowed_uids: Vec::new(),
            ssid: None,
            private_dns_broken: false,
            administrator_uids: Vec::new(),
            owner_uid: INVALID_UID,
            requestor_uid: INVALID_UID,
            requestor_package: None,
            sub_ids: Vec::new(),
            underlying: None,
            enterprise_id: 0,
            reservation_id: 0,
            match_non_thread_local: false,
        }
    }

    /// `maybeMarkCapabilitiesRestricted`
    /// (`NetworkCapabilitiesUtils.inferRestrictedCapability`).
    pub fn mark_restricted(&mut self) {
        let c = self.capabilities;
        if c & FORCE_RESTRICTED_CAPABILITIES != 0
            || (c & UNRESTRICTED_CAPABILITIES == 0 && c & RESTRICTED_CAPABILITIES != 0)
        {
            self.remove(NOT_RESTRICTED);
        }
    }

    pub fn has(&self, capability: u32) -> bool {
        self.capabilities & (1 << capability) != 0
    }

    /// `addCapability`: also no longer forbidden.
    pub fn add(&mut self, capability: u32) {
        self.capabilities |= 1 << capability;
        self.forbidden &= !(1 << capability);
    }

    pub fn remove(&mut self, capability: u32) {
        self.capabilities &= !(1 << capability);
    }

    /// `addForbiddenCapability`: also no longer requested.
    pub fn forbid(&mut self, capability: u32) {
        self.forbidden |= 1 << capability;
        self.capabilities &= !(1 << capability);
    }

    pub fn has_transport(&self, transport: u32) -> bool {
        self.transports & (1 << transport) != 0
    }

    pub fn is_metered(&self) -> bool {
        !self.has(NOT_METERED)
    }

    pub fn has_signal_strength(&self) -> bool {
        self.signal_strength > SIGNAL_STRENGTH_UNSPECIFIED
    }

    /// `setSingleUid`.
    pub fn set_single_uid(&mut self, uid: i32) {
        self.uids = Some(vec![(uid, uid)]);
    }

    /// `describeFirstNonRequestableCapability`.
    pub fn first_non_requestable(&self) -> Option<&'static str> {
        let held = self.capabilities | self.forbidden;
        if let Some((_, name)) = NON_REQUESTABLE.iter().find(|(c, _)| held & (1 << c) != 0) {
            return Some(name);
        }
        if self.link_up_kbps != 0 || self.link_down_kbps != 0 {
            return Some("link bandwidth");
        }
        if self.has_signal_strength() {
            return Some("signalStrength");
        }
        self.private_dns_broken.then_some("privateDnsBroken")
    }

    /// `appliesToUid`.
    pub fn applies_to_uid(&self, uid: i32) -> bool {
        self.uids.as_ref().is_none_or(|ranges| {
            ranges
                .iter()
                .any(|&(start, stop)| start <= uid && uid <= stop)
        })
    }

    /// `satisfiedByNetworkCapabilities`: whether a network of `nc`
    /// satisfies a request of these.
    pub fn satisfied_by(&self, nc: &NetworkCapabilities) -> bool {
        let capabilities = nc.capabilities & self.capabilities == self.capabilities
            && self.forbidden & nc.capabilities == 0;
        let transports = self.transports == 0 || self.transports & nc.transports != 0;
        let bandwidths =
            self.link_up_kbps <= nc.link_up_kbps && self.link_down_kbps <= nc.link_down_kbps;
        let specifier = match &self.specifier {
            None => true,
            Some(s) => {
                nc.specifier.as_ref() == Some(s) || nc.specifier == Some(Specifier::MatchAll)
            }
        };
        let enterprise = nc.enterprise_id & self.enterprise_id == self.enterprise_id
            || (nc.enterprise_id == 0 && self.enterprise_id == 1 << NET_ENTERPRISE_ID_1);
        let uids = match (&self.uids, &nc.uids) {
            (Some(required), Some(_)) => required.iter().all(|&(start, stop)| {
                (start <= nc.owner_uid && nc.owner_uid <= stop) || nc.applies_to_range(start, stop)
            }),
            _ => true,
        };
        let ssid = self.ssid.is_none() || self.ssid == nc.ssid;
        let requestor = self.requestor_uid == INVALID_UID
            || nc.requestor_uid == INVALID_UID
            || (self.requestor_uid == nc.requestor_uid
                && (nc.requestor_package.is_none()
                    || self.requestor_package.is_none()
                    || self.requestor_package == nc.requestor_package));
        let sub_ids =
            self.sub_ids.is_empty() || nc.sub_ids.iter().any(|s| self.sub_ids.contains(s));
        let reservation = if self.reservation_id == 0 {
            nc.reservation_id != RES_ID_MATCH_ALL_RESERVATIONS
        } else {
            nc.reservation_id == RES_ID_MATCH_ALL_RESERVATIONS
                || self.reservation_id == nc.reservation_id
        };
        let local = !nc.has(LOCAL_NETWORK)
            || self.match_non_thread_local
            || nc.has_transport(TRANSPORT_THREAD);
        capabilities
            && transports
            && bandwidths
            && specifier
            && enterprise
            && self.signal_strength <= nc.signal_strength
            && uids
            && ssid
            && requestor
            && sub_ids
            && reservation
            && local
    }

    /// `appliesToUidRange`.
    fn applies_to_range(&self, start: i32, stop: i32) -> bool {
        self.uids
            .as_ref()
            .is_none_or(|ranges| ranges.iter().any(|&(s, e)| s <= start && stop <= e))
    }

    /// What `uid`, holding `held`, sees of a network's capabilities:
    /// `networkCapabilitiesRestrictedForCallerPermissions`, then
    /// `createWithLocationInfoSanitizedIfNecessaryWhenParceled`. Without
    /// transport info nothing is location-sensitive but the owner, which
    /// only its own app sees, a VPN's always, another network's if
    /// `owner_sees` (location info asked for or a target before S, and
    /// the location permission).
    pub fn redacted_for(
        &self,
        uid: i32,
        held: &CallerPermissions,
        owner_sees: impl FnOnce() -> Result<bool, Exception>,
    ) -> Result<NetworkCapabilities, Exception> {
        let mut nc = self.clone();
        if !held.settings {
            nc.uids = None;
            nc.ssid = None;
        }
        // An Ethernet specifier redacts to itself.
        if !held.network_stack {
            nc.administrator_uids.clear();
        }
        if !(uid == SYSTEM_UID || nc.owner_uid == uid || held.factory) {
            nc.allowed_uids.clear();
        }
        if nc.underlying.is_some() && !held.factory_or_settings {
            nc.underlying = None;
        }
        if uid != nc.owner_uid || (!nc.has_transport(TRANSPORT_VPN) && !owner_sees()?) {
            nc.owner_uid = INVALID_UID;
        }
        Ok(nc)
    }
}

/// The permissions a caller's view of a network's capabilities depends
/// on.
pub struct CallerPermissions {
    /// NETWORK_SETTINGS or MAINLINE_NETWORK_STACK.
    pub settings: bool,
    /// NETWORK_STACK or MAINLINE_NETWORK_STACK.
    pub network_stack: bool,
    /// NETWORK_FACTORY.
    pub factory: bool,
    /// NETWORK_FACTORY, NETWORK_SETTINGS, MAINLINE_NETWORK_STACK, or the
    /// Bluetooth uid.
    pub factory_or_settings: bool,
}

impl ReadParcelable for NetworkCapabilities {
    /// `CREATOR.createFromParcel`. A specifier other than Ethernet's and
    /// MatchAll, or any transport info, is `BAD_TYPE`.
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let capabilities = r.read_i64()? as u64 & ALL_VALID_CAPABILITIES;
        let forbidden = r.read_i64()? as u64 & ALL_VALID_CAPABILITIES;
        let transports = r.read_i64()? as u64 & ALL_VALID_TRANSPORTS;
        let link_up_kbps = r.read_i32()?;
        let link_down_kbps = r.read_i32()?;
        let specifier = match parcelable_class(r)?.as_deref() {
            None => None,
            Some(ETHERNET_SPECIFIER) => {
                Some(Specifier::Ethernet(r.read_string16()?.ok_or(BAD_VALUE)?))
            }
            Some(MATCH_ALL_SPECIFIER) => Some(Specifier::MatchAll),
            Some(_) => return Err(BAD_TYPE),
        };
        if parcelable_class(r)?.is_some() {
            return Err(BAD_TYPE);
        }
        let signal_strength = r.read_i32()?;
        let uids = match r.read_i32()? {
            n if n < 0 => None,
            n => Some(
                (0..n)
                    .map(|_| match parcelable_class(r)?.as_deref() {
                        Some("android.net.UidRange") => Ok((r.read_i32()?, r.read_i32()?)),
                        _ => Err(BAD_VALUE),
                    })
                    .collect::<ParcelResult<_>>()?,
            ),
        };
        let allowed_uids = aim_service_aidl::read_int_array(r)?.unwrap_or_default();
        let ssid = r.read_string16()?;
        let private_dns_broken = r.read_bool()?;
        let administrator_uids = aim_service_aidl::read_int_array(r)?.unwrap_or_default();
        let owner_uid = r.read_i32()?;
        let requestor_uid = r.read_i32()?;
        let requestor_package = r.read_string16()?;
        let sub_ids = aim_service_aidl::read_int_array(r)?.ok_or(BAD_VALUE)?;
        let underlying = aim_service_aidl::read_typed_list(r)?;
        Ok(NetworkCapabilities {
            capabilities,
            forbidden,
            transports,
            link_up_kbps,
            link_down_kbps,
            specifier,
            signal_strength,
            uids,
            allowed_uids,
            ssid,
            private_dns_broken,
            administrator_uids,
            owner_uid,
            requestor_uid,
            requestor_package,
            sub_ids,
            underlying,
            enterprise_id: r.read_i32()? & ALL_VALID_ENTERPRISE_IDS,
            reservation_id: r.read_i32()?,
            match_non_thread_local: r.read_bool()?,
        })
    }
}

impl WriteParcelable for NetworkCapabilities {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64((self.capabilities & ALL_VALID_CAPABILITIES) as i64);
        p.write_i64((self.forbidden & ALL_VALID_CAPABILITIES) as i64);
        p.write_i64((self.transports & ALL_VALID_TRANSPORTS) as i64);
        p.write_i32(self.link_up_kbps);
        p.write_i32(self.link_down_kbps);
        match &self.specifier {
            Some(s) => write_parcelable(p, s.class(), Some(s)),
            None => p.write_string16(None),
        }
        p.write_string16(None); // transport info
        p.write_i32(self.signal_strength);
        match &self.uids {
            None => p.write_i32(-1),
            Some(ranges) => {
                p.write_i32(ranges.len() as i32);
                for &(start, stop) in ranges {
                    p.write_string16(Some("android.net.UidRange"));
                    p.write_i32(start);
                    p.write_i32(stop);
                }
            }
        }
        aim_service_aidl::write_int_array(p, Some(&self.allowed_uids));
        p.write_string16(self.ssid.as_deref());
        p.write_bool(self.private_dns_broken);
        aim_service_aidl::write_int_array(p, Some(&self.administrator_uids));
        p.write_i32(self.owner_uid);
        p.write_i32(self.requestor_uid);
        p.write_string16(self.requestor_package.as_deref());
        aim_service_aidl::write_int_array(p, Some(&self.sub_ids));
        aim_service_aidl::write_typed_list(p, self.underlying.as_deref());
        p.write_i32(self.enterprise_id & ALL_VALID_ENTERPRISE_IDS);
        p.write_i32(self.reservation_id);
        p.write_bool(self.match_non_thread_local);
    }
}

/// `NetworkRequest.Type`, by ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestType {
    None,
    Listen,
    TrackDefault,
    Request,
    BackgroundRequest,
    TrackSystemDefault,
    ListenForBest,
    Reservation,
}

const REQUEST_TYPES: &[(RequestType, &str)] = &[
    (RequestType::None, "NONE"),
    (RequestType::Listen, "LISTEN"),
    (RequestType::TrackDefault, "TRACK_DEFAULT"),
    (RequestType::Request, "REQUEST"),
    (RequestType::BackgroundRequest, "BACKGROUND_REQUEST"),
    (RequestType::TrackSystemDefault, "TRACK_SYSTEM_DEFAULT"),
    (RequestType::ListenForBest, "LISTEN_FOR_BEST"),
    (RequestType::Reservation, "RESERVATION"),
];

impl RequestType {
    pub fn from_ordinal(ordinal: i32) -> Option<RequestType> {
        REQUEST_TYPES
            .get(usize::try_from(ordinal).ok()?)
            .map(|(t, _)| *t)
    }

    pub fn name(self) -> &'static str {
        REQUEST_TYPES
            .iter()
            .find(|(t, _)| *t == self)
            .map_or("NONE", |(_, n)| n)
    }
}

/// `NetworkRequest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkRequest {
    pub caps: NetworkCapabilities,
    pub legacy_type: i32,
    pub request_id: i32,
    pub kind: RequestType,
}

impl NetworkRequest {
    pub fn is_listen(&self) -> bool {
        self.kind == RequestType::Listen
    }
}

impl ReadParcelable for NetworkRequest {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let caps = NetworkCapabilities::read_from(r)?;
        let legacy_type = r.read_i32()?;
        let request_id = r.read_i32()?;
        let name = r.read_string16()?.ok_or(BAD_VALUE)?;
        let kind = REQUEST_TYPES
            .iter()
            .find(|(_, n)| *n == name)
            .ok_or(BAD_VALUE)?
            .0;
        Ok(NetworkRequest {
            caps,
            legacy_type,
            request_id,
            kind,
        })
    }
}

impl WriteParcelable for NetworkRequest {
    fn write_to(&self, p: &mut Parcel) {
        self.caps.write_to(p);
        p.write_i32(self.legacy_type);
        p.write_i32(self.request_id);
        p.write_string16(Some(self.kind.name()));
    }
}

/// `NetworkInfo`, its states by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkInfo {
    pub network_type: i32,
    pub subtype: i32,
    pub type_name: String,
    pub subtype_name: String,
    pub state: &'static str,
    pub detailed_state: &'static str,
    pub failover: bool,
    pub available: bool,
    pub roaming: bool,
    pub reason: Option<String>,
    pub extra_info: Option<String>,
}

impl NetworkInfo {
    /// `setDetailedState(BLOCKED, null, null)`: disconnected.
    pub fn block(&mut self) {
        self.state = "DISCONNECTED";
        self.detailed_state = "BLOCKED";
        self.reason = None;
        self.extra_info = None;
    }
}

impl WriteParcelable for NetworkInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.network_type);
        p.write_i32(self.subtype);
        p.write_string16(Some(&self.type_name));
        p.write_string16(Some(&self.subtype_name));
        p.write_string16(Some(self.state));
        p.write_string16(Some(self.detailed_state));
        p.write_i32(self.failover as i32);
        p.write_i32(self.available as i32);
        p.write_i32(self.roaming as i32);
        p.write_string16(self.reason.as_deref());
        p.write_string16(self.extra_info.as_deref());
    }
}

/// An IPv4 `LinkAddress`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkAddress {
    pub addr: Ipv4Addr,
    pub prefix: u8,
    pub flags: i32,
    pub scope: i32,
    pub deprecation: i64,
    pub expiration: i64,
}

impl WriteParcelable for LinkAddress {
    fn write_to(&self, p: &mut Parcel) {
        aim_service_aidl::write_byte_array(p, Some(&self.addr.octets()));
        p.write_i32(self.prefix as i32);
        p.write_i32(self.flags);
        p.write_i32(self.scope);
        p.write_i64(self.deprecation);
        p.write_i64(self.expiration);
    }
}

/// An IPv4 `RouteInfo`: its destination (`IpPrefix`), gateway (0.0.0.0
/// for a directly connected one), interface, type and MTU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteInfo {
    pub destination: (Ipv4Addr, u8),
    pub gateway: Ipv4Addr,
    pub interface: String,
    pub kind: i32,
    pub mtu: i32,
}

struct IpPrefix(Ipv4Addr, u8);

impl WriteParcelable for IpPrefix {
    fn write_to(&self, p: &mut Parcel) {
        aim_service_aidl::write_byte_array(p, Some(&self.0.octets()));
        p.write_i32(self.1 as i32);
    }
}

impl WriteParcelable for RouteInfo {
    fn write_to(&self, p: &mut Parcel) {
        let (addr, prefix) = self.destination;
        write_parcelable(p, "android.net.IpPrefix", Some(&IpPrefix(addr, prefix)));
        aim_service_aidl::write_byte_array(p, Some(&self.gateway.octets()));
        p.write_string16(Some(&self.interface));
        p.write_i32(self.kind);
        p.write_i32(self.mtu);
    }
}

/// IPv4 `LinkProperties`: no proxy, NAT64 prefix, stacked links, P-CSCF
/// servers or captive portal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkProperties {
    pub interface: String,
    pub addresses: Vec<LinkAddress>,
    pub dns_servers: Vec<Ipv4Addr>,
    pub use_private_dns: bool,
    pub private_dns_server: Option<String>,
    pub domains: Option<String>,
    pub dhcp_server: Option<Ipv4Addr>,
    pub mtu: i32,
    pub routes: Vec<RouteInfo>,
    pub wake_on_lan: bool,
}

impl WriteParcelable for LinkProperties {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.interface));
        p.write_i32(self.addresses.len() as i32);
        for a in &self.addresses {
            write_parcelable(p, "android.net.LinkAddress", Some(a));
        }
        // writeAddresses: an IPv4 address is its bytes alone.
        p.write_i32(self.dns_servers.len() as i32);
        for d in &self.dns_servers {
            aim_service_aidl::write_byte_array(p, Some(&d.octets()));
        }
        p.write_i32(0); // validated private DNS servers
        p.write_bool(self.use_private_dns);
        p.write_string16(self.private_dns_server.as_deref());
        p.write_i32(0); // P-CSCF servers
        p.write_string16(self.domains.as_deref());
        aim_service_aidl::write_byte_array(
            p,
            self.dhcp_server
                .map(|a| a.octets())
                .as_ref()
                .map(|a| &a[..]),
        );
        p.write_i32(self.mtu);
        p.write_string16(None); // TCP buffer sizes
        p.write_i32(self.routes.len() as i32);
        for r in &self.routes {
            write_parcelable(p, "android.net.RouteInfo", Some(r));
        }
        p.write_i32(0); // writeByte: no HTTP proxy
        p.write_string16(None); // NAT64 prefix
        p.write_i32(0); // writeList: no stacked links
        p.write_bool(self.wake_on_lan);
        p.write_string16(None); // captive portal API URL, not parceled
        p.write_string16(None); // captive portal data, not parceled
    }
}

/// `Messenger`: its target, if any.
pub struct Messenger(pub Option<Binder>);

impl ReadParcelable for Messenger {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Messenger(r.read_binder()?))
    }
}

/// `String.hashCode`, which orders an `ArrayMap`'s keys.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// A callback `Message` (`callCallbackForRequest`): `what` and `arg1`,
/// and its data, a bundle of parcelables under their classes' simple
/// names (`putParcelable`), in its `ArrayMap`'s order.
pub struct Message<'a> {
    what: i32,
    arg1: i32,
    data: Vec<(&'a str, Value<'a>)>,
}

impl<'a> Message<'a> {
    pub fn new(what: i32, arg1: i32, mut data: Vec<(&'a str, Value<'a>)>) -> Self {
        data.sort_by_key(|(k, _)| java_hash(k));
        Message { what, arg1, data }
    }
}

impl WriteParcelable for Message<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.what);
        p.write_i32(self.arg1);
        p.write_i32(0); // arg2
        p.write_i32(0); // no obj
        p.write_i64(0); // when
        write_bundle(p, &self.data);
        p.write_binder(None); // replyTo
        p.write_i32(-1); // sendingUid: UID_NONE
        p.write_i32(-1); // workSourceUid: UID_NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(p: &Parcel) -> Vec<i32> {
        p.data()
            .chunks(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn string16(s: &str) -> Vec<i32> {
        let mut p = Parcel::new();
        p.write_string16(Some(s));
        ints(&p)
    }

    #[test]
    fn capabilities_round_trip_as_the_creator_reads_them() {
        let mut nc = NetworkCapabilities::default();
        nc.add(INTERNET);
        nc.transports = 1 << TRANSPORT_ETHERNET;
        nc.specifier = Some(Specifier::Ethernet("eth0".into()));
        nc.set_single_uid(10_123);
        nc.requestor_package = Some("com.example".into());
        let mut p = Parcel::new();
        nc.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(NetworkCapabilities::read_from(&mut r).unwrap(), nc);
        assert_eq!(r.remaining(), 0);
    }

    /// `writeToParcel` of `new NetworkCapabilities()`, field by field.
    #[test]
    fn default_capabilities_are_written_field_by_field() {
        let mut p = Parcel::new();
        NetworkCapabilities::default().write_to(&mut p);
        let caps = DEFAULT_CAPABILITIES as i64;
        // The capabilities, forbidden and transports (longs), then the
        // link bandwidths.
        let mut want = vec![caps as i32, (caps >> 32) as i32, 0, 0, 0, 0, 0, 0];
        want.extend([-1, -1]); // specifier, transport info
        want.push(i32::MIN); // signal strength
        want.push(-1); // uids
        want.push(0); // allowed uids
        want.push(-1); // SSID
        want.push(0); // private DNS broken
        want.push(0); // administrator uids
        want.extend([-1, -1, -1]); // owner, requestor, requestor package
        want.push(0); // sub ids
        want.push(-1); // underlying networks
        want.extend([0, 0, 0]); // enterprise id, reservation id, local matching
        assert_eq!(ints(&p), want);
        assert_eq!(
            DEFAULT_CAPABILITIES,
            (1 << 13) | (1 << 14) | (1 << 15) | (1 << 37)
        );
    }

    #[test]
    fn an_unknown_specifier_is_not_read() {
        let mut p = Parcel::new();
        p.write_i64(0);
        p.write_i64(0);
        p.write_i64(0);
        p.write_i32(0);
        p.write_i32(0);
        p.write_string16(Some("android.net.wifi.WifiNetworkSpecifier"));
        assert_eq!(
            NetworkCapabilities::read_from(&mut p.reader()),
            Err(BAD_TYPE)
        );
    }

    #[test]
    fn requests_match_as_satisfied_by_network_capabilities() {
        let mut network = NetworkCapabilities::default();
        for c in [INTERNET, NOT_METERED, VALIDATED, FOREGROUND] {
            network.add(c);
        }
        network.transports = 1 << TRANSPORT_ETHERNET;
        network.specifier = Some(Specifier::Ethernet("eth0".into()));
        let mut request = NetworkCapabilities::default();
        request.add(INTERNET);
        request.set_single_uid(10_100);
        request.requestor_uid = 10_100;
        assert!(request.satisfied_by(&network));
        let mut wifi = request.clone();
        wifi.transports = 1 << 1;
        assert!(!wifi.satisfied_by(&network));
        let mut not_metered_forbidden = request.clone();
        not_metered_forbidden.forbid(NOT_METERED);
        assert!(!not_metered_forbidden.satisfied_by(&network));
        let mut other_iface = request.clone();
        other_iface.specifier = Some(Specifier::Ethernet("eth1".into()));
        assert!(!other_iface.satisfied_by(&network));
        let mut bandwidth = request.clone();
        bandwidth.link_down_kbps = 1;
        assert!(!bandwidth.satisfied_by(&network));
        network.link_down_kbps = 100_000;
        assert!(bandwidth.satisfied_by(&network));
        // A network for one uid range does not satisfy another's request.
        network.uids = Some(vec![(10_000, 10_050)]);
        assert!(!request.satisfied_by(&network));
    }

    #[test]
    fn capabilities_are_redacted_for_the_caller() {
        let mut nc = NetworkCapabilities::default();
        nc.uids = Some(vec![(10_000, 10_999)]);
        nc.ssid = Some("\"home\"".into());
        nc.administrator_uids = vec![10_005];
        nc.allowed_uids = vec![10_006];
        nc.underlying = Some(vec![Some(Network(100))]);
        nc.owner_uid = 10_005;
        let none = CallerPermissions {
            settings: false,
            network_stack: false,
            factory: false,
            factory_or_settings: false,
        };
        let app = nc.redacted_for(10_100, &none, || unreachable!()).unwrap();
        assert_eq!(
            (&app.uids, &app.ssid, app.owner_uid),
            (&None, &None, INVALID_UID)
        );
        assert!(
            app.administrator_uids.is_empty()
                && app.allowed_uids.is_empty()
                && app.underlying.is_none()
        );
        let all = CallerPermissions {
            settings: true,
            network_stack: true,
            factory: true,
            factory_or_settings: true,
        };
        let privileged = nc.redacted_for(10_100, &all, || unreachable!()).unwrap();
        assert_eq!(
            privileged,
            NetworkCapabilities {
                owner_uid: INVALID_UID,
                ..nc.clone()
            }
        );
        // The owner sees its allowed uids, and itself when it may.
        let owner = nc.redacted_for(10_005, &none, || Ok(true)).unwrap();
        assert_eq!(
            (owner.owner_uid, &owner.allowed_uids[..]),
            (10_005, &[10_006][..])
        );
        assert_eq!(
            nc.redacted_for(10_005, &none, || Ok(false))
                .unwrap()
                .owner_uid,
            INVALID_UID
        );
    }

    #[test]
    fn non_requestable_capabilities_are_named() {
        let mut nc = NetworkCapabilities::default();
        assert_eq!(nc.first_non_requestable(), None);
        nc.add(NOT_VCN_MANAGED);
        assert_eq!(nc.first_non_requestable(), None);
        nc.forbid(FOREGROUND);
        nc.add(VALIDATED);
        assert_eq!(nc.first_non_requestable(), Some("VALIDATED"));
        let mut signal = NetworkCapabilities::default();
        signal.signal_strength = -60;
        assert_eq!(signal.first_non_requestable(), Some("signalStrength"));
    }

    #[test]
    fn requests_are_written_with_their_type_by_name() {
        let r = NetworkRequest {
            caps: NetworkCapabilities::cleared(),
            legacy_type: TYPE_NONE,
            request_id: 7,
            kind: RequestType::TrackDefault,
        };
        let mut p = Parcel::new();
        r.write_to(&mut p);
        let all = ints(&p);
        let tail = [vec![-1, 7], string16("TRACK_DEFAULT")].concat();
        assert_eq!(all[all.len() - tail.len()..], tail[..]);
        assert_eq!(NetworkRequest::read_from(&mut p.reader()).unwrap(), r);
        assert_eq!(
            RequestType::from_ordinal(6),
            Some(RequestType::ListenForBest)
        );
        assert_eq!(RequestType::from_ordinal(8), None);
    }

    #[test]
    fn network_info_is_written_as_write_to_parcel() {
        let mut info = NetworkInfo {
            network_type: TYPE_ETHERNET,
            subtype: TYPE_NONE,
            type_name: "Ethernet".into(),
            subtype_name: String::new(),
            state: "CONNECTED",
            detailed_state: "CONNECTED",
            failover: false,
            available: true,
            roaming: false,
            reason: None,
            extra_info: Some("02:61:69:6d:00:02".into()),
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let want = [
            vec![9, -1],
            string16("Ethernet"),
            string16(""),
            string16("CONNECTED"),
            string16("CONNECTED"),
            vec![0, 1, 0, -1],
            string16("02:61:69:6d:00:02"),
        ]
        .concat();
        assert_eq!(ints(&p), want);
        info.block();
        assert_eq!(
            (info.state, info.detailed_state, info.extra_info),
            ("DISCONNECTED", "BLOCKED", None)
        );
    }

    #[test]
    fn link_properties_are_written_as_write_to_parcel() {
        let lp = LinkProperties {
            interface: "eth0".into(),
            addresses: vec![LinkAddress {
                addr: Ipv4Addr::new(192, 168, 1, 20),
                prefix: 24,
                flags: 0x80,
                scope: 0,
                deprecation: LIFETIME_PERMANENT,
                expiration: LIFETIME_PERMANENT,
            }],
            dns_servers: vec![Ipv4Addr::new(192, 168, 1, 1)],
            use_private_dns: false,
            private_dns_server: None,
            domains: None,
            dhcp_server: Some(Ipv4Addr::new(192, 168, 1, 1)),
            mtu: 1500,
            routes: vec![RouteInfo {
                destination: (Ipv4Addr::UNSPECIFIED, 0),
                gateway: Ipv4Addr::new(192, 168, 1, 1),
                interface: "eth0".into(),
                kind: RTN_UNICAST,
                mtu: 0,
            }],
            wake_on_lan: false,
        };
        let mut p = Parcel::new();
        lp.write_to(&mut p);
        let bytes = |a: [u8; 4]| vec![4, i32::from_le_bytes(a)];
        let want = [
            string16("eth0"),
            vec![1],
            string16("android.net.LinkAddress"),
            bytes([192, 168, 1, 20]),
            vec![24, 0x80, 0, -1, i32::MAX, -1, i32::MAX],
            vec![1],
            bytes([192, 168, 1, 1]),
            vec![0, 0, -1, 0, -1],
            bytes([192, 168, 1, 1]),
            vec![1500, -1, 1],
            string16("android.net.RouteInfo"),
            string16("android.net.IpPrefix"),
            bytes([0, 0, 0, 0]),
            vec![0],
            bytes([192, 168, 1, 1]),
            string16("eth0"),
            vec![RTN_UNICAST, 0],
            vec![0, -1, 0, 0, -1, -1],
        ]
        .concat();
        assert_eq!(ints(&p), want);
    }

    /// A callback message's bundle is in `ArrayMap` order: by the keys'
    /// `hashCode`.
    #[test]
    fn messages_keep_their_bundles_in_array_map_order() {
        assert_eq!(java_hash("Network"), -786_828_786);
        let mut keys = [
            "NetworkRequest",
            "Network",
            "NetworkCapabilities",
            "LinkProperties",
            "LocalNetworkInfo",
        ];
        keys.sort_by_key(|k| java_hash(k));
        assert_eq!(
            keys,
            [
                "NetworkCapabilities",
                "NetworkRequest",
                "LinkProperties",
                "Network",
                "LocalNetworkInfo"
            ]
        );
        let network = Network(100);
        let message = Message::new(
            4,
            0,
            vec![
                (
                    "Network",
                    Value::Parcelable("android.net.Network", &network),
                ),
                ("NetworkRequest", Value::Int(1)),
            ],
        );
        let mut p = Parcel::new();
        message.write_to(&mut p);
        let all = ints(&p);
        assert_eq!(all[..6], [4, 0, 0, 0, 0, 0]);
        // The bundle: its length, the magic, two entries, NetworkRequest
        // first.
        assert_eq!(all[7..9], [crate::bundle::MAGIC, 2]);
        let key = string16("NetworkRequest");
        assert_eq!(all[9..9 + key.len()], key[..]);
        assert_eq!(all[all.len() - 2..], [-1, -1]);
    }
}
