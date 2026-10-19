//! `telephony.registry`: TelephonyRegistry for the app host
//! (`android-16.0.0_r1`), which the system server runs on a device without
//! telephony too: the records of the subscription listeners apps register,
//! a pid's limited as `add` limits them. Without telephony no phone
//! process reports subscriptions, so no listener is ever told of a change.
//! Phone state listeners (`listenWithEventList`) and the rest are #1713.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::com_android_internal_telephony_itelephonyregistry as tr;

use super::{AppHost, PER_USER_RANGE, reply};

/// `TelephonyCallback.FLAG_PER_PID_REGISTRATION_LIMIT` (DeviceConfig
/// `telephony`), its default, and `PHONE_STATE_LISTENER_LIMIT_CHANGE_ID`.
const REGISTRATION_LIMIT_FLAG: &str = "phone_state_listener_per_pid_registration_limit";
const DEFAULT_PER_PID_REGISTRATION_LIMIT: i32 = 50;
const PHONE_STATE_LISTENER_LIMIT_CHANGE_ID: i64 = 150880553;
/// `Process.SYSTEM_UID`, `PHONE_UID`.
const SYSTEM_UID: i32 = 1000;
const PHONE_UID: i32 = 1001;

/// A listener's record: its binder, its caller's pid, and its death link.
struct Record {
    binder: Strong,
    pid: i32,
    death: u64,
}

pub struct TelephonyRegistry {
    host: Arc<AppHost>,
    records: Arc<Mutex<Vec<Record>>>,
}

impl TelephonyRegistry {
    pub fn new(host: Arc<AppHost>) -> Self {
        TelephonyRegistry { host, records: Arc::default() }
    }

    /// `addOnSubscriptionsChangedListener` and its opportunistic twin:
    /// the caller's package checked, the record added (no notification
    /// has occurred to repeat to it).
    fn add_listener(&self, uid: i32, pid: i32, package: Option<&str>, callback: Option<Binder>) -> Result<(), Exception> {
        // `AppOpsManager.checkPackage`.
        let package = package.unwrap_or_default();
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!("Package {package} does not belong to {uid}")));
        }
        let Some(Binder::Handle(handle)) = callback else {
            return Err(Exception::new(EX_NULL_POINTER, "callback"));
        };
        // `doesLimitApplyForListeners`: not the system's or the phone's.
        let limited = !matches!(uid.rem_euclid(PER_USER_RANGE as i32), SYSTEM_UID | PHONE_UID);
        self.add(handle, uid, pid, limited)
    }

    /// `add`: an existing record of the binder, else a new one; past the
    /// pid's limit, an IllegalStateException where the compat change is
    /// on.
    fn add(&self, handle: u32, uid: i32, pid: i32, limited: bool) -> Result<(), Exception> {
        let limit = self
            .host
            .inputs()?
            .settings
            .device_config("telephony", REGISTRATION_LIMIT_FLAG)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_PER_PID_REGISTRATION_LIMIT);
        let mut records = self.records.lock().unwrap();
        if records.iter().any(|r| r.binder.handle == handle) {
            return Ok(());
        }
        let for_pid = records.iter().filter(|r| r.pid == pid).count() as i32;
        if limited && limit >= 1 && for_pid >= limit && self.host.change_enabled(PHONE_STATE_LISTENER_LIMIT_CHANGE_ID, uid)? {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("Pid {pid} has exceeded the number of permissible registered listeners. Ignoring request to add."),
            ));
        }
        let binder = self.host.process.strong(handle);
        let gone = Arc::downgrade(&self.records);
        let death = self.host.process.link_to_death(
            &binder,
            Box::new(move || {
                if let Some(records) = gone.upgrade() {
                    records.lock().unwrap().retain(|r| r.binder.handle != handle);
                }
            }),
        );
        records.push(Record { binder, pid, death });
        Ok(())
    }

    /// `remove`.
    fn remove(&self, callback: Option<Binder>) {
        let Some(Binder::Handle(handle)) = callback else { return };
        let mut records = self.records.lock().unwrap();
        if let Some(i) = records.iter().position(|r| r.binder.handle == handle) {
            let r = records.remove(i);
            self.host.process.clear_death(&r.binder, r.death);
        }
    }
}

impl Service for TelephonyRegistry {
    fn descriptor(&self) -> &str {
        tr::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let (uid, pid) = (call.sender_euid as i32, call.sender_pid);
        let mut p = Parcel::new();
        let result = match call.code {
            tr::ADD_ON_SUBSCRIPTIONS_CHANGED_LISTENER => {
                let a = tr::AddOnSubscriptionsChangedListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.add_listener(uid, pid, a.pkg.as_deref(), a.callback).map(|()| tr::write_add_on_subscriptions_changed_listener_reply(&mut p))
            }
            tr::ADD_ON_OPPORTUNISTIC_SUBSCRIPTIONS_CHANGED_LISTENER => {
                let a = tr::AddOnOpportunisticSubscriptionsChangedListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.add_listener(uid, pid, a.pkg.as_deref(), a.callback)
                    .map(|()| tr::write_add_on_opportunistic_subscriptions_changed_listener_reply(&mut p))
            }
            tr::REMOVE_ON_SUBSCRIPTIONS_CHANGED_LISTENER => {
                let a = tr::RemoveOnSubscriptionsChangedListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.remove(a.callback);
                tr::write_remove_on_subscriptions_changed_listener_reply(&mut p);
                Ok(())
            }
            code => {
                let name = tr::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("telephony.registry", &format!("{name} (#1713)")))
            }
        };
        reply(result.map(|()| p))
    }
}
