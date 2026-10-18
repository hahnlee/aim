//! `vpn_management`: VpnManagerService for the app host, registered so
//! that `VpnManager`, which Settings and apps make from it, exists. No
//! method is implemented yet (#1640).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};

use super::{AppHost, reply};

/// `IVpnManager`'s interface token.
const DESCRIPTOR: &str = "android.net.IVpnManager";

pub struct VpnManager {
    host: Arc<AppHost>,
}

impl VpnManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        VpnManager { host }
    }
}

impl Service for VpnManager {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        reply(Err(self.host.unimplemented("vpn_management", call.code)))
    }
}
