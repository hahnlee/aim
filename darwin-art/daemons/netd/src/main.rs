//! `netd` of the derived image (ADR 0012 appendix, "Replaced native
//! daemons"): INetd (netd_aidl_interface V17) without netlink, iptables,
//! policy routing or eBPF, which the original drives.
//!
//! - Guest sockets are host sockets: the host's networking carries them,
//!   so the network, route, firewall, bandwidth and traffic-accounting
//!   calls are bookkeeping that succeeds (`networks`).
//! - The only interface is the loopback. A network backed by the Mac's
//!   connection (which ConnectivityService would see through a transport)
//!   is not provided yet.
//! - Tethering offload, clatd, IPsec and XFRM need kernel features the host
//!   does not expose to the guest and fail with UNSUPPORTED_OPERATION.
//! - netd hosts the original DnsResolver (`resolv`), which serves
//!   `dnsresolver` and the `dnsproxyd` socket, and answers `fwmarkd`
//!   (`fwmark`), which libnetd_client in every process talks to.

mod fwmark;
mod networks;
mod resolv;

use std::sync::Mutex;

use android_net_netd::aidl::android::net::{
    INetd::{self, BnNetd},
    INetdUnsolicitedEventListener::INetdUnsolicitedEventListener,
    InterfaceConfigurationParcel::InterfaceConfigurationParcel,
    IpSecMigrateInfoParcel::IpSecMigrateInfoParcel,
    MarkMaskParcel::MarkMaskParcel,
    NativeNetworkConfig::NativeNetworkConfig,
    RouteInfoParcel::RouteInfoParcel,
    TetherConfigParcel::TetherConfigParcel,
    TetherOffloadRuleParcel::TetherOffloadRuleParcel,
    TetherStatsParcel::TetherStatsParcel,
    UidRangeParcel::UidRangeParcel,
    netd::aidl::NativeUidRangeConfig::NativeUidRangeConfig,
};
use binder::{
    BinderFeatures, ExceptionCode, Interface, ParcelFileDescriptor, SpIBinder, Status, Strong,
};

const SERVICE: &str = "netd";
const LOOPBACK: &str = "lo";

#[derive(Default)]
struct Netd {
    listeners: Mutex<Vec<Strong<dyn INetdUnsolicitedEventListener>>>,
}

fn unsupported<T>(what: &str) -> binder::Result<T> {
    Err(Status::new_exception_str(
        ExceptionCode::UNSUPPORTED_OPERATION,
        Some(format!("{what}: no such kernel feature on this device")),
    ))
}

fn no_interface<T>(name: &str) -> binder::Result<T> {
    Err(Status::new_service_specific_error_str(
        libc::ENODEV,
        Some(format!("no interface {name}")),
    ))
}

fn with_network(net_id: i32, f: impl FnOnce(&mut networks::Network)) -> binder::Result<()> {
    let mut g = networks::state();
    match g.as_mut().unwrap().networks.get_mut(&net_id) {
        Some(n) => {
            f(n);
            Ok(())
        }
        None => Err(Status::new_service_specific_error_str(
            libc::ENONET,
            Some(format!("no network {net_id}")),
        )),
    }
}

fn add_network(net_id: i32, permission: i32) {
    let mut g = networks::state();
    g.as_mut()
        .unwrap()
        .networks
        .entry(net_id)
        .or_default()
        .permission = permission;
}

impl Interface for Netd {}

