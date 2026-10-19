//! `connectivity`: ConnectivityService for the app host
//! (`ConnectivityService` of the Connectivity module at
//! `android-16.0.0_r1`, ADR 0013, #1509). It has one network, the Mac's,
//! which guest sockets really use (they are host sockets), described as
//! the original describes the validated Ethernet network `eth0` stands
//! for in a boot (docs/network.md): `EthernetTracker`'s default
//! capabilities with the interface's specifier, VALIDATED and FOREGROUND
//! as ConnectivityService marks its default network, the agent's legacy
//! `NetworkInfo`, and the link properties IpClient makes of the virtual
//! router's lease. It follows the Mac (`aim_uplink`): no network while the
//! Mac has none, a new one under a new netId when the Mac moves to another
//! network, new link properties when only its DNS servers change. Apps
//! read it with the original's checks and redaction, and file requests and
//! listens whose callbacks go to their `Messenger` as
//! `callCallbackForRequest` sends them, to the methods the callback
//! declares. The app host's apps are all top, so no firewall chain and
//! no data saver blocks them; an app without INTERNET is blocked as the
//! original blocks it. PendingIntent requests, reservations, agents,
//! providers and offers, VPNs, keepalives, tethering, diagnostics and the
//! rest are not implemented (#1509).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_TYPE, BAD_VALUE, Binder, EX_NULL_POINTER, EX_SERVICE_SPECIFIC, Exception, Parcel,
};
use aim_service_aidl::android_net_iconnectivitymanager as cm;
use aim_service_aidl::android_os_imessenger as imessenger;
use aim_uplink::Uplink;

use super::appops_service::{self, MODE_DEFAULT, OP_WRITE_SETTINGS};
use super::network::{
    self, CallerPermissions, FOREGROUND, INTERNET, INVALID_UID, LIFETIME_PERMANENT, LOCAL_NETWORK,
    LinkAddress, LinkProperties, Message, Messenger, NOT_CONGESTED, NOT_METERED, NOT_RESTRICTED,
    NOT_ROAMING, NOT_SUSPENDED, NOT_VCN_MANAGED, NOT_VPN, Network, NetworkCapabilities,
    NetworkInfo, NetworkRequest, RTN_UNICAST, RequestType, RouteInfo, Specifier,
    TRANSPORT_ETHERNET, TRANSPORT_WIFI, TYPE_ETHERNET, TYPE_NONE, VALIDATED,
};
use super::network_native;
use super::settings::Table;
use super::{AppHost, PER_USER_RANGE, reply};
use crate::location::parcels::Value;

const ACCESS_NETWORK_STATE: &str = "android.permission.ACCESS_NETWORK_STATE";
const ACCESS_WIFI_STATE: &str = "android.permission.ACCESS_WIFI_STATE";
const CHANGE_NETWORK_STATE: &str = "android.permission.CHANGE_NETWORK_STATE";
const WRITE_SETTINGS: &str = "android.permission.WRITE_SETTINGS";
const INTERNET_PERMISSION: &str = "android.permission.INTERNET";
const NETWORK_SETTINGS: &str = "android.permission.NETWORK_SETTINGS";
const NETWORK_SETUP_WIZARD: &str = "android.permission.NETWORK_SETUP_WIZARD";
const NETWORK_STACK: &str = "android.permission.NETWORK_STACK";
const MAINLINE_NETWORK_STACK: &str = "android.permission.MAINLINE_NETWORK_STACK";
const NETWORK_FACTORY: &str = "android.permission.NETWORK_FACTORY";
const NETWORK_SIGNAL_STRENGTH_WAKEUP: &str = "android.permission.NETWORK_SIGNAL_STRENGTH_WAKEUP";
const CONNECTIVITY_USE_RESTRICTED_NETWORKS: &str =
    "android.permission.CONNECTIVITY_USE_RESTRICTED_NETWORKS";
const CONNECTIVITY_INTERNAL: &str = "android.permission.CONNECTIVITY_INTERNAL";
/// `ConnectivityManager.CALLBACK_*` the service sends.
const CALLBACK_AVAILABLE: i32 = 2;
const CALLBACK_LOST: i32 = 4;
const CALLBACK_UNAVAIL: i32 = 5;
const CALLBACK_CAP_CHANGED: i32 = 6;
const CALLBACK_IP_CHANGED: i32 = 7;
/// `NetworkCallback.DECLARED_METHODS_ALL`, `FLAG_INCLUDE_LOCATION_INFO`.
const DECLARED_METHODS_ALL: i32 = !0;
const FLAG_INCLUDE_LOCATION_INFO: i32 = 1;
/// `ConnectivityManager.FEATURE_USE_DECLARED_METHODS_FOR_CALLBACKS`,
/// `FEATURE_QUEUE_NETWORK_AGENT_EVENTS_IN_SYSTEM_SERVER`, and the
/// tethering flags that switch them off (`ConnectivityFlags`).
const FEATURES: &[(i64, &str)] = &[
    (1, "use_declared_methods_for_callbacks"),
    (1 << 1, "queue_network_agent_events_in_system_server"),
];
/// The request limits: one less than `MAX_NETWORK_REQUESTS_PER_UID` and
/// `_PER_SYSTEM_UID`, as the legacy counter throws at the limit;
/// `ConnectivityManager.Errors.TOO_MANY_REQUESTS`.
const MAX_REQUESTS_PER_UID: i32 = 100 - 1;
const MAX_REQUESTS_PER_SYSTEM_UID: i32 = 250 - 1;
const TOO_MANY_REQUESTS: i32 = 1;
/// `ConnectivityManager.BLOCKED_REASON_NETWORK_RESTRICTED`,
/// `MULTIPATH_PREFERENCE_UNMETERED`, `RESTRICT_BACKGROUND_STATUS_*`,
/// `MAX_NETWORK_TYPE`.
const BLOCKED_REASON_NETWORK_RESTRICTED: i32 = 1 << 8;
const MULTIPATH_PREFERENCE_UNMETERED: i32 = 7;
const RESTRICT_BACKGROUND_STATUS_DISABLED: i32 = 1;
const RESTRICT_BACKGROUND_STATUS_WHITELISTED: i32 = 2;
const RESTRICT_BACKGROUND_STATUS_ENABLED: i32 = 3;
const MAX_NETWORK_TYPE: i32 = 18;
/// `NetworkPolicyManager.POLICY_REJECT_METERED_BACKGROUND`,
/// `POLICY_ALLOW_METERED_BACKGROUND`.
const POLICY_REJECT_METERED_BACKGROUND: i32 = 1;
const POLICY_ALLOW_METERED_BACKGROUND: i32 = 4;
/// Compat changes (`ConnectivityCompatChanges`).
const NETWORK_BLOCKED_WITHOUT_INTERNET_PERMISSION: i64 = 333_340_911;
const ENABLE_MATCH_LOCAL_NETWORK: i64 = 319_212_206;
const ENABLE_MATCH_NON_THREAD_LOCAL_NETWORKS: i64 = 349_487_600;
const ENABLE_SELF_CERTIFIED_CAPABILITIES_DECLARATION: i64 = 266_524_688;
/// `NET_CAPABILITY_PRIORITIZE_LATENCY`, `_BANDWIDTH`.
const PRIORITIZE_LATENCY: u32 = 34;
const PRIORITIZE_BANDWIDTH: u32 = 35;
/// `Process.SYSTEM_UID`, `BLUETOOTH_UID`, `FIRST_APPLICATION_UID`,
/// `LAST_APPLICATION_UID`.
const SYSTEM_UID: i32 = 1000;
const BLUETOOTH_UID: i32 = 1002;
const FIRST_APPLICATION_UID: i32 = 10_000;
const LAST_APPLICATION_UID: i32 = 19_999;
/// `Build.VERSION_CODES.M`; `Build.VENDOR_API_2024_Q2`.
const M: i32 = 23;
const VENDOR_API_2024_Q2: i32 = 202_404;
/// `NetIdManager.MIN_NET_ID`; `NetworkRequest.FIRST_REQUEST_ID`.
const MIN_NET_ID: i32 = 100;
const FIRST_REQUEST_ID: i32 = 1;
/// The interface the guest knows the Mac's network by, and its hardware
/// address (netif's `eth0`), which the Ethernet agent's `NetworkInfo`
/// carries as its extra info; `EthernetNetworkFactory.NETWORK_TYPE`.
const INTERFACE: &str = "eth0";
const HW_ADDRESS: &str = "02:61:69:6d:00:02";
const NETWORK_TYPE: &str = "Ethernet";
/// `EthernetTracker.DEFAULT_CAPABILITIES`' link bandwidths.
const ETHERNET_KBPS: i32 = 100 * 1000;
/// The MTUs `DhcpPacket` takes from a lease (`MIN_MTU`, `MAX_MTU`).
const DHCP_MTUS: std::ops::RangeInclusive<u32> = 1280..=1500;
/// `IFA_F_PERMANENT`, the flag netif gives `eth0`'s address.
const IFA_F_PERMANENT: i32 = 0x80;

/// `ConnectivityManager.getNetworkTypeName`, by type.
const TYPE_NAMES: &[&str] = &[
    "MOBILE",
    "WIFI",
    "MOBILE_MMS",
    "MOBILE_SUPL",
    "MOBILE_DUN",
    "MOBILE_HIPRI",
    "WIMAX",
    "BLUETOOTH",
    "DUMMY",
    "ETHERNET",
    "MOBILE_FOTA",
    "MOBILE_IMS",
    "MOBILE_CBS",
    "WIFI_P2P",
    "MOBILE_IA",
    "MOBILE_EMERGENCY",
    "PROXY",
    "VPN",
    "TEST",
];

/// The network: the Mac's as last seen, under its netId.
struct Net {
    id: i32,
    uplink: Uplink,
    dns: Vec<Ipv4Addr>,
}

