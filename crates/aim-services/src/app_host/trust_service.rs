//! `trust`: whether the device is locked, for the app host's services
//! that ask (the clipboard's). The app host runs inside the Mac's user
//! session, whose lock is the device's: while it runs, the device is
//! unlocked, so listeners are kept and never called.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel};
use aim_service_aidl::android_app_trust_itrustmanager as trust;

use super::{AppHost, reply};

pub struct TrustManager {
    host: Arc<AppHost>,
    listeners: Mutex<Vec<Binder>>,
}

impl TrustManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        TrustManager { host, listeners: Mutex::new(Vec::new()) }
    }
}

impl Service for TrustManager {
    fn descriptor(&self) -> &str {
        trust::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            trust::IS_DEVICE_LOCKED => {
                trust::IsDeviceLocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                trust::write_is_device_locked_reply(&mut p, false);
            }
            trust::REGISTER_DEVICE_LOCKED_STATE_LISTENER => {
                let args = trust::RegisterDeviceLockedStateListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(listener) = args.listener {
                    self.listeners.lock().unwrap().push(listener);
                }
                trust::write_register_device_locked_state_listener_reply(&mut p);
            }
            code => return reply(Err(self.host.unimplemented("trust", code))),
        }
        Ok(p)
    }
}
