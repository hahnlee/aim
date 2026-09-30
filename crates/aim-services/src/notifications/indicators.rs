//! The apps' indicators (`docs/notifications.md`, "The menu bar"): the app
//! ops SystemUI's status bar watches while an app has them active
//! (`AppOpsManager.startWatchingActive`, as `AppOpsControllerImpl` and
//! `LocationControllerImpl` watch them), each app's shown by its own menu
//! bar item: the microphone and camera of the privacy indicators, and the
//! high-power location of the location icon. An indicator is on while the
//! app has any of its ops active, with any attribution tag; the
//! platform's own uids are not shown.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{Parcel, UNKNOWN_TRANSACTION};
use aim_host_display::notify::{Indicator, Message};
use aim_service_aidl::{
    com_android_internal_app_iappopsactivecallback as active,
    com_android_internal_app_iappopsservice as appops,
};

use super::Bridge;

/// `AppOpsManager` ops and what each shows.
const OPS: [(i32, Indicator); 5] = [
    (27, Indicator::Microphone),  // OP_RECORD_AUDIO
    (100, Indicator::Microphone), // OP_PHONE_CALL_MICROPHONE
    (26, Indicator::Camera),      // OP_CAMERA
    (101, Indicator::Camera),     // OP_PHONE_CALL_CAMERA
    (42, Indicator::Location),    // OP_MONITOR_HIGH_POWER_LOCATION
];
/// `Process.FIRST_APPLICATION_UID`: below it, the platform's own.
const FIRST_APPLICATION_UID: i32 = 10_000;

/// Who has an indicator's op active: its op, uid and attribution tag.
type Holder = (i32, i32, Option<String>);

/// The active ops, by package and indicator.
pub(super) type Active = HashMap<(String, Indicator), HashSet<Holder>>;

impl Bridge {
    /// Watches the ops, once AppOps is there (with NMS, in system_server).
    pub(super) fn watch_ops(self: &Arc<Self>) {
        let callback = self
            .process
            .add_service(Arc::new(ActiveListener(Arc::downgrade(self))));
        let watch = appops::StartWatchingActive {
            ops: Some(OPS.iter().map(|&(op, _)| op).collect()),
            callback: Some(callback),
        };
        if let Err(e) = self.call(
            "appops",
            appops::START_WATCHING_ACTIVE,
            |p| watch.write(p),
            appops::read_start_watching_active_reply,
        ) {
            eprintln!("guest-init: notifications: indicators: {e}");
        }
    }

    /// `op` became active or inactive for `package` of `uid`.
    fn op_active(&self, op: i32, uid: i32, package: String, tag: Option<String>, on: bool) {
        let Some(&(_, indicator)) = OPS.iter().find(|&&(o, _)| o == op) else {
            return;
        };
        if uid % 100_000 < FIRST_APPLICATION_UID {
            return;
        }
        let key = (package, indicator);
        let changed = update(&mut self.active.lock().unwrap(), &key, (op, uid, tag), on);
        if let Some(on) = changed {
            let (package, indicator) = key;
            self.send(&Message::Indicator {
                package,
                indicator,
                on,
            });
        }
    }

    /// AppOps died with system_server: every indicator goes off.
    pub(super) fn ops_gone(&self) {
        let active = std::mem::take(&mut *self.active.lock().unwrap());
        for (package, indicator) in active.into_keys() {
            self.send(&Message::Indicator {
                package,
                indicator,
                on: false,
            });
        }
    }
}

/// Records `holder`'s op of `key` as active or not; the indicator's new
/// state if it changed.
fn update(
    active: &mut Active,
    key: &(String, Indicator),
    holder: Holder,
    on: bool,
) -> Option<bool> {
    let holders = active.entry(key.clone()).or_default();
    let was = !holders.is_empty();
    if on {
        holders.insert(holder);
    } else {
        holders.remove(&holder);
    }
    let now = !holders.is_empty();
    if !now {
        active.remove(key);
    }
    (was != now).then_some(now)
}

/// The `IAppOpsActiveCallback` node.
struct ActiveListener(Weak<Bridge>);

impl Service for ActiveListener {
    fn descriptor(&self) -> &str {
        active::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(bridge) = self.0.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        if call.code != active::OP_ACTIVE_CHANGED {
            return Err(UNKNOWN_TRANSACTION);
        }
        let a = active::OpActiveChanged::read(&mut call.data)?;
        bridge.op_active(
            a.op,
            a.uid,
            a.package_name.unwrap_or_default(),
            a.attribution_tag,
            a.active,
        );
        Ok(Parcel::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_on_while_any_holder_is_active() {
        let mut active = Active::new();
        let key = ("com.example".to_string(), Indicator::Microphone);
        let (a, b) = ((27, 10_100, None), (27, 10_100, Some("voice".to_string())));
        assert_eq!(update(&mut active, &key, a.clone(), true), Some(true));
        assert_eq!(update(&mut active, &key, b.clone(), true), None);
        assert_eq!(update(&mut active, &key, a.clone(), true), None);
        assert_eq!(update(&mut active, &key, a, false), None);
        assert_eq!(update(&mut active, &key, b.clone(), false), Some(false));
        assert!(active.is_empty());
        assert_eq!(update(&mut active, &key, b, false), None);
        assert!(active.is_empty());
    }
}