/// The network's capabilities: `EthernetTracker.DEFAULT_CAPABILITIES`
/// with the interface's specifier (`EthernetNetworkFactory.addInterface`),
/// validated and in the foreground, as ConnectivityService marks its
/// default network.
fn network_capabilities() -> NetworkCapabilities {
    let mut nc = NetworkCapabilities::default();
    for c in [
        INTERNET,
        NOT_CONGESTED,
        NOT_METERED,
        NOT_RESTRICTED,
        NOT_ROAMING,
        NOT_SUSPENDED,
        NOT_VCN_MANAGED,
    ] {
        nc.add(c);
    }
    nc.add(VALIDATED);
    nc.add(FOREGROUND);
    nc.transports = 1 << TRANSPORT_ETHERNET;
    nc.link_up_kbps = ETHERNET_KBPS;
    nc.link_down_kbps = ETHERNET_KBPS;
    nc.specifier = Some(Specifier::Ethernet(INTERFACE.into()));
    nc
}

/// The Ethernet agent's `NetworkInfo` (`getLegacyNetworkInfo`, then
/// `markConnected`).
fn network_info() -> NetworkInfo {
    NetworkInfo {
        network_type: TYPE_ETHERNET,
        subtype: TYPE_NONE,
        type_name: NETWORK_TYPE.into(),
        subtype_name: String::new(),
        state: "CONNECTED",
        detailed_state: "CONNECTED",
        failover: false,
        available: true,
        roaming: false,
        reason: None,
        extra_info: Some(HW_ADDRESS.into()),
    }
}

/// The link properties IpClient makes of the virtual router's lease
/// (`assembleLinkProperties`): the address as netif reports it, the
/// lease's routes (`StaticIpConfiguration.getRoutes`), DNS servers, MTU
/// (when `DhcpPacket` takes it) and server; private DNS as `DnsManager`
/// marks it with no validated server: in use only in strict mode, by
/// its specifier (`private_dns`: None in another mode).
fn link_properties(
    up: &Uplink,
    dns: &[Ipv4Addr],
    private_dns: Option<Option<String>>,
) -> LinkProperties {
    let mask = u32::MAX.checked_shl(32 - up.prefix as u32).unwrap_or(0);
    let subnet = Ipv4Addr::from(u32::from(up.addr) & mask);
    let route = |destination, gateway| RouteInfo {
        destination,
        gateway,
        interface: INTERFACE.into(),
        kind: RTN_UNICAST,
        mtu: 0,
    };
    let mut routes = vec![route((subnet, up.prefix), Ipv4Addr::UNSPECIFIED)];
    if u32::from(up.gateway) & mask != u32::from(subnet) {
        routes.push(route((up.gateway, 32), Ipv4Addr::UNSPECIFIED));
    }
    routes.push(route((Ipv4Addr::UNSPECIFIED, 0), up.gateway));
    LinkProperties {
        interface: INTERFACE.into(),
        addresses: vec![LinkAddress {
            addr: up.addr,
            prefix: up.prefix,
            flags: IFA_F_PERMANENT,
            scope: 0,
            deprecation: LIFETIME_PERMANENT,
            expiration: LIFETIME_PERMANENT,
        }],
        dns_servers: dns.to_vec(),
        use_private_dns: private_dns.is_some(),
        private_dns_server: private_dns.flatten(),
        domains: None,
        dhcp_server: Some(up.gateway),
        mtu: if DHCP_MTUS.contains(&up.mtu) {
            up.mtu as i32
        } else {
            0
        },
        routes,
        wake_on_lan: false,
    }
}

/// The default request's capabilities (`createDefaultInternetRequestForTransport`),
/// filed by the system.
fn default_request_capabilities() -> NetworkCapabilities {
    let mut nc = NetworkCapabilities::default();
    nc.add(INTERNET);
    nc.add(NOT_VCN_MANAGED);
    nc.requestor_uid = SYSTEM_UID;
    nc.requestor_package = Some("android".into());
    nc
}

/// `restrictRequestUidsForCallerAndSetRequestorInfo`: a caller without
/// NETWORK_SETTINGS (`settings`) asks for its own uid, or for every uid;
/// the requestor is the caller; no administrators or owner.
fn restrict_uids_and_set_requestor(
    nc: &mut NetworkCapabilities,
    uid: i32,
    package: Option<String>,
    settings: bool,
) {
    // A specifier of every uid's request would be redacted: an Ethernet
    // one redacts to itself.
    if !settings && nc.uids.is_some() {
        nc.set_single_uid(uid);
    }
    nc.requestor_uid = uid;
    nc.requestor_package = package;
    nc.administrator_uids.clear();
    nc.owner_uid = INVALID_UID;
}

/// `copyDefaultNetworkCapabilitiesForUid`: the default request's, for
/// `as_uid`'s default network, filed by `uid`.
fn default_capabilities_for_uid(
    as_uid: i32,
    uid: i32,
    package: Option<String>,
    settings: bool,
) -> NetworkCapabilities {
    let mut nc = default_request_capabilities();
    nc.remove(NOT_VPN);
    nc.set_single_uid(as_uid);
    restrict_uids_and_set_requestor(&mut nc, uid, package, settings);
    nc
}

/// `ensureListenableCapabilities` and, for a request,
/// `ensureRequestableCapabilities`.
fn ensure_capabilities(nc: &NetworkCapabilities, request: bool) -> Result<(), Exception> {
    if nc.specifier == Some(Specifier::MatchAll) {
        return Err(Exception::illegal_argument(
            "A MatchAllNetworkSpecifier is not permitted",
        ));
    }
    if nc.private_dns_broken {
        return Err(Exception::illegal_argument(
            "Can't request broken private DNS",
        ));
    }
    if !nc.allowed_uids.is_empty() {
        return Err(Exception::illegal_argument("Can't request access UIDs"));
    }
    match nc.first_non_requestable().filter(|_| request) {
        Some(bad) => Err(Exception::illegal_argument(format!(
            "Cannot request network with {bad}"
        ))),
        None => Ok(()),
    }
}

/// NetworkPolicyManagerService's data saver state as it persisted it
/// (`readPolicyAL`): whether background data is restricted, and the uid
/// policies, with the restrict-background allowlist folded in.
#[derive(Debug, Default, PartialEq)]
struct NetPolicy {
    restrict_background: bool,
    uid_policies: HashMap<i32, i32>,
}

/// `UserHandle.isApp`.
fn is_app(uid: i32) -> bool {
    (FIRST_APPLICATION_UID..=LAST_APPLICATION_UID).contains(&uid.rem_euclid(PER_USER_RANGE as i32))
}

impl NetPolicy {
    /// `netpolicy.xml`, or without one, the device's default
    /// (`upgradeDefaultBackgroundDataUL`).
    fn read(bytes: Option<&[u8]>, default_restrict: bool) -> Result<NetPolicy, String> {
        let Some(bytes) = bytes else {
            return Ok(NetPolicy {
                restrict_background: default_restrict,
                ..Default::default()
            });
        };
        let root = aim_android_xml::read(bytes)?;
        let mut policy = NetPolicy::default();
        if root.name == "policy-list" {
            let version = root.int("version")?.ok_or("policy-list: no version")?;
            policy.restrict_background = version >= 3
                && root
                    .bool("restrictBackground")?
                    .ok_or("policy-list: no restrictBackground")?;
        }
        let mut allowed = Vec::new();
        for e in root.children() {
            let uid = || e.int("uid")?.ok_or_else(|| format!("<{}>: no uid", e.name));
            match e.name.as_str() {
                "uid-policy" => {
                    let uid = uid()?;
                    if is_app(uid) {
                        policy
                            .uid_policies
                            .insert(uid, e.int("policy")?.ok_or("uid-policy: no policy")?);
                    }
                }
                "app-policy" => {
                    let uid = e.int("appId")?.ok_or("app-policy: no appId")?;
                    if is_app(uid) {
                        policy
                            .uid_policies
                            .insert(uid, e.int("policy")?.ok_or("app-policy: no policy")?);
                    }
                }
                "whitelist" => {
                    for r in e.children().filter(|r| r.name == "restrict-background") {
                        allowed.push(r.int("uid")?.ok_or("restrict-background: no uid")?);
                    }
                }
                _ => {}
            }
        }
        for uid in allowed {
            let p = policy.uid_policies.get(&uid).copied().unwrap_or(0);
            if p & POLICY_REJECT_METERED_BACKGROUND == 0 && is_app(uid) {
                policy
                    .uid_policies
                    .insert(uid, p | POLICY_ALLOW_METERED_BACKGROUND);
            }
        }
        Ok(policy)
    }

    /// `getRestrictBackgroundStatusInternal`.
    fn status(&self, uid: i32) -> i32 {
        let policy = self.uid_policies.get(&uid).copied().unwrap_or(0);
        if policy == POLICY_REJECT_METERED_BACKGROUND {
            RESTRICT_BACKGROUND_STATUS_ENABLED
        } else if !self.restrict_background {
            RESTRICT_BACKGROUND_STATUS_DISABLED
        } else if policy & POLICY_ALLOW_METERED_BACKGROUND != 0 {
            RESTRICT_BACKGROUND_STATUS_WHITELISTED
        } else {
            RESTRICT_BACKGROUND_STATUS_ENABLED
        }
    }
}

/// `NetworkRequestInfo` of an app's request or listen.
struct Nri {
    /// The request the app holds (`getNetworkRequestForCallback`); a
    /// TRACK_DEFAULT one is satisfied as the copy of the default request
    /// it tracks, whose capabilities are the same.
    request: NetworkRequest,
    messenger: Option<Strong>,
    /// The caller's binder, linked to its death.
    binder: Option<(Strong, u64)>,
    uid: i32,
    as_uid: i32,
    /// Counted against the system's limit.
    system: bool,
    callback_flags: i32,
    declared_methods: i32,
    /// The netId satisfying it.
    satisfier: Option<i32>,
    /// When it is released unless satisfied (`EVENT_TIMEOUT_NETWORK_REQUEST`).
    timeout: Option<Instant>,
}

/// A callback to send: the request's index, `what`, and the netId.
type Callback = (usize, i32, i32);

struct State {
    network: Option<Net>,
    next_net_id: i32,
    next_request_id: i32,
    requests: Vec<Nri>,
    /// `RequestInfoPerUidCounter`s: each uid's requests, by counter.
    counts: HashMap<(bool, i32), i32>,
}

