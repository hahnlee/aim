//! `trust`: whether the device is locked and secure. The app host runs
//! inside the Mac's user session, whose lock is the device's (the owner's
//! choice, 2026-10-19): while it runs, the device is unlocked, so
//! listeners are kept and never called, and it is secure when the Mac's
//! user has a password (`aim_host_lock`).

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_app_trust_itrustmanager as trust;

use super::{AppHost, reply};

/// `Context.DEVICE_ID_DEFAULT`, `UserHandle.USER_ALL`.
const DEVICE_ID_DEFAULT: i32 = 0;
const USER_ALL: i32 = -1;

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
            // `isDeviceSecure`: a virtual device is insecure; the user is
            // the caller's, or another with INTERACT_ACROSS_USERS_FULL.
            trust::IS_DEVICE_SECURE => {
                let a = trust::IsDeviceSecure::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let secure = (|| {
                    if a.device_id != DEVICE_ID_DEFAULT {
                        return Ok(false);
                    }
                    if a.user_id == USER_ALL {
                        return Err(Exception::illegal_argument(format!("Call does not support special user #{}", a.user_id)));
                    }
                    self.host.incoming_user(call.sender_euid as i32, a.user_id, false)?;
                    aim_host_lock::secure().map_err(|code| Exception::new(EX_ILLEGAL_STATE, format!("trust: the Mac's lock: LocalAuthentication error {code}")))
                })();
                match secure {
                    Ok(secure) => trust::write_is_device_secure_reply(&mut p, secure),
                    Err(e) => return reply(Err(e)),
                }
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