#[allow(non_snake_case)]
impl INetd::INetd for Netd {
    fn isAlive(&self) -> binder::Result<bool> {
        Ok(true)
    }
    fn firewallReplaceUidChain(&self, _: &str, _: bool, _: &[i32]) -> binder::Result<bool> {
        Ok(true)
    }
    fn bandwidthEnableDataSaver(&self, _: bool) -> binder::Result<bool> {
        Ok(true)
    }
    fn networkCreatePhysical(&self, net_id: i32, permission: i32) -> binder::Result<()> {
        add_network(net_id, permission);
        Ok(())
    }
    fn networkCreateVpn(&self, net_id: i32, _: bool) -> binder::Result<()> {
        add_network(net_id, INetd::PERMISSION_NONE);
        Ok(())
    }
    fn networkCreate(&self, config: &NativeNetworkConfig) -> binder::Result<()> {
        add_network(config.netId, config.permission);
        Ok(())
    }
    fn networkDestroy(&self, net_id: i32) -> binder::Result<()> {
        networks::state().as_mut().unwrap().networks.remove(&net_id);
        if networks::default_network() == net_id as u32 {
            networks::set_default_network(0);
        }
        Ok(())
    }
    fn networkAddInterface(&self, net_id: i32, iface: &str) -> binder::Result<()> {
        with_network(net_id, |n| {
            n.interfaces.insert(iface.to_string());
        })
    }
    fn networkRemoveInterface(&self, net_id: i32, iface: &str) -> binder::Result<()> {
        with_network(net_id, |n| {
            n.interfaces.remove(iface);
        })
    }
    fn networkAddUidRanges(&self, _: i32, _: &[UidRangeParcel]) -> binder::Result<()> {
        Ok(())
    }
    fn networkRemoveUidRanges(&self, _: i32, _: &[UidRangeParcel]) -> binder::Result<()> {
        Ok(())
    }
    fn networkAddUidRangesParcel(&self, _: &NativeUidRangeConfig) -> binder::Result<()> {
        Ok(())
    }
    fn networkRemoveUidRangesParcel(&self, _: &NativeUidRangeConfig) -> binder::Result<()> {
        Ok(())
    }
    fn networkRejectNonSecureVpn(&self, _: bool, _: &[UidRangeParcel]) -> binder::Result<()> {
        Ok(())
    }
    fn socketDestroy(&self, _: &[UidRangeParcel], _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn tetherApplyDnsInterfaces(&self) -> binder::Result<bool> {
        Ok(true)
    }
    fn tetherGetStats(&self) -> binder::Result<Vec<TetherStatsParcel>> {
        Ok(Vec::new())
    }
    fn interfaceAddAddress(&self, ifname: &str, _: &str, _: i32) -> binder::Result<()> {
        no_interface(ifname)
    }
    fn interfaceDelAddress(&self, ifname: &str, _: &str, _: i32) -> binder::Result<()> {
        no_interface(ifname)
    }
    fn getProcSysNet(
        &self,
        ipversion: i32,
        which: i32,
        ifname: &str,
        parameter: &str,
    ) -> binder::Result<String> {
        let key = networks::proc_sys_key(ipversion, which, ifname, parameter);
        let g = networks::state();
        Ok(g.as_ref()
            .unwrap()
            .proc_sys
            .get(&key)
            .cloned()
            .unwrap_or_else(|| "0".into()))
    }
    fn setProcSysNet(
        &self,
        ipversion: i32,
        which: i32,
        ifname: &str,
        parameter: &str,
        value: &str,
    ) -> binder::Result<()> {
        let key = networks::proc_sys_key(ipversion, which, ifname, parameter);
        networks::state()
            .as_mut()
            .unwrap()
            .proc_sys
            .insert(key, value.to_string());
        Ok(())
    }
    fn ipSecSetEncapSocketOwner(&self, _: &ParcelFileDescriptor, _: i32) -> binder::Result<()> {
        unsupported("ipSecSetEncapSocketOwner")
    }
    fn ipSecAllocateSpi(&self, _: i32, _: &str, _: &str, _: i32) -> binder::Result<i32> {
        unsupported("ipSecAllocateSpi")
    }
    fn ipSecAddSecurityAssociation(
        &self,
        _: i32,
        _: i32,
        _: &str,
        _: &str,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
        _: &str,
        _: &[u8],
        _: i32,
        _: &str,
        _: &[u8],
        _: i32,
        _: &str,
        _: &[u8],
        _: i32,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecAddSecurityAssociation")
    }
    fn ipSecDeleteSecurityAssociation(
        &self,
        _: i32,
        _: &str,
        _: &str,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecDeleteSecurityAssociation")
    }
    fn ipSecApplyTransportModeTransform(
        &self,
        _: &ParcelFileDescriptor,
        _: i32,
        _: i32,
        _: &str,
        _: &str,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecApplyTransportModeTransform")
    }
    fn ipSecRemoveTransportModeTransform(&self, _: &ParcelFileDescriptor) -> binder::Result<()> {
        unsupported("ipSecRemoveTransportModeTransform")
    }
    fn ipSecAddSecurityPolicy(
        &self,
        _: i32,
        _: i32,
        _: i32,
        _: &str,
        _: &str,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecAddSecurityPolicy")
    }
    fn ipSecUpdateSecurityPolicy(
        &self,
        _: i32,
        _: i32,
        _: i32,
        _: &str,
        _: &str,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecUpdateSecurityPolicy")
    }
    fn ipSecDeleteSecurityPolicy(
        &self,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecDeleteSecurityPolicy")
    }
    fn ipSecAddTunnelInterface(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecAddTunnelInterface")
    }
    fn ipSecUpdateTunnelInterface(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        unsupported("ipSecUpdateTunnelInterface")
    }
    fn ipSecRemoveTunnelInterface(&self, _: &str) -> binder::Result<()> {
        unsupported("ipSecRemoveTunnelInterface")
    }
    fn ipSecMigrate(&self, _: &IpSecMigrateInfoParcel) -> binder::Result<()> {
        unsupported("ipSecMigrate")
    }
    fn wakeupAddInterface(&self, _: &str, _: &str, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn wakeupDelInterface(&self, _: &str, _: &str, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn setIPv6AddrGenMode(&self, _: &str, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn idletimerAddInterface(&self, _: &str, _: i32, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn idletimerRemoveInterface(&self, _: &str, _: i32, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn strictUidCleartextPenalty(&self, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn clatdStart(&self, _: &str, _: &str) -> binder::Result<String> {
        unsupported("clatdStart")
    }
    fn clatdStop(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn ipfwdEnabled(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn ipfwdGetRequesterList(&self) -> binder::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn ipfwdEnableForwarding(&self, _: &str) -> binder::Result<()> {
        unsupported("ipfwdEnableForwarding")
    }
    fn ipfwdDisableForwarding(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn ipfwdAddInterfaceForward(&self, _: &str, _: &str) -> binder::Result<()> {
        unsupported("ipfwdAddInterfaceForward")
    }
    fn ipfwdRemoveInterfaceForward(&self, _: &str, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthSetInterfaceQuota(&self, _: &str, _: i64) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthRemoveInterfaceQuota(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthSetInterfaceAlert(&self, _: &str, _: i64) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthRemoveInterfaceAlert(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthSetGlobalAlert(&self, _: i64) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthAddNaughtyApp(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthRemoveNaughtyApp(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthAddNiceApp(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn bandwidthRemoveNiceApp(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn tetherStart(&self, _: &[String]) -> binder::Result<()> {
        unsupported("tetherStart")
    }
    fn tetherStartWithConfiguration(&self, _: &TetherConfigParcel) -> binder::Result<()> {
        unsupported("tetherStartWithConfiguration")
    }
    fn tetherStop(&self) -> binder::Result<()> {
        Ok(())
    }
    fn tetherIsEnabled(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn tetherInterfaceAdd(&self, name: &str) -> binder::Result<()> {
        no_interface(name)
    }
    fn tetherInterfaceRemove(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn tetherInterfaceList(&self) -> binder::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn tetherDnsSet(&self, _: i32, _: &[String]) -> binder::Result<()> {
        Ok(())
    }
    fn tetherDnsList(&self) -> binder::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn networkAddRoute(
        &self,
        net_id: i32,
        ifname: &str,
        dst: &str,
        next_hop: &str,
    ) -> binder::Result<()> {
        with_network(net_id, |n| {
            n.routes
                .insert((ifname.into(), dst.into(), next_hop.into()));
        })
    }
    fn networkRemoveRoute(
        &self,
        net_id: i32,
        ifname: &str,
        dst: &str,
        next_hop: &str,
    ) -> binder::Result<()> {
        with_network(net_id, |n| {
            n.routes
                .remove(&(ifname.into(), dst.into(), next_hop.into()));
        })
    }
    fn networkAddLegacyRoute(
        &self,
        net_id: i32,
        ifname: &str,
        dst: &str,
        next_hop: &str,
        _: i32,
    ) -> binder::Result<()> {
        self.networkAddRoute(net_id, ifname, dst, next_hop)
    }
    fn networkRemoveLegacyRoute(
        &self,
        net_id: i32,
        ifname: &str,
        dst: &str,
        next_hop: &str,
        _: i32,
    ) -> binder::Result<()> {
        self.networkRemoveRoute(net_id, ifname, dst, next_hop)
    }
    fn networkAddRouteParcel(&self, net_id: i32, r: &RouteInfoParcel) -> binder::Result<()> {
        self.networkAddRoute(net_id, &r.ifName, &r.destination, &r.nextHop)
    }
    fn networkUpdateRouteParcel(&self, net_id: i32, r: &RouteInfoParcel) -> binder::Result<()> {
        self.networkAddRoute(net_id, &r.ifName, &r.destination, &r.nextHop)
    }
    fn networkRemoveRouteParcel(&self, net_id: i32, r: &RouteInfoParcel) -> binder::Result<()> {
        self.networkRemoveRoute(net_id, &r.ifName, &r.destination, &r.nextHop)
    }
    fn networkGetDefault(&self) -> binder::Result<i32> {
        Ok(networks::default_network() as i32)
    }
    fn networkSetDefault(&self, net_id: i32) -> binder::Result<()> {
        networks::set_default_network(net_id as u32);
        Ok(())
    }
    fn networkClearDefault(&self) -> binder::Result<()> {
        networks::set_default_network(0);
        Ok(())
    }
    fn networkSetPermissionForNetwork(&self, net_id: i32, permission: i32) -> binder::Result<()> {
        with_network(net_id, |n| n.permission = permission)
    }
    fn networkSetPermissionForUser(&self, _: i32, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn networkClearPermissionForUser(&self, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn trafficSetNetPermForUids(&self, _: i32, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn networkSetProtectAllow(&self, uid: i32) -> binder::Result<()> {
        networks::state()
            .as_mut()
            .unwrap()
            .protect_allowed
            .insert(uid);
        Ok(())
    }
    fn networkSetProtectDeny(&self, uid: i32) -> binder::Result<()> {
        networks::state()
            .as_mut()
            .unwrap()
            .protect_allowed
            .remove(&uid);
        Ok(())
    }
    fn networkCanProtect(&self, uid: i32) -> binder::Result<bool> {
        Ok(networks::state()
            .as_ref()
            .unwrap()
            .protect_allowed
            .contains(&uid))
    }
    fn networkAllowBypassVpnOnNetwork(&self, _: bool, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn setNetworkAllowlist(&self, _: &[NativeUidRangeConfig]) -> binder::Result<()> {
        Ok(())
    }
    fn firewallSetFirewallType(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn firewallSetInterfaceRule(&self, _: &str, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn firewallSetUidRule(&self, _: i32, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn firewallEnableChildChain(&self, _: i32, _: bool) -> binder::Result<()> {
        Ok(())
    }
    fn firewallAddUidInterfaceRules(&self, _: &str, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn firewallRemoveUidInterfaceRules(&self, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn interfaceGetList(&self) -> binder::Result<Vec<String>> {
        Ok(vec![LOOPBACK.to_string()])
    }
    fn interfaceGetCfg(&self, ifname: &str) -> binder::Result<InterfaceConfigurationParcel> {
        if ifname != LOOPBACK {
            return no_interface(ifname);
        }
        Ok(InterfaceConfigurationParcel {
            ifName: LOOPBACK.into(),
            hwAddr: "00:00:00:00:00:00".into(),
            ipv4Addr: "127.0.0.1".into(),
            prefixLength: 8,
            flags: [
                INetd::IF_STATE_UP,
                INetd::IF_FLAG_LOOPBACK,
                INetd::IF_FLAG_RUNNING,
            ]
            .map(String::from)
            .into(),
        })
    }
    fn interfaceSetCfg(&self, cfg: &InterfaceConfigurationParcel) -> binder::Result<()> {
        if cfg.ifName != LOOPBACK {
            return no_interface(&cfg.ifName);
        }
        Ok(())
    }
    fn interfaceSetIPv6PrivacyExtensions(&self, _: &str, _: bool) -> binder::Result<()> {
        Ok(())
    }
    fn interfaceClearAddrs(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn interfaceSetEnableIPv6(&self, _: &str, _: bool) -> binder::Result<()> {
        Ok(())
    }
    fn interfaceSetMtu(&self, ifname: &str, _: i32) -> binder::Result<()> {
        if ifname != LOOPBACK {
            return no_interface(ifname);
        }
        Ok(())
    }
    fn tetherAddForward(&self, _: &str, _: &str) -> binder::Result<()> {
        unsupported("tetherAddForward")
    }
    fn tetherRemoveForward(&self, _: &str, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn setTcpRWmemorySize(&self, _: &str, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn registerUnsolicitedEventListener(
        &self,
        listener: &Strong<dyn INetdUnsolicitedEventListener>,
    ) -> binder::Result<()> {
        self.listeners.lock().unwrap().push(listener.clone());
        Ok(())
    }
    fn trafficSwapActiveStatsMap(&self) -> binder::Result<()> {
        Ok(())
    }
    fn getOemNetd(&self) -> binder::Result<SpIBinder> {
        unsupported("getOemNetd")
    }
    fn getFwmarkForNetwork(&self, net_id: i32) -> binder::Result<MarkMaskParcel> {
        // Fwmark: the network id in the low 16 bits.
        Ok(MarkMaskParcel {
            mark: net_id & 0xffff,
            mask: 0xffff,
        })
    }
    fn tetherOffloadRuleAdd(&self, _: &TetherOffloadRuleParcel) -> binder::Result<()> {
        unsupported("tetherOffloadRuleAdd")
    }
    fn tetherOffloadRuleRemove(&self, _: &TetherOffloadRuleParcel) -> binder::Result<()> {
        Ok(())
    }
    fn tetherOffloadGetStats(&self) -> binder::Result<Vec<TetherStatsParcel>> {
        Ok(Vec::new())
    }
    fn tetherOffloadSetInterfaceQuota(&self, _: i32, _: i64) -> binder::Result<()> {
        Ok(())
    }
    fn tetherOffloadGetAndClearStats(&self, if_index: i32) -> binder::Result<TetherStatsParcel> {
        Ok(TetherStatsParcel {
            ifIndex: if_index,
            ..Default::default()
        })
    }
}

/// The descriptor init created for socket `name` (ANDROID_SOCKET_<name>).
fn control_socket(name: &str) -> Option<i32> {
    std::env::var(format!("ANDROID_SOCKET_{name}"))
        .ok()?
        .parse()
        .ok()
}

fn main() {
    daemon_log::init("netd");
    if let Err(e) = resolv::start() {
        log::error!("DnsResolver: {e}");
        std::process::exit(1);
    }
    if let Some(fd) = control_socket("fwmarkd") {
        std::thread::spawn(move || fwmark::serve(fd));
    } else {
        log::warn!("no fwmarkd socket from init");
    }
    let netd = BnNetd::new_binder(Netd::default(), BinderFeatures::default());
    if let Err(e) = binder::add_service(SERVICE, netd.as_binder()) {
        log::error!("cannot register {SERVICE}: {e:?}");
        std::process::exit(1);
    }
    log::info!("registered {SERVICE}");
    binder::ProcessState::start_thread_pool();
    binder::ProcessState::join_thread_pool();
}
