//! What ConnectivityService has netd and DnsResolver do for the app
//! host's network (`createNativeNetwork`, `updateLinkProperties`,
//! `makeDefault`, `destroyNativeNetwork` and `DnsManager
//! .sendDnsConfigurationForNetwork` at `android-16.0.0_r1`), so that apps'
//! name lookups (bionic's `dnsproxyd`) resolve on it: the network made
//! with its interface and routes, its resolver cache configured with the
//! Mac's DNS servers, and made the default. With them goes what
//! EthernetTracker and IpClient do to the interface itself: up once it is
//! tracked (`NetdUtils.setInterfaceUp`), the lease's address added before
//! the network is made (`InterfaceController.addAddress`) and the
//! addresses cleared when it goes (`clearAllAddresses`), so the kernel's
//! interface matches the link properties apps are given. The calls are
//! written as the frozen interfaces' Java proxies write them
//! (netd_aidl_interface V17, dnsresolver_aidl_interface V16).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::net::Ipv4Addr;

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::Parcel;

use super::network::RouteInfo;

const NETD_DESCRIPTOR: &str = "android.net.INetd";
const INTERFACE_ADD_ADDRESS: u32 = 15;
const INTERFACE_GET_CFG: u32 = 81;
const INTERFACE_SET_CFG: u32 = 82;
const INTERFACE_CLEAR_ADDRS: u32 = 84;
const NETWORK_DESTROY: u32 = 6;
const NETWORK_ADD_INTERFACE: u32 = 7;
const NETWORK_ADD_ROUTE: u32 = 62;
const NETWORK_SET_DEFAULT: u32 = 67;
const NETWORK_CLEAR_DEFAULT: u32 = 68;
const NETWORK_CREATE: u32 = 105;
const RESOLVER_DESCRIPTOR: &str = "android.net.IDnsResolver";
const SET_RESOLVER_CONFIGURATION: u32 = 3;
const CREATE_NETWORK_CACHE: u32 = 8;
const DESTROY_NETWORK_CACHE: u32 = 9;
/// `NativeNetworkType.PHYSICAL`, `INetd.PERMISSION_NONE`,
/// `NativeVpnType` of no VPN (`VpnManager.TYPE_VPN_NONE`), `INetd
/// .NEXTHOP_NONE`.
const PHYSICAL: i32 = 0;
const PERMISSION_NONE: i32 = 0;
const TYPE_VPN_NONE: i32 = -1;
const NEXTHOP_NONE: &str = "";

/// `DnsManager`'s resolver parameters: the `DNS_RESOLVER_*` settings,
/// checked as `updateParametersSettings` checks them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolverSettings {
    pub sample_validity: i32,
    pub success_threshold: i32,
    pub min_samples: i32,
    pub max_samples: i32,
}

impl ResolverSettings {
    /// The settings as `setting` reads them, each out of range taking its
    /// default.
    pub(super) fn read(setting: impl Fn(&str) -> Option<i32>) -> Self {
        let mut s = ResolverSettings {
            sample_validity: setting("dns_resolver_sample_validity_seconds").unwrap_or(1800),
            success_threshold: setting("dns_resolver_success_threshold_percent").unwrap_or(25),
            min_samples: setting("dns_resolver_min_samples").unwrap_or(8),
            max_samples: setting("dns_resolver_max_samples").unwrap_or(64),
        };
        if !(0..=65535).contains(&s.sample_validity) {
            s.sample_validity = 1800;
        }
        if !(0..=100).contains(&s.success_threshold) {
            s.success_threshold = 25;
        }
        if s.min_samples < 0 || s.min_samples > s.max_samples || s.max_samples > 64 {
            (s.min_samples, s.max_samples) = (8, 64);
        }
        s
    }
}

/// The network as netd and DnsResolver are to know it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NativeNetwork {
    pub net_id: i32,
    pub interface: String,
    /// The lease's address and prefix length.
    pub address: (Ipv4Addr, u8),
    pub routes: Vec<RouteInfo>,
    pub dns: Vec<Ipv4Addr>,
    /// Private DNS: `None` off; else in strict mode its specifier, whose
    /// servers no NetworkMonitor resolves here, or in opportunistic mode
    /// none (the plain servers tried over TLS too).
    pub private_dns: Option<Option<String>>,
    pub transports: Vec<i32>,
    pub settings: ResolverSettings,
}

pub(super) struct Native {
    pub netd: Strong,
    pub resolver: Strong,
}

