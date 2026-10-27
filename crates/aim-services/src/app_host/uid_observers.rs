//! Uids' states in the app host (`UidObserverController` and
//! `ActivityManagerService`'s uid queries at `android-16.0.0_r1`, ADR
//! 0013): a uid is active, at `PROCESS_STATE_TOP` with every capability,
//! while a process of it runs (the app host's processes are all top), and
//! gone with its last; `registerUidObserver` observers hear these changes
//! as `dispatchUidsChangedForObserver` tells them (those of
//! `registerUidObserverForUids` only of the uids they watch, which
//! `addUidToObserver` and `removeUidFromObserver` change), and `isUidActive` and
//! `getUidProcessState` answer from them. The app host's own services'
//! `addOnUidImportanceListener` listeners hear every change, since each
//! crosses any cutpoint: from none to top, and gone.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Strong};
use aim_binder_host::parcel::{Binder, Exception, Parcel};

use super::activity_user::AllowMode;
use super::{AppHost, PER_USER_RANGE, appops_service};

/// `ActivityManager.UID_OBSERVER_*`.
const UID_OBSERVER_PROCSTATE: i32 = 1 << 0;
const UID_OBSERVER_GONE: i32 = 1 << 1;
const UID_OBSERVER_ACTIVE: i32 = 1 << 3;
const UID_OBSERVER_CAPABILITY: i32 = 1 << 5;
/// `ActivityManager.PROCESS_STATE_*`, and `PROCESS_CAPABILITY_ALL`, a top
/// process's (`OomAdjuster.getDefaultCapability`).
const MIN_PROCESS_STATE: i32 = 0;
const PROCESS_STATE_TOP: i32 = 2;
const PROCESS_STATE_NONEXISTENT: i32 = 20;
const PROCESS_CAPABILITY_ALL: i32 = 0xff;
/// `IUidObserver`'s descriptor and its one-way calls.
const DESCRIPTOR: &str = "android.app.IUidObserver";
const ON_UID_GONE: u32 = 1;
const ON_UID_ACTIVE: u32 = 2;
const ON_UID_STATE_CHANGED: u32 = 4;
/// `AppOpsManager.OP_GET_USAGE_STATS`.
const OP_GET_USAGE_STATS: i32 = 43;

/// A uid's change (`UidRecord.CHANGE_*`).
#[derive(Clone, Copy)]
pub(super) enum Change {
    /// Its first process runs: active, top, every capability.
    Active,
    Gone,
}

/// `UidObserverRegistration`.
struct Observer {
    observer: Strong,
    death: u64,
    which: i32,
    cutpoint: i32,
    /// The states last reported, by uid, when there is a cutpoint.
    last: HashMap<i32, i32>,
    /// The uids it watches, ascending, else every uid (`mUids`).
    uids: Option<Vec<i32>>,
    /// Its token (`mToken`), with which uids are added and removed.
    token: Option<Binder>,
}

/// An `OnUidImportanceListener` of this process, told the uid and its
/// change.
pub(super) type ImportanceListener = Arc<dyn Fn(i32, Change) + Send + Sync>;

#[derive(Default)]
pub struct UidObservers {
    observers: Mutex<Vec<Observer>>,
    /// The importance listeners, by their registration.
    importance: Mutex<Vec<(u64, ImportanceListener)>>,
    next_importance: Mutex<u64>,
    /// The sequence of each state change (`procStateSeq`).
    seq: Mutex<i64>,
}

