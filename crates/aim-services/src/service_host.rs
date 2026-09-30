//! The service host's own binder, registered as `aim.service_host`: the
//! device's system service in system_server (java/device-services) hands
//! it the system_server bridge when system services are ready
//! (docs/system-services.md, "The system_server bridge").

use std::sync::{Arc, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{Binder, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iservicehost as host;

use crate::SYSTEM_UID;
use crate::system::System;

pub struct ServiceHost {
    system: Weak<System>,
}

impl ServiceHost {
    pub fn new(system: &Arc<System>) -> Self {
        Self {
            system: Arc::downgrade(system),
        }
    }
}

impl Service for ServiceHost {
    fn descriptor(&self) -> &str {
        host::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != host::ATTACH_BRIDGE {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Only system_server's side holds the bridge; the system uid is
        // the one it runs as.
        if call.sender_euid != SYSTEM_UID {
            eprintln!(
                "services: attachBridge from uid {} refused",
                call.sender_euid
            );
            return Ok(Parcel::new());
        }
        let bridge = host::AttachBridge::read(&mut call.data)?.bridge;
        match (bridge, self.system.upgrade()) {
            (Some(Binder::Handle(handle)), Some(system)) => match system.attach_bridge(handle) {
                Ok(()) => eprintln!("services: system_server's bridge attached"),
                Err(e) => eprintln!("services: cannot use the bridge: {}", e.message),
            },
            (bridge, _) => eprintln!("services: attachBridge without a bridge ({bridge:?})"),
        }
        Ok(Parcel::new())
    }
}