impl Native {
    fn call(service: &Strong, descriptor: &str, code: u32, name: &str, write: impl FnOnce(&mut Parcel)) -> Result<(), String> {
        let mut data = Parcel::new();
        data.write_interface_token(descriptor);
        write(&mut data);
        let reply = service.transact(code, &data, false).map_err(|s| format!("{name}: {s}"))?;
        match reply.reader().read_exception() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("{name}: {e:?}")),
            Err(s) => Err(format!("{name}: {s}")),
        }
    }

    fn netd(&self, code: u32, name: &str, write: impl FnOnce(&mut Parcel)) -> Result<(), String> {
        Self::call(&self.netd, NETD_DESCRIPTOR, code, name, write)
    }

    /// `NetdUtils.setInterfaceUp`: the interface's configuration with
    /// "up" in its flags and "down" out, as EthernetTracker sets an
    /// interface it starts tracking.
    pub(super) fn set_interface_up(&self, interface: &str) -> Result<(), String> {
        let mut data = Parcel::new();
        data.write_interface_token(NETD_DESCRIPTOR);
        data.write_string16(Some(interface));
        let reply = self.netd.transact(INTERFACE_GET_CFG, &data, false).map_err(|s| format!("interfaceGetCfg: {s}"))?;
        let mut r = reply.reader();
        let read = |e| format!("interfaceGetCfg: {e}");
        r.read_exception().map_err(read)?.map_err(|e| format!("interfaceGetCfg: {e:?}"))?;
        // `InterfaceConfigurationParcel`: present, its size, then ifName,
        // hwAddr, ipv4Addr, prefixLength and flags.
        if r.read_i32().map_err(read)? == 0 {
            return Err(format!("interfaceGetCfg: no configuration of {interface}"));
        }
        r.read_i32().map_err(read)?;
        let name = r.read_string16().map_err(read)?;
        let hw = r.read_string16().map_err(read)?;
        let ipv4 = r.read_string16().map_err(read)?;
        let prefix = r.read_i32().map_err(read)?;
        let count = r.read_i32().map_err(read)?.max(0);
        let mut flags = Vec::new();
        for _ in 0..count {
            if let Some(flag) = r.read_string16().map_err(read)? {
                flags.push(flag);
            }
        }
        flags.retain(|f| f != "down" && f != "up");
        flags.push("up".into());
        self.netd(INTERFACE_SET_CFG, "interfaceSetCfg", |p| {
            // `writeTypedObject`: present.
            p.write_i32(1);
            structured(p, |p| {
                p.write_string16(name.as_deref());
                p.write_string16(hw.as_deref());
                p.write_string16(ipv4.as_deref());
                p.write_i32(prefix);
                strings(p, &flags);
            })
        })
    }

    fn resolver(&self, code: u32, name: &str, write: impl FnOnce(&mut Parcel)) -> Result<(), String> {
        Self::call(&self.resolver, RESOLVER_DESCRIPTOR, code, name, write)
    }

    /// `createNativeNetwork`, `updateLinkProperties`' interfaces, routes
    /// and DNS, then `makeDefault`.
    pub(super) fn create(&self, n: &NativeNetwork) -> Result<(), String> {
        self.netd(INTERFACE_ADD_ADDRESS, "interfaceAddAddress", |p| {
            p.write_string16(Some(&n.interface));
            p.write_string16(Some(&n.address.0.to_string()));
            p.write_i32(n.address.1.into());
        })?;
        self.netd(NETWORK_CREATE, "networkCreate", |p| {
            // `writeTypedObject`: present.
            p.write_i32(1);
            structured(p, |p| {
                p.write_i32(n.net_id);
                p.write_i32(PHYSICAL);
                p.write_i32(PERMISSION_NONE);
                p.write_bool(false); // not secure
                p.write_i32(TYPE_VPN_NONE);
                p.write_bool(false); // local routes kept
            })
        })?;
        self.resolver(CREATE_NETWORK_CACHE, "createNetworkCache", |p| p.write_i32(n.net_id))?;
        self.netd(NETWORK_ADD_INTERFACE, "networkAddInterface", |p| {
            p.write_i32(n.net_id);
            p.write_string16(Some(&n.interface));
        })?;
        for route in &n.routes {
            // `NetdUtils.modifyRoute` of a unicast route.
            let (dst, prefix) = route.destination;
            let next_hop = if route.gateway.is_unspecified() { NEXTHOP_NONE.to_string() } else { route.gateway.to_string() };
            self.netd(NETWORK_ADD_ROUTE, "networkAddRoute", |p| {
                p.write_i32(n.net_id);
                p.write_string16(Some(&route.interface));
                p.write_string16(Some(&format!("{dst}/{prefix}")));
                p.write_string16(Some(&next_hop));
            })?;
        }
        self.configure_dns(n)?;
        self.netd(NETWORK_SET_DEFAULT, "networkSetDefault", |p| p.write_i32(n.net_id))
    }

    /// `sendDnsConfigurationForNetwork`.
    pub(super) fn configure_dns(&self, n: &NativeNetwork) -> Result<(), String> {
        let servers: Vec<String> = n.dns.iter().map(Ipv4Addr::to_string).collect();
        self.resolver(SET_RESOLVER_CONFIGURATION, "setResolverConfiguration", |p| write_resolver_params(p, n, &servers))
    }

    /// `makeDefault(null)` of the default network, then
    /// `destroyNativeNetwork`, and its interface's addresses cleared.
    pub(super) fn destroy(&self, n: &NativeNetwork) -> Result<(), String> {
        self.netd(NETWORK_CLEAR_DEFAULT, "networkClearDefault", |_| {})?;
        self.netd(NETWORK_DESTROY, "networkDestroy", |p| p.write_i32(n.net_id))?;
        self.resolver(DESTROY_NETWORK_CACHE, "destroyNetworkCache", |p| p.write_i32(n.net_id))?;
        self.netd(INTERFACE_CLEAR_ADDRS, "interfaceClearAddrs", |p| p.write_string16(Some(&n.interface)))
    }
}