impl State {
    fn new(now: Option<(Uplink, Vec<Ipv4Addr>)>) -> State {
        let mut state = State {
            network: None,
            next_net_id: MIN_NET_ID,
            next_request_id: FIRST_REQUEST_ID,
            requests: Vec::new(),
            counts: HashMap::new(),
        };
        state.follow(now);
        state
    }

    /// `nextNetworkRequestId`.
    fn request_id(&mut self) -> i32 {
        self.next_request_id += 1;
        self.next_request_id - 1
    }

    fn net(&self, network: Option<&Network>) -> Option<&Net> {
        self.network
            .as_ref()
            .filter(|n| network.is_some_and(|w| w.0 == n.id))
    }

    /// Follows the Mac's network `now`: the same network with other DNS
    /// servers has new link properties; another network, or none, loses
    /// the old one, and a new one satisfies what it can.
    fn follow(&mut self, now: Option<(Uplink, Vec<Ipv4Addr>)>) -> Vec<Callback> {
        let mut out = Vec::new();
        if let (Some(net), Some((uplink, dns))) = (&mut self.network, &now)
            && net.uplink == *uplink
        {
            if net.dns != *dns {
                net.dns = dns.clone();
                let id = net.id;
                out.extend(
                    self.requests
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| n.satisfier == Some(id))
                        .map(|(i, _)| (i, CALLBACK_IP_CHANGED, id)),
                );
            }
            return out;
        }
        if let Some(old) = self.network.take() {
            for (i, n) in self.requests.iter_mut().enumerate() {
                if n.satisfier == Some(old.id) {
                    n.satisfier = None;
                    out.push((i, CALLBACK_LOST, old.id));
                }
            }
        }
        if let Some((uplink, dns)) = now {
            self.network = Some(Net {
                id: self.next_net_id,
                uplink,
                dns,
            });
            self.next_net_id += 1;
            out.extend(self.satisfy());
        }
        out
    }

    /// The requests the network newly satisfies (`rematchAllNetworksAndRequests`):
    /// available, and no longer timing out.
    fn satisfy(&mut self) -> Vec<Callback> {
        let Some(id) = self.network.as_ref().map(|n| n.id) else {
            return Vec::new();
        };
        let caps = network_capabilities();
        let mut out = Vec::new();
        for (i, n) in self.requests.iter_mut().enumerate() {
            if n.satisfier.is_none() && n.request.caps.satisfied_by(&caps) {
                n.satisfier = Some(id);
                n.timeout = None;
                out.push((i, CALLBACK_AVAILABLE, id));
            }
        }
        out
    }

    /// `RequestInfoPerUidCounter.incrementCountOrThrow`: a request of
    /// `uid` counted, or refused past the limit.
    fn admit(&mut self, system: bool, uid: i32) -> Result<(), Exception> {
        let max = if system {
            MAX_REQUESTS_PER_SYSTEM_UID
        } else {
            MAX_REQUESTS_PER_UID
        };
        let count = self.counts.entry((system, uid)).or_default();
        if *count + 1 > max {
            return Err(Exception {
                service_specific: TOO_MANY_REQUESTS,
                ..Exception::new(
                    EX_SERVICE_SPECIFIC,
                    format!("Uid {uid} exceeded its allotted requests limit"),
                )
            });
        }
        *count += 1;
        Ok(())
    }

    /// `handleRegisterNetworkRequest` of an admitted request: kept, then
    /// matched.
    fn add(&mut self, nri: Nri) -> Vec<Callback> {
        self.requests.push(nri);
        self.satisfy()
    }

    /// `handleRemoveNetworkRequest` of the request at `at`.
    fn remove(&mut self, at: usize) -> Nri {
        let nri = self.requests.remove(at);
        if let Some(count) = self.counts.get_mut(&(nri.system, nri.uid)) {
            *count -= 1;
        }
        nri
    }

    /// `handleReleaseNetworkRequest`: the request, if `uid` filed it (or
    /// is the system).
    fn release(&mut self, request: &NetworkRequest, uid: i32) -> Option<Nri> {
        let at = self.requests.iter().position(|n| n.request == *request)?;
        if uid != SYSTEM_UID && self.requests[at].uid != uid {
            eprintln!(
                "app host: connectivity: uid {uid} attempted to release an unowned request {}",
                request.request_id
            );
            return None;
        }
        Some(self.remove(at))
    }

    /// The requests whose time ran out unsatisfied
    /// (`handleTimedOutNetworkRequest`), removed.
    fn expire(&mut self, now: Instant) -> Vec<Nri> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.requests.len() {
            if self.requests[i].timeout.is_some_and(|t| t <= now) {
                out.push(self.remove(i));
            } else {
                i += 1;
            }
        }
        out
    }
}

/// The callback `what` for `nri` of netId `net_id` (`callCallbackForRequest`):
/// the `IMessenger.send` of its message, or none when the callback does
/// not declare the method. AVAILABLE carries `caps`, `lp` and the blocked
/// reasons, CAP_CHANGED `caps`, IP_CHANGED `lp`.
fn callback_data(
    nri: &Nri,
    what: i32,
    net_id: i32,
    arg1: i32,
    caps: Option<&NetworkCapabilities>,
    lp: Option<&LinkProperties>,
) -> Option<Parcel> {
    // FEATURE_USE_DECLARED_METHODS_FOR_CALLBACKS (`isCallbackOverridden`).
    if nri.declared_methods & (1 << what) == 0 {
        return None;
    }
    let network = Network(net_id);
    let mut data = vec![(
        "NetworkRequest",
        Value::Parcelable("android.net.NetworkRequest", &nri.request),
    )];
    if what != CALLBACK_UNAVAIL {
        data.push((
            "Network",
            Value::Parcelable("android.net.Network", &network),
        ));
    }
    if let Some(caps) = caps {
        data.push((
            "NetworkCapabilities",
            Value::Parcelable("android.net.NetworkCapabilities", caps),
        ));
    }
    if let Some(lp) = lp {
        data.push((
            "LinkProperties",
            Value::Parcelable("android.net.LinkProperties", lp),
        ));
    }
    if what == CALLBACK_AVAILABLE {
        data.push(("LocalNetworkInfo", Value::Null));
    }
    let mut p = Parcel::new();
    imessenger::Send {
        msg: Some(Message::new(what, arg1, data)),
    }
    .write(&mut p);
    Some(p)
}

pub struct ConnectivityService {
    host: Arc<AppHost>,
    shared: Arc<Shared>,
    /// netd and DnsResolver, once registered, and the network they were
    /// last given.
    native: Mutex<(Option<network_native::Native>, Option<network_native::NativeNetwork>)>,
}

struct Shared {
    state: Mutex<State>,
    /// A request's timeout was set.
    changed: Condvar,
}

/// The Mac's network and DNS servers now.
fn mac_network() -> Option<(Uplink, Vec<Ipv4Addr>)> {
    aim_uplink::uplink().map(|u| (u, aim_uplink::dns_servers()))
}

impl ConnectivityService {
    /// The default network's capabilities as the system's own callers see
    /// them (JobScheduler's `ConnectivityController`); none while the Mac
    /// has no network.
    pub(super) fn default_network(&self) -> Option<NetworkCapabilities> {
        self.shared.state.lock().unwrap().network.as_ref().map(|_| network_capabilities())
    }

    /// Whether netd has registered and been given its interface
    /// (`NetworkManagementService.prepareNativeDaemon` has run).
    pub(super) fn netd_connected(&self) -> bool {
        self.native.lock().unwrap().0.is_some()
    }

