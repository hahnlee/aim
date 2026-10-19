//! ActivityManagerService's unsafe intent strict mode callbacks
//! (`registerStrictModeCallback`, `triggerUnsafeIntentStrictMode` at
//! `android-16.0.0_r1`): a process's `IUnsafeIntentStrictModeCallback`,
//! kept until it dies, told each unsafe intent event the resolution of its
//! activity or service start reports (`SaferIntentUtils.reportUnsafeIntentEvent`),
//! with the intent's filter (`cloneFilter`). Broadcasts' events are not
//! reported yet (#1715).
//!
//! Background activity launch strict mode callbacks
//! (`BackgroundActivityStartController.addStrictModeCallback`), by uid,
//! kept until they die: the controller tells them of a launch it aborts.
//! The app host's processes are all top (#1738), so none of their
//! launches is a background one to abort, and none is told.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Binder, Parcel};
use aim_service_aidl::WriteParcelable;

use super::AppHost;
use super::kept::RawIntent;

/// `IUnsafeIntentStrictModeCallback`'s descriptor and `onUnsafeIntent`.
const DESCRIPTOR: &str = "android.app.IUnsafeIntentStrictModeCallback";
const ON_UNSAFE_INTENT: u32 = 1;

/// `mStrictModeCallbacks`: each process's callback and its death link, by
/// pid.
pub(super) type Callbacks = Arc<Mutex<HashMap<i32, (Strong, u64)>>>;

/// `mStrictModeBalCallbacks`: each callback and its death link, by uid and
/// binder.
pub(super) type BalCallbacks = Arc<Mutex<HashMap<(i32, u32), (Strong, u64)>>>;

impl AppHost {
    /// `registerStrictModeCallback` of the process `pid`.
    pub(super) fn register_strict_mode_callback(&self, pid: i32, callback: Option<Binder>) {
        let Some(Binder::Handle(handle)) = callback else {
            return;
        };
        let strong = self.process.strong(handle);
        let callbacks = Arc::downgrade(&self.strict_mode_callbacks);
        let process = self.process.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(callbacks) = callbacks.upgrade()
                    && let Some((strong, death)) = callbacks.lock().unwrap().remove(&pid)
                {
                    process.clear_death(&strong, death);
                }
            }),
        );
        if let Some((old, old_death)) = self
            .strict_mode_callbacks
            .lock()
            .unwrap()
            .insert(pid, (strong, death))
        {
            self.process.clear_death(&old, old_death);
        }
    }

    /// `addStrictModeCallback` of `uid`: false if it was added already.
    pub(super) fn add_bal_strict_mode_callback(&self, uid: i32, callback: Option<Binder>) -> bool {
        let Some(Binder::Handle(handle)) = callback else {
            return false;
        };
        if self
            .bal_strict_mode_callbacks
            .lock()
            .unwrap()
            .contains_key(&(uid, handle))
        {
            return false;
        }
        let strong = self.process.strong(handle);
        let callbacks = Arc::downgrade(&self.bal_strict_mode_callbacks);
        let process = self.process.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(callbacks) = callbacks.upgrade()
                    && let Some((strong, death)) = callbacks.lock().unwrap().remove(&(uid, handle))
                {
                    process.clear_death(&strong, death);
                }
            }),
        );
        self.bal_strict_mode_callbacks
            .lock()
            .unwrap()
            .insert((uid, handle), (strong, death));
        true
    }

    /// `removeStrictModeCallback` of `uid`.
    pub(super) fn remove_bal_strict_mode_callback(&self, uid: i32, callback: Option<Binder>) {
        if let Some(Binder::Handle(handle)) = callback
            && let Some((strong, death)) = self
                .bal_strict_mode_callbacks
                .lock()
                .unwrap()
                .remove(&(uid, handle))
        {
            self.process.clear_death(&strong, death);
        }
    }

    /// `triggerUnsafeIntentStrictMode` of each event of a start of
    /// `intent` by the process `pid`, if it registered a callback.
    pub(super) fn report_unsafe_intent(
        &self,
        pid: Option<i32>,
        intent: &RawIntent,
        events: Vec<(i32, bool)>,
    ) {
        let Some(pid) = pid else { return };
        if events.is_empty() {
            return;
        }
        let Some(handle) = self
            .strict_mode_callbacks
            .lock()
            .unwrap()
            .get(&pid)
            .map(|(s, _)| s.handle)
        else {
            return;
        };
        let filter = match intent.clone_filter() {
            Ok(filter) => filter,
            Err(e) => return eprintln!("app host: strict mode: cloneFilter: {e:?}"),
        };
        for (event, _blocked) in events {
            let mut p = Parcel::new();
            p.write_interface_token(DESCRIPTOR);
            p.write_i32(event);
            p.write_i32(1);
            filter.write_to(&mut p);
            if self
                .process
                .transact(handle, ON_UNSAFE_INTENT, &p, true)
                .is_err()
                && let Some((strong, death)) =
                    self.strict_mode_callbacks.lock().unwrap().remove(&pid)
            {
                self.process.clear_death(&strong, death);
                return;
            }
        }
    }
}