/// A structured parcelable: its size, then its fields.
fn structured(p: &mut Parcel, fields: impl FnOnce(&mut Parcel)) {
    let start = p.position();
    p.write_i32(0);
    fields(p);
    p.set_i32_at(start, (p.position() - start) as i32);
}

fn strings(p: &mut Parcel, values: &[String]) {
    p.write_i32(values.len() as i32);
    for v in values {
        p.write_string16(Some(v));
    }
}

/// `ResolverParamsParcel` as `sendDnsConfigurationForNetwork` fills it:
/// no search domains; private DNS's TLS name in strict mode, its TLS
/// servers the plain ones in opportunistic mode; no resolver options or
/// DoH (DDR off).
fn write_resolver_params(p: &mut Parcel, n: &NativeNetwork, servers: &[String]) {
    p.write_i32(1);
    structured(p, |p| {
        p.write_i32(n.net_id);
        p.write_i32(n.settings.sample_validity);
        p.write_i32(n.settings.success_threshold);
        p.write_i32(n.settings.min_samples);
        p.write_i32(n.settings.max_samples);
        p.write_i32(0); // base timeout: the resolver's
        p.write_i32(0); // retry count: the resolver's
        strings(p, servers);
        strings(p, &[]); // domains
        let strict = n.private_dns.as_ref().and_then(|s| s.as_deref());
        p.write_string16(Some(strict.unwrap_or_default())); // TLS name
        strings(p, if matches!(n.private_dns, Some(None)) { servers } else { &[] });
        strings(p, &[]); // TLS fingerprints
        p.write_string16(Some("")); // CA certificate
        p.write_i32(0); // TLS connect timeout
        p.write_i32(0); // no resolver options
        p.write_i32(n.transports.len() as i32);
        for t in &n.transports {
            p.write_i32(*t);
        }
        p.write_bool(false); // not metered
        p.write_i32(0); // no DoH parameters
        strings(p, std::slice::from_ref(&n.interface));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_settings_out_of_range_take_their_defaults() {
        let s = ResolverSettings::read(|name| match name {
            "dns_resolver_sample_validity_seconds" => Some(70_000),
            "dns_resolver_min_samples" => Some(10),
            "dns_resolver_max_samples" => Some(5),
            _ => None,
        });
        assert_eq!(s, ResolverSettings { sample_validity: 1800, success_threshold: 25, min_samples: 8, max_samples: 64 });
    }

    #[test]
    fn resolver_params_are_a_nonnull_structured_parcelable() {
        let n = NativeNetwork {
            net_id: 100,
            interface: "eth0".into(),
            address: (Ipv4Addr::new(10, 0, 0, 2), 24),
            routes: Vec::new(),
            dns: vec![Ipv4Addr::new(10, 0, 0, 1)],
            private_dns: Some(None),
            transports: vec![3],
            settings: ResolverSettings::read(|_| None),
        };
        let mut p = Parcel::new();
        write_resolver_params(&mut p, &n, &["10.0.0.1".into()]);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 1);
        let size = r.read_i32().unwrap() as usize;
        assert_eq!(size + 4, p.data().len());
        assert_eq!(r.read_i32().unwrap(), 100);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1800, 25));
    }
}