    pub fn new(host: Arc<AppHost>) -> ConnectivityService {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::new(mac_network())),
            changed: Condvar::new(),
        });
        let weak = Arc::downgrade(&host);
        let _ = std::thread::Builder::new()
            .name("connectivity-uplink".into())
            .spawn(move || {
                aim_uplink::watch(aim_uplink::routing_socket(), || {
                    if let Some(service) = weak
                        .upgrade()
                        .and_then(|h| h.connectivity_service.get().cloned())
                    {
                        service.follow();
                    }
                })
            });
        let (weak, s) = (Arc::downgrade(&host), shared.clone());
        let _ = std::thread::Builder::new()
            .name("connectivity-timeouts".into())
            .spawn(move || timeouts(weak, s));
        let weak = Arc::downgrade(&host);
        let _ = std::thread::Builder::new().name("connectivity-netd".into()).spawn(move || {
            // netd registers itself and the DnsResolver it hosts once the
            // BPF programs are loaded.
            let service = |name: &str| loop {
                let host = weak.upgrade()?;
                if let Ok(s) = super::display::check_service(&host.process, name) {
                    return Some(s);
                }
                drop(host);
                std::thread::sleep(Duration::from_millis(200));
            };
            let (Some(netd), Some(resolver)) = (service("netd"), service("dnsresolver")) else { return };
            let Some(cs) = weak.upgrade().and_then(|h| h.connectivity_service.get().cloned()) else { return };
            let native = network_native::Native { netd, resolver };
            if let Err(e) = native.set_interface_up(INTERFACE) {
                eprintln!("app host: connectivity: netd: {e}");
            }
            cs.native.lock().unwrap().0 = Some(native);
            // The settings the network's description reads are loaded once
            // the guest's properties are set.
            while cs.host.inputs().is_err() {
                std::thread::sleep(Duration::from_millis(200));
            }
            cs.sync_native();
        });
        ConnectivityService { host, shared, native: Mutex::new((None, None)) }
    }

    /// Follows the Mac's network.
    fn follow(&self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            let callbacks = state.follow(mac_network());
            self.deliver(&state, &callbacks);
        }
        self.sync_native();
    }

    /// Gives netd and DnsResolver the network as ConnectivityService gives
    /// it them: a new one made and made the default, a lost one destroyed,
    /// new DNS servers or private DNS sent to the resolver, each followed
    /// by `flushVmDnsCache`.
    fn sync_native(&self) {
        if self.native.lock().unwrap().0.is_none() {
            return;
        }
        let wanted = {
            let state = self.shared.state.lock().unwrap();
            state.network.as_ref().map(|net| self.native_network(net))
        };
        let wanted = match wanted.transpose() {
            Ok(wanted) => wanted,
            Err(e) => return eprintln!("app host: connectivity: {e:?}"),
        };
        let mut native = self.native.lock().unwrap();
        let (Some(services), configured) = &mut *native else { return };
        if *configured == wanted {
            return;
        }
        let result = match (configured.as_ref(), wanted.as_ref()) {
            (Some(old), Some(new)) if old.net_id == new.net_id => services.configure_dns(new),
            (old, new) => old
                .map_or(Ok(()), |old| services.destroy(old))
                .and_then(|()| new.map_or(Ok(()), |new| services.create(new))),
        };
        match result {
            Ok(()) => {
                let flush = wanted.is_some();
                *configured = wanted;
                drop(native);
                if flush {
                    self.flush_vm_dns_cache();
                }
            }
            Err(e) => eprintln!("app host: connectivity: netd: {e}"),
        }
    }

    /// The network as netd and DnsResolver are to know it.
    fn native_network(&self, net: &Net) -> Result<network_native::NativeNetwork, Exception> {
        let lp = self.link_properties(net)?;
        let settings = &self.host.inputs()?.settings;
        let global = |name: &str| settings.get(Table::Global, name).filter(|v| !v.is_empty());
        // `getPrivateDnsMode`: off, opportunistic (the default) or strict.
        let private_dns = match global("private_dns_mode").or_else(|| global("private_dns_default_mode")).as_deref() {
            Some("off") => None,
            Some("hostname") => Some(global("private_dns_specifier")),
            _ => Some(None),
        };
        Ok(network_native::NativeNetwork {
            net_id: net.id,
            interface: INTERFACE.into(),
            address: (net.uplink.addr, net.uplink.prefix),
            routes: lp.routes,
            dns: net.dns.clone(),
            private_dns,
            transports: vec![TRANSPORT_ETHERNET as i32],
            settings: network_native::ResolverSettings::read(|name| global(name).and_then(|v| v.parse().ok())),
        })
    }

    /// `DnsManager.flushVmDnsCache`: apps drop the names they resolved.
    fn flush_vm_dns_cache(&self) {
        const FLAG_RECEIVER_REPLACE_PENDING: i32 = 0x2000_0000;
        const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
        let intent = super::kept::RawIntent::new(
            "android.intent.action.CLEAR_DNS_CACHE",
            FLAG_RECEIVER_REPLACE_PENDING | FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT,
            &[],
        );
        self.host.broadcasts.send_from_system(&self.host, intent);
    }

    fn has(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `hasAnyPermissionOf`.
    fn has_any(&self, uid: i32, permissions: &[&str]) -> Result<bool, Exception> {
        for p in permissions {
            if self.has(uid, p)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `PermissionUtils.enforceAnyPermissionOf`.
    fn enforce_any(&self, uid: i32, permissions: &[&str]) -> Result<(), Exception> {
        if self.has_any(uid, permissions)? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Requires one of the following permissions: {}.",
            permissions.join(", ")
        )))
    }

    /// `enforceAccessPermission`.
    fn enforce_access(&self, uid: i32) -> Result<(), Exception> {
        if self.has(uid, ACCESS_NETWORK_STATE)? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "ConnectivityService: Neither user {uid} nor current process has {ACCESS_NETWORK_STATE}."
        )))
    }

    /// `AppOpsManager.checkPackage`.
    fn check_package(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        match package {
            Some(p) if self.host.owns_package(uid, p)? => Ok(()),
            _ => Err(Exception::security(format!(
                "Package {} does not belong to {uid}",
                package.unwrap_or("null")
            ))),
        }
    }

    /// `enforceChangePermission`: CHANGE_NETWORK_STATE, or WRITE_SETTINGS
    /// by its app op.
    fn enforce_change(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        if self.has(uid, CHANGE_NETWORK_STATE)? {
            return Ok(());
        }
        let Some(package) = package else {
            return Err(Exception::security("Calling package name is null."));
        };
        match appops_service::note(&self.host, OP_WRITE_SETTINGS, uid, Some(package))? {
            0 => return Ok(()),
            MODE_DEFAULT if self.has(uid, WRITE_SETTINGS)? => return Ok(()),
            _ => {}
        }
        Err(Exception::security(format!(
            "{package} was not granted either of these permissions:{CHANGE_NETWORK_STATE},{WRITE_SETTINGS}."
        )))
    }

    /// `ConnectivitySettingsManager.getUidsAllowedOnRestrictedNetworks`.
    fn allowed_on_restricted_networks(&self, uid: i32) -> Result<bool, Exception> {
        let settings = &self.host.inputs()?.settings;
        Ok(settings
            .get(Table::Global, "uids_allowed_on_restricted_networks")
            .is_some_and(|list| list.split(';').any(|u| u.parse() == Ok(uid))))
    }

    /// `PermissionMonitor.hasUseBackgroundNetworksPermission`: a network
    /// or system permission for the uid (vendor apps' carryover, for
    /// targets before Q, is not modelled: #1509).
    fn uses_background_networks(&self, uid: i32) -> Result<bool, Exception> {
        Ok(self.has_any(
            uid,
            &[
                CHANGE_NETWORK_STATE,
                MAINLINE_NETWORK_STACK,
                NETWORK_STACK,
                CONNECTIVITY_USE_RESTRICTED_NETWORKS,
            ],
        )? || self.allowed_on_restricted_networks(uid)?)
    }

    /// `isNetworkWithCapabilitiesBlocked`: the app host's apps are all top,
    /// so no firewall chain or data saver blocks them; one without INTERNET
    /// is blocked once its target has the change.
    fn blocked(&self, uid: i32, ignore_blocked: bool) -> Result<bool, Exception> {
        if ignore_blocked || self.has(uid, INTERNET_PERMISSION)? {
            return Ok(false);
        }
        self.host
            .change_enabled(NETWORK_BLOCKED_WITHOUT_INTERNET_PERMISSION, uid)
    }

    /// `getBlockedState` of the network for `uid`: no blocked reasons
    /// (as `blocked`) but a missing INTERNET permission's.
    fn blocked_reasons(&self, uid: i32) -> Result<i32, Exception> {
        Ok(if self.blocked(uid, false)? {
            BLOCKED_REASON_NETWORK_RESTRICTED
        } else {
            0
        })
    }

    /// `getFilteredNetworkInfo` of the network for `uid`, as `network_type`.
    fn filtered_info(
        &self,
        network_type: i32,
        uid: i32,
        ignore_blocked: bool,
    ) -> Result<NetworkInfo, Exception> {
        let mut info = network_info();
        info.network_type = network_type;
        if self.blocked(uid, ignore_blocked)? {
            info.block();
        }
        Ok(info)
    }

    fn has_feature(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .system
            .features
            .iter()
            .any(|(n, v)| n == name && *v >= 0))
    }

    fn property(&self, name: &str) -> Option<String> {
        (self.host.configured.get()?.properties)(name)
    }

    /// `LegacyTypeTracker.isTypeSupported` (`loadSupportedTypes`).
    fn type_supported(&self, network_type: i32) -> Result<bool, Exception> {
        Ok(match network_type {
            1 => self.has_feature("android.hardware.wifi")?,
            13 => self.has_feature("android.hardware.wifi.direct")?,
            0 | 2..=5 | 10..=12 | 14 | 15 => self.data_capable()?,
            7 => self.has_feature("android.hardware.bluetooth")?,
            16 => self.has_feature("android.hardware.type.watch")?,
            TYPE_ETHERNET => {
                self.has_feature("android.hardware.ethernet")?
                    || self.has_feature("android.hardware.usb.host")?
            }
            17 => true,
            _ => false,
        })
    }

    /// `TelephonyManager.isDataCapable`: the feature, or on a vendor
    /// before 2024 Q2 `config_mobile_data_capable`.
    fn data_capable(&self) -> Result<bool, Exception> {
        if self.has_feature("android.hardware.telephony.data")? {
            return Ok(true);
        }
        let first = self
            .property("ro.product.first_api_level")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let vendor = self
            .property("ro.vendor.api_level")
            .and_then(|v| v.parse().ok())
            .unwrap_or(first);
        Ok(vendor < VENDOR_API_2024_Q2
            && self
                .host
                .inputs()?
                .packages
                .framework_bool("config_mobile_data_capable")
                .unwrap_or(true))
    }

    /// `getNetworkInfo(type)` for `uid`: the network's for Ethernet, else
    /// `makeFakeNetworkInfo` for a supported type.
    fn network_info_for_type(
        &self,
        network_type: i32,
        uid: i32,
    ) -> Result<Option<NetworkInfo>, Exception> {
        if !(0..=MAX_NETWORK_TYPE).contains(&network_type) || !self.type_supported(network_type)? {
            return Ok(None);
        }
        let connected = self.shared.state.lock().unwrap().network.is_some();
        if network_type == TYPE_ETHERNET && connected {
            return self.filtered_info(network_type, uid, false).map(Some);
        }
        let mut info = NetworkInfo {
            network_type,
            subtype: 0,
            type_name: TYPE_NAMES[network_type as usize].into(),
            subtype_name: String::new(),
            state: "DISCONNECTED",
            detailed_state: "DISCONNECTED",
            failover: false,
            available: true,
            roaming: false,
            reason: None,
            extra_info: None,
        };
        if self.blocked(uid, false)? {
            info.block();
        }
        Ok(Some(info))
    }

    /// The network's link properties, private DNS as `DnsManager` reads
    /// its mode (`ConnectivitySettingsUtils.getPrivateDnsMode`).
    fn link_properties(&self, net: &Net) -> Result<LinkProperties, Exception> {
        let settings = &self.host.inputs()?.settings;
        let global = |name| settings.get(Table::Global, name).filter(|v| !v.is_empty());
        let mode = global("private_dns_mode").or_else(|| global("private_dns_default_mode"));
        let strict =
            (mode.as_deref() == Some("hostname")).then(|| global("private_dns_specifier"));
        Ok(link_properties(&net.uplink, &net.dns, strict))
    }

    /// The network's capabilities as `uid` sees them.
    fn capabilities_for(
        &self,
        uid: i32,
        include_location: bool,
    ) -> Result<NetworkCapabilities, Exception> {
        let held = CallerPermissions {
            settings: self.has_any(uid, &[NETWORK_SETTINGS, MAINLINE_NETWORK_STACK])?,
            network_stack: self.has_any(uid, &[NETWORK_STACK, MAINLINE_NETWORK_STACK])?,
            factory: self.has(uid, NETWORK_FACTORY)?,
            factory_or_settings: self.has_any(
                uid,
                &[NETWORK_FACTORY, NETWORK_SETTINGS, MAINLINE_NETWORK_STACK],
            )? || uid.rem_euclid(PER_USER_RANGE as i32) == BLUETOOTH_UID,
        };
        network_capabilities().redacted_for(uid, &held, || {
            Err(self.host.unimplemented_named(
                "connectivity",
                &format!("a network owner's location check (include location: {include_location}) (#1509)"),
            ))
        })
    }

    /// Sends `callbacks` (`callCallbackForRequest`).
    fn deliver(&self, state: &State, callbacks: &[Callback]) {
        for &(at, what, net_id) in callbacks {
            if let Err(e) = self.callback(state, &state.requests[at], what, net_id) {
                eprintln!("app host: connectivity: a callback: {}", e.message);
            }
        }
    }

    fn callback(&self, state: &State, nri: &Nri, what: i32, net_id: i32) -> Result<(), Exception> {
        let Some(messenger) = &nri.messenger else {
            return Ok(());
        };
        let net = state.network.as_ref().filter(|n| n.id == net_id);
        let include_location = nri.callback_flags & FLAG_INCLUDE_LOCATION_INFO != 0;
        let caps = match (what, net) {
            (CALLBACK_AVAILABLE | CALLBACK_CAP_CHANGED, Some(_)) => {
                Some(self.capabilities_for(nri.uid, include_location)?)
            }
            _ => None,
        };
        // No captive portal data to sanitize (`linkPropertiesRestrictedForCallerPermissions`).
        let lp = match (what, net) {
            (CALLBACK_AVAILABLE | CALLBACK_IP_CHANGED, Some(net)) => {
                Some(self.link_properties(net)?)
            }
            _ => None,
        };
        let arg1 = if what == CALLBACK_AVAILABLE {
            self.blocked_reasons(nri.as_uid)?
        } else {
            0
        };
        if let Some(data) = callback_data(nri, what, net_id, arg1, caps.as_ref(), lp.as_ref())
            && let Err(s) = messenger.transact(imessenger::SEND, &data, true)
        {
            // May occur naturally in the race of binder death.
            eprintln!(
                "app host: connectivity: a callback for request {}: status {s}",
                nri.request.request_id
            );
        }
        Ok(())
    }

    /// `restrictBackgroundRequestForCaller`.
    fn restrict_background(&self, nc: &mut NetworkCapabilities, uid: i32) -> Result<(), Exception> {
        if !self.uses_background_networks(uid)? {
            nc.add(FOREGROUND);
        }
        Ok(())
    }

    /// `maybeDisableLocalNetworkMatching`.
    fn maybe_disable_local_network_matching(
        &self,
        nc: &mut NetworkCapabilities,
        uid: i32,
    ) -> Result<(), Exception> {
        nc.match_non_thread_local = self
            .host
            .change_enabled(ENABLE_MATCH_NON_THREAD_LOCAL_NETWORKS, uid)?;
        if !self.host.change_enabled(ENABLE_MATCH_LOCAL_NETWORK, uid)? && !nc.has(LOCAL_NETWORK) {
            nc.forbid(LOCAL_NETWORK);
        }
        Ok(())
    }

    /// `ensureSufficientPermissionsForRequest`.
    fn ensure_permissions_for(
        &self,
        nc: &NetworkCapabilities,
        uid: i32,
        package: Option<&str>,
    ) -> Result<(), Exception> {
        if nc.ssid.is_some() && !self.has_any(uid, &[NETWORK_SETTINGS, MAINLINE_NETWORK_STACK])? {
            return Err(Exception::security(
                "Insufficient permissions to request a specific SSID",
            ));
        }
        if nc.has_signal_strength()
            && !self.has_any(
                uid,
                &[
                    NETWORK_SIGNAL_STRENGTH_WAKEUP,
                    MAINLINE_NETWORK_STACK,
                    NETWORK_SETTINGS,
                ],
            )?
        {
            return Err(Exception::security(
                "Insufficient permissions to request a specific signal strength",
            ));
        }
        self.check_package(uid, package)
    }

    /// `ApplicationInfo.targetSdkVersion` of `package` at least `version`
    /// (`isTargetSdkAtleast`), true for a package not installed.
    fn target_at_least(&self, version: i32, package: Option<&str>) -> Result<bool, Exception> {
        let inputs = self.host.inputs()?;
        Ok(package
            .and_then(|p| inputs.packages.application_info(p, 0))
            .is_none_or(|ai| ai.target_sdk_version >= version))
    }

    /// `enforceNetworkRequestPermissions`. Carrier privileges, which may
    /// request restricted networks, need telephony, which the app host
    /// does not have.
    fn enforce_request_permissions(
        &self,
        nc: &NetworkCapabilities,
        uid: i32,
        package: Option<&str>,
    ) -> Result<(), Exception> {
        if nc.has(PRIORITIZE_LATENCY) || nc.has(PRIORITIZE_BANDWIDTH) {
            let inputs = self.host.inputs()?;
            let enabled = match package.and_then(|p| inputs.packages.application_info(p, 0)) {
                Some(ai) => inputs.compat.is_enabled(
                    ENABLE_SELF_CERTIFIED_CAPABILITIES_DECLARATION,
                    package,
                    ai.target_sdk_version,
                ),
                None => inputs
                    .compat
                    .default_value(ENABLE_SELF_CERTIFIED_CAPABILITIES_DECLARATION),
            };
            if enabled {
                return Err(self.host.unimplemented_named(
                    "connectivity",
                    "self-certified network capabilities (#1509)",
                ));
            }
        }
        if nc.has(NOT_RESTRICTED) {
            return self.enforce_change(uid, package);
        }
        if self.has_any(
            uid,
            &[CONNECTIVITY_USE_RESTRICTED_NETWORKS, CONNECTIVITY_INTERNAL],
        )? || self.allowed_on_restricted_networks(uid)?
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "ConnectivityService: user {uid} has no permission to access restricted network."
        )))
    }

    /// Links a request's binder to its death, which releases it.
    fn link(&self, binder: Option<Binder>, request_id: i32) -> Option<(Strong, u64)> {
        let Some(Binder::Handle(handle)) = binder else {
            return None;
        };
        let binder = self.host.process.strong(handle);
        let weak = Arc::downgrade(&self.host);
        let death = self.host.process.link_to_death(
            &binder,
            Box::new(move || {
                if let Some(service) = weak
                    .upgrade()
                    .and_then(|h| h.connectivity_service.get().cloned())
                {
                    service.binder_died(request_id);
                }
            }),
        );
        Some((binder, death))
    }

    /// `NetworkRequestInfo.binderDied`: released, without a callback.
    fn binder_died(&self, request_id: i32) {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(at) = state
            .requests
            .iter()
            .position(|n| n.request.request_id == request_id)
        {
            state.remove(at);
        }
    }

    /// Unlinks a request no longer kept.
    fn unlink(&self, nri: &Nri) {
        if let Some((binder, death)) = &nri.binder {
            self.host.process.clear_death(binder, *death);
        }
    }

    /// Registers `request` filed by `uid` (`trackUidAndRegisterNetworkRequest`)
    /// and sends what the network satisfies.
    #[allow(clippy::too_many_arguments)]
    fn register(
        &self,
        state: &mut State,
        request: NetworkRequest,
        uid: i32,
        as_uid: i32,
        messenger: Option<Messenger>,
        binder: Option<Binder>,
        callback_flags: i32,
        declared_methods: i32,
        timeout_ms: i32,
    ) -> Result<(), Exception> {
        let system = self.has(uid, MAINLINE_NETWORK_STACK)?;
        state.admit(system, uid)?;
        let messenger = match messenger.and_then(|m| m.0) {
            Some(Binder::Handle(h)) => Some(self.host.process.strong(h)),
            _ => None,
        };
        let nri = Nri {
            binder: self.link(binder, request.request_id),
            request,
            messenger,
            uid,
            as_uid,
            system,
            callback_flags,
            declared_methods,
            satisfier: None,
            timeout: (timeout_ms > 0)
                .then(|| Instant::now() + Duration::from_millis(timeout_ms as u64)),
        };
        let callbacks = state.add(nri);
        self.deliver(state, &callbacks);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// `requestNetwork`.
    fn request_network(
        &self,
        call: &Call<'_>,
        a: cm::RequestNetwork<NetworkCapabilities, Messenger>,
    ) -> Result<NetworkRequest, Exception> {
        let uid = call.sender_euid as i32;
        let declared = if a.declared_methods_flag == 0 {
            DECLARED_METHODS_ALL
        } else {
            a.declared_methods_flag
        };
        let package = a.calling_package_name;
        if a.legacy != TYPE_NONE
            && !self.has_any(uid, &[NETWORK_STACK, MAINLINE_NETWORK_STACK])?
            && self.target_at_least(M, package.as_deref())?
        {
            return Err(Exception::security(
                "Insufficient permissions to specify legacy type",
            ));
        }
        let as_uid = if a.uid != INVALID_UID {
            self.enforce_any(uid, &[NETWORK_SETTINGS, MAINLINE_NETWORK_STACK])?;
            a.uid
        } else {
            uid
        };
        let kind = RequestType::from_ordinal(a.req_type).ok_or_else(|| {
            Exception::illegal_argument(format!("Unsupported request type {}", a.req_type))
        })?;
        let settings = self.has_any(uid, &[NETWORK_SETTINGS, MAINLINE_NETWORK_STACK])?;
        let mut nc = match kind {
            RequestType::TrackDefault => {
                let nc = default_capabilities_for_uid(as_uid, uid, package.clone(), settings);
                self.enforce_access(uid)?;
                nc
            }
            RequestType::TrackSystemDefault => {
                self.enforce_any(
                    uid,
                    &[
                        NETWORK_SETTINGS,
                        NETWORK_SETUP_WIZARD,
                        MAINLINE_NETWORK_STACK,
                        CONNECTIVITY_USE_RESTRICTED_NETWORKS,
                    ],
                )?;
                default_request_capabilities()
            }
            RequestType::BackgroundRequest | RequestType::Request => {
                if kind == RequestType::BackgroundRequest {
                    self.enforce_any(
                        uid,
                        &[NETWORK_SETTINGS, NETWORK_STACK, MAINLINE_NETWORK_STACK],
                    )?;
                }
                let mut nc = a
                    .network_capabilities
                    .unwrap_or_else(NetworkCapabilities::cleared);
                self.enforce_request_permissions(&nc, uid, package.as_deref())?;
                // `enforceMeteredApnPolicy`: the app host's apps are all
                // top, so none is restricted on metered networks.
                self.maybe_disable_local_network_matching(&mut nc, uid)?;
                nc
            }
            RequestType::ListenForBest => {
                self.enforce_access(uid)?;
                let mut nc = a
                    .network_capabilities
                    .unwrap_or_else(NetworkCapabilities::cleared);
                self.maybe_disable_local_network_matching(&mut nc, uid)?;
                nc
            }
            RequestType::Reservation => {
                return Err(self.host.unimplemented_named(
                    "connectivity",
                    "requestNetwork of a reservation (#1509)",
                ));
            }
            RequestType::None | RequestType::Listen => {
                return Err(Exception::illegal_argument(format!(
                    "Unsupported request type {}",
                    kind.name()
                )));
            }
        };
        ensure_capabilities(&nc, true)?;
        self.ensure_permissions_for(&nc, uid, package.as_deref())?;
        if kind == RequestType::ListenForBest {
            self.restrict_background(&mut nc, uid)?;
        }
        restrict_uids_and_set_requestor(&mut nc, uid, package, settings);
        if a.timeout_sec < 0 {
            return Err(Exception::illegal_argument("Bad timeout specified"));
        }
        let mut state = self.shared.state.lock().unwrap();
        let request = NetworkRequest {
            caps: nc,
            legacy_type: a.legacy,
            request_id: state.request_id(),
            kind,
        };
        if kind == RequestType::TrackDefault {
            // The default request's copy for the uid it tracks
            // (`copyDefaultNetworkRequestsForUid`).
            state.request_id();
        }
        self.register(
            &mut state,
            request.clone(),
            uid,
            as_uid,
            a.messenger,
            a.binder,
            a.callback_flags,
            declared,
            a.timeout_sec,
        )?;
        Ok(request)
    }

    /// `listenForNetwork`.
    fn listen_for_network(
        &self,
        call: &Call<'_>,
        a: cm::ListenForNetwork<NetworkCapabilities, Messenger>,
    ) -> Result<NetworkRequest, Exception> {
        let uid = call.sender_euid as i32;
        let declared = if a.declared_methods_flag == 0 {
            DECLARED_METHODS_ALL
        } else {
            a.declared_methods_flag
        };
        let package = a.calling_package_name;
        // `hasWifiNetworkListenPermission`: a listen for Wi-Fi alone by an
        // app with ACCESS_WIFI_STATE.
        let wifi_only = a
            .network_capabilities
            .as_ref()
            .is_some_and(|nc| nc.transports == 1 << TRANSPORT_WIFI);
        if !(wifi_only && self.has(uid, ACCESS_WIFI_STATE)?) {
            self.enforce_access(uid)?;
        }
        let Some(mut nc) = a.network_capabilities else {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "networkCapabilities is null",
            ));
        };
        self.ensure_permissions_for(&nc, uid, package.as_deref())?;
        let settings = self.has_any(uid, &[NETWORK_SETTINGS, MAINLINE_NETWORK_STACK])?;
        restrict_uids_and_set_requestor(&mut nc, uid, package, settings);
        self.maybe_disable_local_network_matching(&mut nc, uid)?;
        self.restrict_background(&mut nc, uid)?;
        ensure_capabilities(&nc, false)?;
        let mut state = self.shared.state.lock().unwrap();
        let request = NetworkRequest {
            caps: nc,
            legacy_type: TYPE_NONE,
            request_id: state.request_id(),
            kind: RequestType::Listen,
        };
        self.register(
            &mut state,
            request.clone(),
            uid,
            uid,
            a.messenger,
            a.binder,
            a.callback_flags,
            declared,
            0,
        )?;
        Ok(request)
    }

    /// `releaseNetworkRequest`.
    fn release_network_request(
        &self,
        call: &Call<'_>,
        request: Option<NetworkRequest>,
    ) -> Result<(), Exception> {
        let request =
            request.ok_or_else(|| Exception::new(EX_NULL_POINTER, "networkRequest is null"))?;
        if request.kind == RequestType::None {
            return Err(Exception::illegal_argument(
                "All NetworkRequests in ConnectivityService must have a type",
            ));
        }
        let released = self
            .shared
            .state
            .lock()
            .unwrap()
            .release(&request, call.sender_euid as i32);
        if let Some(nri) = released {
            self.unlink(&nri);
        }
        Ok(())
    }

    /// The netId of `uid`'s active network (`getActiveNetworkForUidInternal`):
    /// the network, unless it is blocked for the uid.
    fn active_network(&self, uid: i32, ignore_blocked: bool) -> Result<Option<Network>, Exception> {
        let id = self
            .shared
            .state
            .lock()
            .unwrap()
            .network
            .as_ref()
            .map(|n| n.id);
        match id {
            Some(id) if !self.blocked(uid, ignore_blocked)? => Ok(Some(Network(id))),
            _ => Ok(None),
        }
    }

    /// `ProxyTracker`'s global proxy: none unless the settings name one.
    fn global_proxy(&self) -> Result<(), Exception> {
        let settings = &self.host.inputs()?.settings;
        let set = |name| {
            settings
                .get(Table::Global, name)
                .is_some_and(|v| !v.is_empty())
        };
        if set("global_http_proxy_host") || set("global_proxy_pac_url") {
            return Err(self
                .host
                .unimplemented_named("connectivity", "a global proxy (#1509)"));
        }
        Ok(())
    }

    /// NetworkPolicyManagerService's data saver state, as it persisted it;
    /// a file it cannot read leaves the defaults, as `readPolicyAL` does.
    fn net_policy(&self) -> Result<NetPolicy, Exception> {
        let path = self.host.data()?.join("system/netpolicy.xml");
        let bytes = std::fs::read(&path).ok();
        let default = self
            .host
            .inputs()?
            .settings
            .get(Table::Global, "default_restrict_background_data")
            .as_deref()
            == Some("1");
        Ok(NetPolicy::read(bytes.as_deref(), default).unwrap_or_else(|e| {
            eprintln!("app host: connectivity: problem reading {}: {e}", path.display());
            NetPolicy::default()
        }))
    }

    /// `getEnabledConnectivityManagerFeatures`: each feature not switched
    /// off by its tethering flag (`isTetheringFeatureNotChickenedOut`).
    fn features(&self) -> Result<i64, Exception> {
        let settings = &self.host.inputs()?.settings;
        let mut features = 0;
        for &(feature, flag) in FEATURES {
            match settings
                .device_config("tethering", flag)
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(0)
            {
                0 | 1 => features |= feature,
                -1 => {}
                _ => {
                    return Err(self.host.unimplemented_named(
                        "connectivity",
                        &format!("{flag} by module version (#1509)"),
                    ));
                }
            }
        }
        Ok(features)
    }
}

