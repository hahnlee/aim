//! `telephony.registry`: TelephonyRegistry for the app host
//! (`android-16.0.0_r1`), which the system server runs on a device without
//! telephony too: the records of the subscription listeners apps register,
//! a pid's limited as `add` limits them, and of the phone state listeners
//! (`listenWithEventList`) once their permissions are checked. Without
//! telephony no phone process reports subscriptions or phone states, and
//! there is no phone to notify a listener of at once, so no listener is
//! ever told of anything. Location events' permissions
//! (LocationAccessPolicy) and the rest are #1713.
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
/// `TelephonyCallback.EVENT_*` with permissions of their own.
const EVENT_MESSAGE_WAITING_INDICATOR_CHANGED: i32 = 3;
const EVENT_CALL_FORWARDING_INDICATOR_CHANGED: i32 = 4;
const EVENT_CELL_LOCATION_CHANGED: i32 = 5;
const EVENT_CALL_STATE_CHANGED: i32 = 6;
const EVENT_CELL_INFO_CHANGED: i32 = 11;
const EVENT_SRVCC_STATE_CHANGED: i32 = 16;
const EVENT_VOICE_ACTIVATION_STATE_CHANGED: i32 = 18;
const EVENT_DISPLAY_INFO_CHANGED: i32 = 21;
const EVENT_ACTIVE_DATA_SUBSCRIPTION_ID_CHANGED: i32 = 23;
const EVENT_RADIO_POWER_STATE_CHANGED: i32 = 24;
const EVENT_EMERGENCY_NUMBER_LIST_CHANGED: i32 = 25;
const EVENT_OUTGOING_EMERGENCY_CALL: i32 = 29;
const EVENT_OUTGOING_EMERGENCY_SMS: i32 = 30;
const EVENT_ALLOWED_NETWORK_TYPE_LIST_CHANGED: i32 = 35;
const EVENT_LEGACY_CALL_STATE_CHANGED: i32 = 36;
const EVENT_EMERGENCY_CALLBACK_MODE_CHANGED: i32 = 40;
const EVENT_SIMULTANEOUS_CELLULAR_CALLING_SUBSCRIPTIONS_CHANGED: i32 = 41;
const EVENT_SECURITY_ALGORITHMS_CHANGED: i32 = 46;
const EVENT_CELLULAR_IDENTIFIER_DISCLOSED_CHANGED: i32 = 47;
/// `REQUIRE_PRECISE_PHONE_STATE_PERMISSION`'s events.
const PRECISE_PHONE_STATE_EVENTS: &[i32] = &[13, 14, 12, 26, 27, 28, 31, 32, 33, 34, 37, 39];
/// The compat changes that make `READ_PHONE_STATE` needed for call
/// states, the active data subscription and cell info, or (disabled)
/// for display info.
const ENABLE_GET_CALL_STATE_PERMISSION_PROTECTION: i64 = 157233955;
const REQUIRE_READ_PHONE_STATE_PERMISSION_FOR_ACTIVE_DATA_SUB_ID: i64 = 182478738;
const REQUIRE_READ_PHONE_STATE_PERMISSION_FOR_CELL_INFO: i64 = 184323934;
const REQUIRE_READ_PHONE_STATE_PERMISSION_FOR_DISPLAY_INFO: i64 = 183164979;
const READ_PHONE_STATE: &str = "android.permission.READ_PHONE_STATE";
const READ_PRIVILEGED_PHONE_STATE: &str = "android.permission.READ_PRIVILEGED_PHONE_STATE";
const READ_PRECISE_PHONE_STATE: &str = "android.permission.READ_PRECISE_PHONE_STATE";
const READ_ACTIVE_EMERGENCY_SESSION: &str = "android.permission.READ_ACTIVE_EMERGENCY_SESSION";
/// `AppOpsManager.OP_READ_PHONE_STATE`, `MODE_ALLOWED`.
const OP_READ_PHONE_STATE: i32 = 51;
const MODE_ALLOWED: i32 = 0;

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

    /// Whether `uid` holds `permission`.
    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `listen` (`listenWithEventList`): the caller's package checked; no
    /// events unregister it; else its permissions for the events
    /// (`checkListenerPermission`), then its record. No subscription
    /// manager without telephony, so its phone is
    /// `INVALID_SIM_SLOT_INDEX` and nothing is notified now.
    fn listen(&self, uid: i32, pid: i32, sub_id: i32, package: Option<&str>, feature: Option<&str>, callback: Option<Binder>, events: &[i32]) -> Result<(), Exception> {
        let package = package.unwrap_or_default();
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!("Package {package} does not belong to {uid}")));
        }
        if events.is_empty() {
            self.remove(callback);
            return Ok(());
        }
        if !self.check_listener_permission(uid, sub_id, package, feature, events)? {
            return Ok(());
        }
        let Some(Binder::Handle(handle)) = callback else {
            return Err(Exception::new(EX_NULL_POINTER, "callback"));
        };
        let limited = !matches!(uid.rem_euclid(PER_USER_RANGE as i32), SYSTEM_UID | PHONE_UID);
        self.add(handle, uid, pid, limited)
    }

    /// `checkListenerPermission`: false where a check fails softly (the
    /// listener is not added), a SecurityException where it fails hard.
    fn check_listener_permission(&self, uid: i32, sub_id: i32, package: &str, feature: Option<&str>, events: &[i32]) -> Result<bool, Exception> {
        let has = |e: i32| events.contains(&e);
        let change = |id: i64| self.host.change_enabled(id, uid);
        let mut successful = true;
        // `isLocationPermissionRequired`.
        if has(EVENT_CELL_LOCATION_CHANGED) || has(EVENT_CELL_INFO_CHANGED) {
            return Err(self.host.unimplemented_named("telephony.registry", "location events' permissions (#1713)"));
        }
        // `isPhoneStatePermissionRequired`.
        let phone_state = has(EVENT_CALL_FORWARDING_INDICATOR_CHANGED)
            || has(EVENT_MESSAGE_WAITING_INDICATOR_CHANGED)
            || has(EVENT_EMERGENCY_NUMBER_LIST_CHANGED)
            || (has(EVENT_LEGACY_CALL_STATE_CHANGED) || has(EVENT_CALL_STATE_CHANGED)) && change(ENABLE_GET_CALL_STATE_PERMISSION_PROTECTION)?
            || has(EVENT_ACTIVE_DATA_SUBSCRIPTION_ID_CHANGED) && change(REQUIRE_READ_PHONE_STATE_PERMISSION_FOR_ACTIVE_DATA_SUB_ID)?
            || has(EVENT_CELL_INFO_CHANGED) && change(REQUIRE_READ_PHONE_STATE_PERMISSION_FOR_CELL_INFO)?
            || has(EVENT_DISPLAY_INFO_CHANGED) && !change(REQUIRE_READ_PHONE_STATE_PERMISSION_FOR_DISPLAY_INFO)?;
        if phone_state && !self.check_read_phone_state(uid, sub_id, package, feature)? {
            successful = false;
        }
        // `isPrecisePhoneStatePermissionRequired`: else carrier privileges,
        // which no app has without subscriptions.
        if events.iter().any(|e| PRECISE_PHONE_STATE_EVENTS.contains(e)) && !self.holds(uid, READ_PRECISE_PHONE_STATE)? {
            return Err(Exception::security("listen"));
        }
        if (has(EVENT_OUTGOING_EMERGENCY_CALL) || has(EVENT_OUTGOING_EMERGENCY_SMS)) && !self.holds(uid, READ_ACTIVE_EMERGENCY_SESSION)? {
            return Err(Exception::security(format!("Neither user {uid} nor current process has {READ_ACTIVE_EMERGENCY_SESSION}.")));
        }
        let privileged = [
            EVENT_SRVCC_STATE_CHANGED,
            EVENT_VOICE_ACTIVATION_STATE_CHANGED,
            EVENT_RADIO_POWER_STATE_CHANGED,
            EVENT_ALLOWED_NETWORK_TYPE_LIST_CHANGED,
            EVENT_EMERGENCY_CALLBACK_MODE_CHANGED,
            EVENT_SIMULTANEOUS_CELLULAR_CALLING_SUBSCRIPTIONS_CHANGED,
            EVENT_CELLULAR_IDENTIFIER_DISCLOSED_CHANGED,
            EVENT_SECURITY_ALGORITHMS_CHANGED,
        ];
        if privileged.iter().any(|&e| has(e)) && !self.holds(uid, READ_PRIVILEGED_PHONE_STATE)? {
            return Err(Exception::security(format!("Neither user {uid} nor current process has {READ_PRIVILEGED_PHONE_STATE}.")));
        }
        Ok(successful)
    }

    /// `TelephonyPermissions.checkReadPhoneState`: READ_PRIVILEGED_PHONE_STATE,
    /// else READ_PHONE_STATE with its app op allowed; carrier privileges,
    /// the fallback for a valid subscription, no app has without
    /// subscriptions.
    fn check_read_phone_state(&self, uid: i32, sub_id: i32, package: &str, feature: Option<&str>) -> Result<bool, Exception> {
        if self.holds(uid, READ_PRIVILEGED_PHONE_STATE)? {
            return Ok(true);
        }
        if !self.holds(uid, READ_PHONE_STATE)? {
            if sub_id >= 0 {
                return Err(Exception::security("listen"));
            }
            return Err(Exception::security(format!("listen: uid {uid} does not have {READ_PHONE_STATE}.")));
        }
        let mode = super::appops_service::note_from(&self.host, SYSTEM_UID, OP_READ_PHONE_STATE, uid, Some(package), feature)?;
        Ok(mode == MODE_ALLOWED)
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
            tr::LISTEN_WITH_EVENT_LIST => {
                let a = tr::ListenWithEventList::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let events = a.events.unwrap_or_default();
                self.listen(uid, pid, a.sub_id, a.pkg.as_deref(), a.feature_id.as_deref(), a.callback, &events)
                    .map(|()| tr::write_listen_with_event_list_reply(&mut p))
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
