//! netd's network state, as ConnectivityService and NetworkStack set it:
//! networks and their interfaces and routes, the default network, per-uid
//! permissions. Nothing here reaches the host's
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
    pub protect_allowed: BTreeSet<i32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn state() -> MutexGuard<'static, Option<State>> {
    let mut g = STATE.lock().unwrap();
    g.get_or_insert_with(State::default);
    g
}