/// Releases requests whose time runs out unsatisfied, with
/// `CALLBACK_UNAVAIL`.
fn timeouts(host: Weak<AppHost>, shared: Arc<Shared>) {
    loop {
        let mut state = shared.state.lock().unwrap();
        let now = Instant::now();
        let next = state.requests.iter().filter_map(|n| n.timeout).min();
        match next {
            Some(t) if t <= now => {}
            Some(t) => {
                drop(shared.changed.wait_timeout(state, t - now).unwrap());
                continue;
            }
            None => {
                drop(shared.changed.wait(state).unwrap());
                continue;
            }
        }
        let Some(service) = host
            .upgrade()
            .and_then(|h| h.connectivity_service.get().cloned())
        else {
            return;
        };
        for nri in state.expire(now) {
            service.unlink(&nri);
            if let Err(e) = service.callback(&state, &nri, CALLBACK_UNAVAIL, 0) {
                eprintln!("app host: connectivity: a timed out request: {}", e.message);
            }
        }
    }
}

impl Service for ConnectivityService {
    fn descriptor(&self) -> &str {
        cm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let bad = |_| BAD_VALUE;
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            cm::GET_ACTIVE_NETWORK => {
                cm::GetActiveNetwork::read(&mut call.data).map_err(bad)?;
                self.enforce_access(uid)
                    .and_then(|()| self.active_network(uid, false))
                    .map(|n| cm::write_get_active_network_reply(&mut p, n.as_ref()))
            }
            cm::GET_ACTIVE_NETWORK_FOR_UID => {
                let a = cm::GetActiveNetworkForUid::read(&mut call.data).map_err(bad)?;
                self.enforce_any(uid, &[NETWORK_STACK, MAINLINE_NETWORK_STACK])
                    .and_then(|()| self.active_network(a.uid, a.ignore_blocked))
                    .map(|n| cm::write_get_active_network_for_uid_reply(&mut p, n.as_ref()))
            }
            cm::GET_ACTIVE_NETWORK_INFO => {
                cm::GetActiveNetworkInfo::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_access(uid)?;
                    let connected = self.shared.state.lock().unwrap().network.is_some();
                    let info = if connected {
                        Some(self.filtered_info(TYPE_ETHERNET, uid, false)?)
                    } else {
                        None
                    };
                    cm::write_get_active_network_info_reply(&mut p, info.as_ref());
                    Ok(())
                })()
            }
            cm::GET_ACTIVE_NETWORK_INFO_FOR_UID => {
                let a = cm::GetActiveNetworkInfoForUid::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_any(uid, &[NETWORK_STACK, MAINLINE_NETWORK_STACK])?;
                    let connected = self.shared.state.lock().unwrap().network.is_some();
                    let info = if connected {
                        Some(self.filtered_info(TYPE_ETHERNET, a.uid, a.ignore_blocked)?)
                    } else {
                        None
                    };
                    cm::write_get_active_network_info_for_uid_reply(&mut p, info.as_ref());
                    Ok(())
                })()
            }
            cm::GET_NETWORK_INFO => {
                let a = cm::GetNetworkInfo::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_access(uid)?;
                    let info = self.network_info_for_type(a.network_type, uid)?;
                    cm::write_get_network_info_reply(&mut p, info.as_ref());
                    Ok(())
                })()
            }
            cm::GET_NETWORK_INFO_FOR_UID => {
                let a = cm::GetNetworkInfoForUid::<Network>::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_access(uid)?;
                    let known = self
                        .shared
                        .state
                        .lock()
                        .unwrap()
                        .net(a.network.as_ref())
                        .is_some();
                    let info = if known {
                        Some(self.filtered_info(TYPE_ETHERNET, a.uid, a.ignore_blocked)?)
                    } else {
                        None
                    };
                    cm::write_get_network_info_for_uid_reply(&mut p, info.as_ref());
                    Ok(())
                })()
            }
            cm::GET_ALL_NETWORKS => {
                cm::GetAllNetworks::read(&mut call.data).map_err(bad)?;
                self.enforce_access(uid).map(|()| {
                    let networks: Vec<_> = self
                        .shared
                        .state
                        .lock()
                        .unwrap()
                        .network
                        .iter()
                        .map(|n| Some(Network(n.id)))
                        .collect();
                    cm::write_get_all_networks_reply(&mut p, Some(&networks));
                })
            }
            cm::GET_LINK_PROPERTIES => {
                let a = cm::GetLinkProperties::<Network>::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_access(uid)?;
                    let state = self.shared.state.lock().unwrap();
                    let lp = state
                        .net(a.network.as_ref())
                        .map(|n| self.link_properties(n))
                        .transpose()?;
                    cm::write_get_link_properties_reply(&mut p, lp.as_ref());
                    Ok(())
                })()
            }
            cm::GET_NETWORK_CAPABILITIES => {
                let a = cm::GetNetworkCapabilities::<Network>::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.check_package(uid, a.calling_package_name.as_deref())?;
                    self.enforce_access(uid)?;
                    let known = self
                        .shared
                        .state
                        .lock()
                        .unwrap()
                        .net(a.network.as_ref())
                        .is_some();
                    let caps = if known {
                        Some(self.capabilities_for(uid, false)?)
                    } else {
                        None
                    };
                    cm::write_get_network_capabilities_reply(&mut p, caps.as_ref());
                    Ok(())
                })()
            }
            // `LegacyNetworkActivityTracker`: the default network, Ethernet,
            // gets no idle timer (`setupDataActivityTracking` tracks only
            // cellular and Wi-Fi), so it stays active.
            cm::IS_DEFAULT_NETWORK_ACTIVE => {
                cm::IsDefaultNetworkActive::read(&mut call.data).map_err(bad)?;
                cm::write_is_default_network_active_reply(&mut p, true);
                Ok(())
            }
            // `reportNetworkConnectivity`: a report that agrees with the
            // network's validation only goes to the connectivity diagnostics
            // callbacks, which none registers; one that disagrees revalidates
            // the network, which is not implemented.
            cm::REPORT_NETWORK_CONNECTIVITY => {
                let a = cm::ReportNetworkConnectivity::<Network>::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_access(uid)?;
                    if !self.has(uid, INTERNET_PERMISSION)? {
                        return Err(Exception::security(format!(
                            "ConnectivityService: Neither user {uid} nor current process has {INTERNET_PERMISSION}."
                        )));
                    }
                    let known = match a.network.as_ref() {
                        None => self.default_network().is_some(),
                        Some(_) => self.shared.state.lock().unwrap().net(a.network.as_ref()).is_some(),
                    };
                    if known && !a.has_connectivity {
                        return Err(self.host.unimplemented_named("connectivity", "revalidating a network an app reports without connectivity (#1509)"));
                    }
                    cm::write_report_network_connectivity_reply(&mut p);
                    Ok(())
                })()
            }
            cm::IS_ACTIVE_NETWORK_METERED => {
                cm::IsActiveNetworkMetered::read(&mut call.data).map_err(bad)?;
                self.enforce_access(uid)
                    .and_then(|()| self.active_network(uid, false))
                    .map(|n| {
                        // The network is not metered; without one, the most
                        // conservative answer.
                        cm::write_is_active_network_metered_reply(
                            &mut p,
                            n.is_none() || network_capabilities().is_metered(),
                        )
                    })
            }
            cm::GET_GLOBAL_PROXY => {
                cm::GetGlobalProxy::read(&mut call.data).map_err(bad)?;
                self.global_proxy()
                    .map(|()| cm::write_get_global_proxy_reply::<network::Network>(&mut p, None))
            }
            cm::GET_PROXY_FOR_NETWORK => {
                cm::GetProxyForNetwork::<Network>::read(&mut call.data).map_err(bad)?;
                // The network has no HTTP proxy.
                self.global_proxy().map(|()| {
                    cm::write_get_proxy_for_network_reply::<network::Network>(&mut p, None)
                })
            }
            cm::GET_MULTIPATH_PREFERENCE => {
                let a = cm::GetMultipathPreference::<Network>::read(&mut call.data).map_err(bad)?;
                (|| {
                    self.enforce_access(uid)?;
                    let known = self
                        .shared
                        .state
                        .lock()
                        .unwrap()
                        .net(a.network.as_ref())
                        .is_some();
                    if !(known && network_capabilities().has(NOT_METERED)) {
                        return Err(self.host.unimplemented_named(
                            "connectivity",
                            "getMultipathPreference of a metered network (#1509)",
                        ));
                    }
                    cm::write_get_multipath_preference_reply(
                        &mut p,
                        MULTIPATH_PREFERENCE_UNMETERED,
                    );
                    Ok(())
                })()
            }
            cm::GET_RESTRICT_BACKGROUND_STATUS_BY_CALLER => {
                cm::GetRestrictBackgroundStatusByCaller::read(&mut call.data).map_err(bad)?;
                self.enforce_access(uid)
                    .and_then(|()| self.net_policy())
                    .map(|policy| {
                        cm::write_get_restrict_background_status_by_caller_reply(
                            &mut p,
                            policy.status(uid),
                        )
                    })
            }
            cm::GET_ENABLED_CONNECTIVITY_MANAGER_FEATURES => {
                cm::GetEnabledConnectivityManagerFeatures::read(&mut call.data).map_err(bad)?;
                self.features()
                    .map(|f| cm::write_get_enabled_connectivity_manager_features_reply(&mut p, f))
            }
            cm::REQUEST_NETWORK => {
                match cm::RequestNetwork::<NetworkCapabilities, Messenger>::read(&mut call.data) {
                    Ok(a) => self
                        .request_network(call, a)
                        .map(|r| cm::write_request_network_reply(&mut p, Some(&r))),
                    Err(BAD_TYPE) => Err(unreadable(&self.host)),
                    Err(s) => return Err(s),
                }
            }
            cm::LISTEN_FOR_NETWORK => {
                match cm::ListenForNetwork::<NetworkCapabilities, Messenger>::read(&mut call.data) {
                    Ok(a) => self
                        .listen_for_network(call, a)
                        .map(|r| cm::write_listen_for_network_reply(&mut p, Some(&r))),
                    Err(BAD_TYPE) => Err(unreadable(&self.host)),
                    Err(s) => return Err(s),
                }
            }
            cm::RELEASE_NETWORK_REQUEST => {
                match cm::ReleaseNetworkRequest::<NetworkRequest>::read(&mut call.data) {
                    Ok(a) => self
                        .release_network_request(call, a.network_request)
                        .map(|()| cm::write_release_network_request_reply(&mut p)),
                    Err(BAD_TYPE) => Err(unreadable(&self.host)),
                    Err(s) => return Err(s),
                }
            }
            code => {
                let name = cm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("connectivity", &format!("{name} (#1509)")))
            }
        };
        reply(result.map(|()| p))
    }
}