impl Observer {
    /// `dispatchUidsChangedForObserver` of `change` of `uid`.
    fn dispatch(&mut self, uid: i32, change: Change, seq: i64) {
        let call = |code: u32, write: &dyn Fn(&mut Parcel)| {
            let mut p = Parcel::new();
            p.write_interface_token(DESCRIPTOR);
            write(&mut p);
            if let Err(s) = self.observer.transact(code, &p, true) {
                eprintln!("app host: uid observer {code}: status {s}");
            }
        };
        match change {
            Change::Active => {
                if self.which & UID_OBSERVER_ACTIVE != 0 {
                    call(ON_UID_ACTIVE, &|p| p.write_i32(uid));
                }
                let mut report = false;
                if self.which & UID_OBSERVER_PROCSTATE != 0 {
                    report = true;
                    if self.cutpoint >= MIN_PROCESS_STATE {
                        report = match self.last.get(&uid) {
                            Some(&last) => {
                                (last <= self.cutpoint) != (PROCESS_STATE_TOP <= self.cutpoint)
                            }
                            None => true,
                        };
                    }
                }
                // Its capabilities change with it.
                report |= self.which & UID_OBSERVER_CAPABILITY != 0;
                if report {
                    if self.cutpoint >= MIN_PROCESS_STATE {
                        self.last.insert(uid, PROCESS_STATE_TOP);
                    }
                    call(ON_UID_STATE_CHANGED, &|p| {
                        p.write_i32(uid);
                        p.write_i32(PROCESS_STATE_TOP);
                        p.write_i64(seq);
                        p.write_i32(PROCESS_CAPABILITY_ALL);
                    });
                }
            }
            Change::Gone => {
                if self.which & UID_OBSERVER_GONE != 0 {
                    call(ON_UID_GONE, &|p| {
                        p.write_i32(uid);
                        p.write_bool(false);
                    });
                }
                self.last.remove(&uid);
            }
        }
    }
}

