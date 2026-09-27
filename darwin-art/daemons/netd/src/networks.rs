//! netd's network state, as ConnectivityService and NetworkStack set it:
//! networks and their interfaces and routes, the default network, per-uid
//! permissions and /proc/sys/net values. Nothing here reaches the host's
//! routing: the host's own networking carries every socket (ADR 0012
//! appendix, netd), so this is bookkeeping that answers INetd's queries
//! consistently.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard};

static DEFAULT_NETWORK: AtomicU32 = AtomicU32::new(0);

/// The default network (0 when there is none).
pub fn default_network() -> u32 {
    DEFAULT_NETWORK.load(Relaxed)
}

pub fn set_default_network(net: u32) {
    DEFAULT_NETWORK.store(net, Relaxed);
}

#[derive(Default)]
pub struct Network {
    pub interfaces: BTreeSet<String>,
    /// (interface, destination, next hop).
    pub routes: BTreeSet<(String, String, String)>,
    pub permission: i32,
}

#[derive(Default)]
pub struct State {
    pub networks: BTreeMap<i32, Network>,
    /// `/proc/sys/net/<ipv4|ipv6>/<conf|neigh>/<ifname>/<parameter>` values
    /// written through setProcSysNet.
    pub proc_sys: BTreeMap<String, String>,
    pub protect_allowed: BTreeSet<i32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn state() -> MutexGuard<'static, Option<State>> {
    let mut g = STATE.lock().unwrap();
    g.get_or_insert_with(State::default);
    g
}

/// The key of a /proc/sys/net value, as netd's InterfaceController builds
/// the path.
pub fn proc_sys_key(ipversion: i32, which: i32, ifname: &str, parameter: &str) -> String {
    let family = if ipversion == 6 { "ipv6" } else { "ipv4" };
    let kind = if which == 2 { "neigh" } else { "conf" };
    format!("{family}/{kind}/{ifname}/{parameter}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_sys_keys_follow_netd_paths() {
        assert_eq!(
            proc_sys_key(6, 1, "eth0", "accept_ra"),
            "ipv6/conf/eth0/accept_ra"
        );
        assert_eq!(
            proc_sys_key(4, 2, "lo", "ucast_solicit"),
            "ipv4/neigh/lo/ucast_solicit"
        );
    }
}