/// A request whose network specifier or transport info the service does
/// not read.
fn unreadable(host: &AppHost) -> Exception {
    host.unimplemented_named(
        "connectivity",
        "a network specifier or transport info other than Ethernet's (#1509)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(addr: [u8; 4], prefix: u8, gateway: [u8; 4], mtu: u32) -> Uplink {
        Uplink {
            index: 4,
            addr: addr.into(),
            prefix,
            gateway: gateway.into(),
            mtu,
        }
    }

    fn nri(caps: NetworkCapabilities, kind: RequestType, id: i32) -> Nri {
        Nri {
            request: NetworkRequest {
                caps,
                legacy_type: TYPE_NONE,
                request_id: id,
                kind,
            },
            messenger: None,
            binder: None,
            uid: 10_100,
            as_uid: 10_100,
            system: false,
            callback_flags: 0,
            declared_methods: DECLARED_METHODS_ALL,
            satisfier: None,
            timeout: None,
        }
    }

    fn register(state: &mut State, n: Nri) -> Result<Vec<Callback>, Exception> {
        state.admit(n.system, n.uid)?;
        Ok(state.add(n))
    }

    #[test]
    fn the_network_is_a_validated_ethernet_network() {
        let nc = network_capabilities();
        for c in [
            INTERNET,
            NOT_RESTRICTED,
            network::TRUSTED,
            NOT_VPN,
            VALIDATED,
            NOT_ROAMING,
            NOT_CONGESTED,
            NOT_SUSPENDED,
            NOT_VCN_MANAGED,
            NOT_METERED,
            FOREGROUND,
            network::NOT_BANDWIDTH_CONSTRAINED,
        ] {
            assert!(nc.has(c), "{c}");
        }
        assert!(nc.has_transport(TRANSPORT_ETHERNET) && !nc.is_metered());
        assert_eq!(nc.capabilities.count_ones(), 12);
        assert_eq!((nc.link_up_kbps, nc.link_down_kbps), (100_000, 100_000));
        // An app's default network request is satisfied by it.
        let request =
            default_capabilities_for_uid(10_100, 10_100, Some("com.example".into()), false);
        assert!(request.satisfied_by(&nc));
        assert_eq!(request.uids, Some(vec![(10_100, 10_100)]));
        assert!(!request.has(NOT_VPN) && request.has(INTERNET) && request.requestor_uid == 10_100);
    }

    #[test]
    fn link_properties_are_the_lease_ipclient_assembles() {
        let lp = link_properties(
            &up([172, 30, 1, 46], 24, [172, 30, 1, 254], 1500),
            &[Ipv4Addr::new(172, 30, 1, 254)],
            None,
        );
        assert_eq!(lp.interface, "eth0");
        assert_eq!(
            (lp.addresses[0].addr, lp.addresses[0].prefix),
            (Ipv4Addr::new(172, 30, 1, 46), 24)
        );
        assert_eq!(lp.routes.len(), 2);
        assert_eq!(lp.routes[0].destination, (Ipv4Addr::new(172, 30, 1, 0), 24));
        assert_eq!(lp.routes[0].gateway, Ipv4Addr::UNSPECIFIED);
        assert_eq!(
            (lp.routes[1].destination, lp.routes[1].gateway),
            ((Ipv4Addr::UNSPECIFIED, 0), Ipv4Addr::new(172, 30, 1, 254))
        );
        assert_eq!(
            (lp.mtu, lp.dhcp_server, lp.use_private_dns),
            (1500, Some(Ipv4Addr::new(172, 30, 1, 254)), false)
        );
        // A gateway outside the subnet gets a host route; an MTU DhcpPacket
        // refuses is none.
        let lp = link_properties(
            &up([10, 0, 0, 5], 32, [10, 0, 0, 1], 9000),
            &[],
            Some(Some("dns.example".into())),
        );
        assert_eq!(lp.routes[1].destination, (Ipv4Addr::new(10, 0, 0, 1), 32));
        assert_eq!(lp.routes.len(), 3);
        assert_eq!(
            (lp.mtu, lp.use_private_dns, lp.private_dns_server.as_deref()),
            (0, true, Some("dns.example"))
        );
    }

    #[test]
    fn requests_follow_the_mac_network() {
        let lease = (
            up([192, 168, 1, 20], 24, [192, 168, 1, 1], 1500),
            vec![Ipv4Addr::new(192, 168, 1, 1)],
        );
        let mut state = State::new(Some(lease.clone()));
        assert_eq!(state.network.as_ref().map(|n| n.id), Some(100));
        let mut internet = NetworkCapabilities::default();
        internet.add(INTERNET);
        let mut wifi = internet.clone();
        wifi.transports = 1 << TRANSPORT_WIFI;
        assert_eq!(
            register(&mut state, nri(internet, RequestType::Listen, 1)).unwrap(),
            [(0, CALLBACK_AVAILABLE, 100)]
        );
        assert_eq!(
            register(&mut state, nri(wifi, RequestType::Request, 2)).unwrap(),
            []
        );
        // New DNS servers: new link properties for what it satisfies.
        let dns = (lease.0.clone(), vec![Ipv4Addr::new(1, 1, 1, 1)]);
        assert_eq!(
            state.follow(Some(dns.clone())),
            [(0, CALLBACK_IP_CHANGED, 100)]
        );
        assert_eq!(state.follow(Some(dns)), []);
        // Another network: the old one lost, a new netId available.
        let other = (up([10, 1, 2, 3], 16, [10, 1, 0, 1], 1500), vec![]);
        assert_eq!(
            state.follow(Some(other.clone())),
            [(0, CALLBACK_LOST, 100), (0, CALLBACK_AVAILABLE, 101)]
        );
        assert_eq!(state.follow(None), [(0, CALLBACK_LOST, 101)]);
        assert_eq!(state.follow(Some(other)), [(0, CALLBACK_AVAILABLE, 102)]);
    }

    #[test]
    fn requests_are_limited_per_uid_and_released_by_their_uid() {
        let mut state = State::new(None);
        for id in 0..MAX_REQUESTS_PER_UID {
            register(
                &mut state,
                nri(NetworkCapabilities::default(), RequestType::Listen, id),
            )
            .unwrap();
        }
        let e = register(
            &mut state,
            nri(NetworkCapabilities::default(), RequestType::Listen, 1000),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.service_specific),
            (EX_SERVICE_SPECIFIC, TOO_MANY_REQUESTS)
        );
        assert_eq!(e.message, "Uid 10100 exceeded its allotted requests limit");
        let request = state.requests[3].request.clone();
        assert!(state.release(&request, 10_200).is_none());
        assert!(state.release(&request, 10_100).is_some());
        register(
            &mut state,
            nri(NetworkCapabilities::default(), RequestType::Listen, 1001),
        )
        .unwrap();
    }

    #[test]
    fn unsatisfied_requests_time_out() {
        let mut state = State::new(None);
        let now = Instant::now();
        let mut timed = nri(NetworkCapabilities::default(), RequestType::Request, 1);
        timed.timeout = Some(now);
        register(&mut state, timed).unwrap();
        register(
            &mut state,
            nri(NetworkCapabilities::default(), RequestType::Request, 2),
        )
        .unwrap();
        let expired = state.expire(now);
        assert_eq!(
            expired
                .iter()
                .map(|n| n.request.request_id)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(state.requests.len(), 1);
        // A network that satisfies a request stops its timeout.
        let mut timed = nri(NetworkCapabilities::default(), RequestType::Request, 3);
        timed.timeout = Some(now);
        register(&mut state, timed).unwrap();
        state.follow(Some((
            up([192, 168, 1, 20], 24, [192, 168, 1, 1], 1500),
            vec![],
        )));
        assert!(state.expire(now).is_empty());
    }

    #[test]
    fn callbacks_carry_their_bundle_to_declared_methods() {
        let mut n = nri(NetworkCapabilities::default(), RequestType::Listen, 7);
        let caps = network_capabilities();
        let lp = link_properties(
            &up([192, 168, 1, 20], 24, [192, 168, 1, 1], 1500),
            &[],
            None,
        );
        let data = callback_data(&n, CALLBACK_AVAILABLE, 100, 0, Some(&caps), Some(&lp)).unwrap();
        let mut r = data.reader();
        r.enforce_interface(imessenger::DESCRIPTOR).unwrap();
        assert_eq!(r.read_i32().unwrap(), 1); // a Message
        assert_eq!(
            (
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ),
            (CALLBACK_AVAILABLE, 0, 0)
        );
        let bundle = {
            r.read_i32().unwrap(); // obj
            r.read_i64().unwrap(); // when
            crate::bundle::read(&mut r).unwrap().unwrap()
        };
        let mut keys: Vec<_> = bundle.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "LinkProperties",
                "LocalNetworkInfo",
                "Network",
                "NetworkCapabilities",
                "NetworkRequest"
            ]
        );
        assert_eq!(bundle["LocalNetworkInfo"], crate::bundle::Value::Null);
        // LOST carries the request and the network; a callback that does
        // not override onLost gets none.
        let lost = callback_data(&n, CALLBACK_LOST, 100, 0, None, None).unwrap();
        let mut r = lost.reader();
        r.enforce_interface(imessenger::DESCRIPTOR).unwrap();
        r.set_position(r.position() + 4 * 4 + 4 + 8);
        assert_eq!(crate::bundle::read(&mut r).unwrap().unwrap().len(), 2);
        n.declared_methods = 1 << CALLBACK_AVAILABLE;
        assert!(callback_data(&n, CALLBACK_LOST, 100, 0, None, None).is_none());
        assert!(callback_data(&n, CALLBACK_UNAVAIL, 100, 0, None, None).is_none());
    }

    #[test]
    fn data_saver_state_is_read_as_net_policy_reads_it() {
        let xml = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<policy-list version="13" restrictBackground="true">
<uid-policy uid="10100" policy="1" />
<uid-policy uid="1000" policy="4" />
<whitelist>
<restrict-background uid="10101" />
<restrict-background uid="10100" />
</whitelist>
</policy-list>"#;
        let policy = NetPolicy::read(Some(xml), false).unwrap();
        assert!(policy.restrict_background);
        assert_eq!(policy.uid_policies.get(&1000), None);
        assert_eq!(policy.status(10_100), RESTRICT_BACKGROUND_STATUS_ENABLED);
        assert_eq!(
            policy.status(10_101),
            RESTRICT_BACKGROUND_STATUS_WHITELISTED
        );
        assert_eq!(policy.status(10_102), RESTRICT_BACKGROUND_STATUS_ENABLED);
        let off = NetPolicy::read(None, false).unwrap();
        assert_eq!(off.status(10_102), RESTRICT_BACKGROUND_STATUS_DISABLED);
        assert!(NetPolicy::read(None, true).unwrap().restrict_background);
    }

    #[test]
    fn requests_ask_for_requestable_capabilities() {
        let mut nc = NetworkCapabilities::default();
        assert!(ensure_capabilities(&nc, true).is_ok());
        nc.add(VALIDATED);
        assert!(ensure_capabilities(&nc, false).is_ok());
        assert_eq!(
            ensure_capabilities(&nc, true).unwrap_err().message,
            "Cannot request network with VALIDATED"
        );
        let mut all = NetworkCapabilities::default();
        all.specifier = Some(Specifier::MatchAll);
        assert_eq!(
            ensure_capabilities(&all, false).unwrap_err().message,
            "A MatchAllNetworkSpecifier is not permitted"
        );
        // A caller's own uid, or every uid, and itself as the requestor.
        let mut mine = NetworkCapabilities::default();
        mine.set_single_uid(10_200);
        mine.owner_uid = 10_200;
        restrict_uids_and_set_requestor(&mut mine, 10_100, Some("com.example".into()), false);
        assert_eq!(
            (mine.uids, mine.owner_uid, mine.requestor_uid),
            (Some(vec![(10_100, 10_100)]), INVALID_UID, 10_100)
        );
        let mut every = NetworkCapabilities::default();
        restrict_uids_and_set_requestor(&mut every, 10_100, None, false);
        assert_eq!(every.uids, None);
    }
}