impl AppHost {
    /// `hasUsageStatsPermission`, else `PACKAGE_USAGE_STATS`, which uid
    /// queries need.
    fn enforce_usage_stats(
        &self,
        call: &Call<'_>,
        package: Option<&str>,
        method: &str,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        // The op allows it, or (by default, else enforced) the permission.
        if appops_service::note(self, OP_GET_USAGE_STATS, uid, package)? == 0
            || self.check_permission(Some("android.permission.PACKAGE_USAGE_STATS"), uid)? == 0
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Permission Denial: {method} from pid={}, uid={uid} requires android.permission.PACKAGE_USAGE_STATS",
            call.sender_pid
        )))
    }

    /// `registerUidObserver`: `observer` hears the changes `which` names,
    /// state changes only across `cutpoint` when it is one.
    pub(super) fn register_uid_observer(
        &self,
        call: &Call<'_>,
        observer: Option<Binder>,
        which: i32,
        cutpoint: i32,
        package: Option<&str>,
    ) -> Result<(), Exception> {
        self.enforce_usage_stats(call, package, "registerUidObserver")?;
        self.add_uid_observer(observer, which, cutpoint, None, None)
    }

    /// `registerUidObserverForUids`: as `registerUidObserver`, of `uids`
    /// only (every uid for none); its token, to add and remove uids with.
    pub(super) fn register_uid_observer_for_uids(
        &self,
        call: &Call<'_>,
        observer: Option<Binder>,
        which: i32,
        cutpoint: i32,
        package: Option<&str>,
        uids: Option<Vec<i32>>,
    ) -> Result<Binder, Exception> {
        self.enforce_usage_stats(call, package, "registerUidObserver")?;
        let token = self
            .process
            .add_service(std::sync::Arc::new(super::activity::Token));
        let uids = uids.map(|mut u| {
            u.sort_unstable();
            u
        });
        self.add_uid_observer(observer, which, cutpoint, uids, Some(token))?;
        Ok(token)
    }

    /// `addUidToObserver` (`add`) or `removeUidFromObserver`: the uids the
    /// observer of `token` watches, kept ascending (`UidObserverRegistration
    /// .addUid`, `removeUid`). An unknown token or an observer of every uid
    /// is left as it is.
    pub(super) fn change_observer_uids(
        &self,
        call: &Call<'_>,
        token: Option<Binder>,
        package: Option<&str>,
        uid: i32,
        add: bool,
    ) -> Result<(), Exception> {
        self.enforce_usage_stats(call, package, "registerUidObserver")?;
        let mut observers = self.uid_observers.observers.lock().unwrap();
        let Some(uids) = observers
            .iter_mut()
            .find(|o| token.is_some() && o.token == token)
            .and_then(|o| o.uids.as_mut())
        else {
            return Ok(());
        };
        match (uids.binary_search(&uid), add) {
            (Err(at), true) => uids.insert(at, uid),
            (Ok(at), false) => {
                uids.remove(at);
            }
            _ => {}
        }
        Ok(())
    }

    fn add_uid_observer(
        &self,
        observer: Option<Binder>,
        which: i32,
        cutpoint: i32,
        uids: Option<Vec<i32>>,
        token: Option<Binder>,
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = observer else {
            return Err(Exception::illegal_argument(
                "registerUidObserver: no observer",
            ));
        };
        let observer = self.process.strong(handle);
        let host = self.me.clone();
        let death = self.process.link_to_death(
            &observer,
            Box::new(move || {
                if let Some(host) = host.upgrade() {
                    host.uid_observers
                        .observers
                        .lock()
                        .unwrap()
                        .retain(|o| o.observer.handle != handle);
                }
            }),
        );
        self.uid_observers.observers.lock().unwrap().push(Observer {
            observer,
            death,
            which,
            cutpoint,
            last: HashMap::new(),
            uids,
            token,
        });
        Ok(())
    }

    /// `ActivityManager.addOnUidImportanceListener` in this process: its
    /// registration, to remove it with.
    pub(super) fn add_uid_importance_listener(&self, listener: ImportanceListener) -> u64 {
        let mut next = self.uid_observers.next_importance.lock().unwrap();
        *next += 1;
        self.uid_observers
            .importance
            .lock()
            .unwrap()
            .push((*next, listener));
        *next
    }

    /// `removeOnUidImportanceListener`.
    pub(super) fn remove_uid_importance_listener(&self, id: u64) {
        self.uid_observers
            .importance
            .lock()
            .unwrap()
            .retain(|(i, _)| *i != id);
    }

    /// `unregisterUidObserver`.
    pub(super) fn unregister_uid_observer(&self, observer: Option<Binder>) {
        let Some(Binder::Handle(handle)) = observer else {
            return;
        };
        let mut observers = self.uid_observers.observers.lock().unwrap();
        if let Some(i) = observers.iter().position(|o| o.observer.handle == handle) {
            let o = observers.remove(i);
            self.process.clear_death(&o.observer, o.death);
        }
    }

    /// Whether a process of `uid` runs (its `UidRecord`, never idle here).
    pub(super) fn uid_runs(&self, uid: i32) -> bool {
        self.apps
            .lock()
            .unwrap()
            .values()
            .any(|a| a.uid as i32 == uid)
    }

    /// `isUidActive`.
    pub(super) fn is_uid_active(
        &self,
        call: &Call<'_>,
        uid: i32,
        package: Option<&str>,
    ) -> Result<bool, Exception> {
        self.enforce_usage_stats(call, package, "isUidActive")?;
        Ok(self.uid_runs(uid))
    }

    /// `getUidProcessState`.
    pub(super) fn uid_process_state(
        &self,
        call: &Call<'_>,
        uid: i32,
        package: Option<&str>,
    ) -> Result<i32, Exception> {
        self.enforce_usage_stats(call, package, "getUidProcessState")?;
        self.handle_incoming_user(
            call.sender_euid as i32,
            uid / PER_USER_RANGE as i32,
            false,
            AllowMode::FullOnly,
            Some("getUidProcessState"),
            package,
        )?;
        Ok(if self.uid_runs(uid) {
            PROCESS_STATE_TOP
        } else {
            PROCESS_STATE_NONEXISTENT
        })
    }

    /// `uid` changed: its observers are told.
    pub(super) fn uid_changed(&self, uid: i32, change: Change) {
        let seq = {
            let mut seq = self.uid_observers.seq.lock().unwrap();
            *seq += 1;
            *seq
        };
        // `isWatchingUid`.
        for o in self
            .uid_observers
            .observers
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|o| {
                o.uids
                    .as_ref()
                    .is_none_or(|u| u.binary_search(&uid).is_ok())
            })
        {
            o.dispatch(uid, change, seq);
        }
        let listeners: Vec<ImportanceListener> = (self.uid_observers.importance.lock().unwrap())
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(uid, change);
        }
        // `noteUidProcessState` as a uid's first process comes
        // (`addProcessNameLocked`) and its last goes
        // (`removeProcessNameLocked`); in between it stays top.
        if let Some(stats) = self.battery_stats.get() {
            stats.note_uid_process_state(
                uid,
                match change {
                    Change::Active => PROCESS_STATE_TOP,
                    Change::Gone => PROCESS_STATE_NONEXISTENT,
                },
            );
        }
        // HintManagerService's own observer (`MyUidObserver`): a uid's
        // process state only ever is top, so only its going matters.
        if let (Change::Gone, Some(hints)) = (change, self.hint_manager.get()) {
            hints.uid_gone(uid);
        }
        // AppOpsService's own observer: the uid crosses the foreground
        // either way.
        appops_service::uid_state_changed(self, uid);
    }
}
