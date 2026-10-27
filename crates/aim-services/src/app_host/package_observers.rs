//! `NativePackageObserverOwner` and `PackageObserverHelper`
//! (`android-16.0.0_r1`) for the app host's native service subscribers.
//! Membership uses Arc identity, ordered by that identity as Java's ArraySet
//! orders these identity-hashed observers. Each notification retains one
//! immutable membership snapshot; callbacks run without registry or package
//! publication locks. Revocation cannot retract an in-flight snapshot.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::dev_aim_server_ipackageobservereventsbridge as events;

use super::{AppHost, SYSTEM_UID, reply};

/// `PackageManagerInternal.PackageListObserver`: uid carries the affected
/// user, rather than a separate user filter. Failures propagate to publication.
pub trait PackageListObserver: Send + Sync {
    fn package_added(&self, name: Option<&str>, uid: i32) -> Result<(), Exception>;
    fn package_changed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception>;
    fn package_removed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception>;
}

/// Discovery owners consume the canonical session's add/update/remove feed too.
/// Guest broadcasts are delivered separately by the package broadcast owner.
pub(super) struct Discoveries(pub Weak<AppHost>);
impl Discoveries {
    fn changed(&self, name: Option<&str>, uid: i32, removed: bool) -> Result<(), Exception> {
        let host = self
            .0
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host gone"))?;
        let name = name.ok_or_else(|| Exception::illegal_argument("package name absent"))?;
        if uid < 0 {
            return Err(Exception::illegal_argument("package UID absent"));
        }
        host.notify_package_discoveries(&super::package_writes::PackageChanged {
            package: name,
            user: uid / super::PER_USER_RANGE as i32,
            components: &[name.to_owned()],
            removed,
        })
    }
}
impl PackageListObserver for Discoveries {
    fn package_added(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.changed(name, uid, false)
    }
    fn package_changed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.changed(name, uid, false)
    }
    fn package_removed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.changed(name, uid, true)
    }
}

#[derive(Default)]
struct Membership {
    closed: bool,
    observers: Arc<Vec<Arc<dyn PackageListObserver>>>,
}

/// One bootstrap epoch's local service registry (`PackageObserverHelper`).
#[derive(Default)]
pub struct Registry(Mutex<Membership>);

fn closed() -> Exception {
    Exception::new(EX_ILLEGAL_STATE, "native package observer epoch closed")
}

fn identity(observer: &Arc<dyn PackageListObserver>) -> usize {
    Arc::as_ptr(observer) as *const () as usize
}

impl Registry {
    fn snapshot(&self) -> Result<Arc<Vec<Arc<dyn PackageListObserver>>>, Exception> {
        let state = self.0.lock().unwrap();
        if state.closed {
            return Err(closed());
        }
        Ok(state.observers.clone())
    }

    /// Adding the same identity twice leaves one member.
    pub fn add_observer(&self, observer: Arc<dyn PackageListObserver>) -> Result<(), Exception> {
        let mut state = self.0.lock().unwrap();
        if state.closed {
            return Err(closed());
        }
        let observers = Arc::make_mut(&mut state.observers);
        if let Err(at) = observers.binary_search_by_key(&identity(&observer), identity) {
            observers.insert(at, observer);
        }
        Ok(())
    }

    fn notify(&self, event: Event, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        let snapshot = self.snapshot()?;
        for observer in snapshot.iter() {
            match event {
                Event::Added => observer.package_added(name, uid)?,
                Event::Changed => observer.package_changed(name, uid)?,
                Event::Removed => observer.package_removed(name, uid)?,
            }
        }
        Ok(())
    }

    /// Revoke this epoch only. Already captured notifications may finish.
    pub fn close(&self) {
        let previous = {
            let mut state = self.0.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.observers)
        };
        drop(previous);
    }
}

#[derive(Clone, Copy)]
enum Event {
    Added,
    Changed,
    Removed,
}

pub(super) struct Events(pub Arc<Registry>);
impl Service for Events {
    fn descriptor(&self) -> &str {
        events::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "native package observer transport requires system UID",
            )));
        }
        let (event, name, uid) = match call.code {
            events::PACKAGE_ADDED => {
                let a = events::PackageAdded::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (Event::Added, a.package_name, a.uid)
            }
            events::PACKAGE_CHANGED => {
                let a = events::PackageChanged::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (Event::Changed, a.package_name, a.uid)
            }
            events::PACKAGE_REMOVED => {
                let a = events::PackageRemoved::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (Event::Removed, a.package_name, a.uid)
            }
            events::REVOKE => {
                events::Revoke::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.close();
                let mut p = Parcel::new();
                events::write_revoke_reply(&mut p);
                return Ok(p);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(self.0.notify(event, name.as_deref(), uid).map(|()| {
            let mut p = Parcel::new();
            p.write_no_exception();
            p
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::ServiceOwner;
    use aim_binder_host::parcel::{Binder, EX_SECURITY};

    type Record = (usize, &'static str, Option<String>, i32);
    #[derive(Default)]
    struct Observer {
        id: usize,
        log: Arc<Mutex<Vec<Record>>>,
        action: Mutex<Option<Box<dyn FnOnce() -> Result<(), Exception> + Send>>>,
    }
    impl Observer {
        fn event(&self, kind: &'static str, name: Option<&str>, uid: i32) -> Result<(), Exception> {
            self.log
                .lock()
                .unwrap()
                .push((self.id, kind, name.map(str::to_owned), uid));
            let action = self.action.lock().unwrap().take();
            if let Some(action) = action {
                action()?;
            }
            Ok(())
        }
    }
    impl PackageListObserver for Observer {
        fn package_added(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
            self.event("added", name, uid)
        }
        fn package_changed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
            self.event("changed", name, uid)
        }
        fn package_removed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
            self.event("removed", name, uid)
        }
    }

    fn observers() -> (Vec<Arc<Observer>>, Arc<Mutex<Vec<Record>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut observers: Vec<_> = (0..3)
            .map(|id| {
                Arc::new(Observer {
                    id,
                    log: log.clone(),
                    action: Mutex::default(),
                })
            })
            .collect();
        observers.sort_by_key(|o| identity(&(o.clone() as Arc<dyn PackageListObserver>)));
        (observers, log)
    }

    #[test]
    fn package_observers_membership_and_delivery_order() {
        let registry = Arc::new(Registry::default());
        let (observers, log) = observers();
        // ArraySet iteration is identity-hash order, not registration order.
        for observer in observers.iter().rev() {
            registry.add_observer(observer.clone()).unwrap();
            registry.add_observer(observer.clone()).unwrap();
        }
        assert_eq!(registry.snapshot().unwrap().len(), 3);
        for (event, name, uid) in [
            (Event::Added, Some("a.b"), 10123),
            (Event::Changed, Some("c.d"), 1010123),
            (Event::Removed, None, -1),
        ] {
            registry.notify(event, name, uid).unwrap();
        }
        let expected: Vec<_> = [
            ("added", Some("a.b"), 10123),
            ("changed", Some("c.d"), 1010123),
            ("removed", None, -1),
        ]
        .into_iter()
        .flat_map(|(kind, name, uid)| {
            observers
                .iter()
                .map(move |o| (o.id, kind, name.map(str::to_owned), uid))
        })
        .collect();
        assert_eq!(*log.lock().unwrap(), expected);
    }

    #[test]
    fn package_observers_reentrant_mutation_uses_one_membership_snapshot() {
        let registry = Arc::new(Registry::default());
        let (observers, log) = observers();
        for o in &observers {
            registry.add_observer(o.clone()).unwrap();
        }
        let added = Arc::new(Observer {
            id: 99,
            log: log.clone(),
            action: Mutex::default(),
        });
        let current = registry.clone();
        let new = added.clone();
        *observers[0].action.lock().unwrap() = Some(Box::new(move || {
            current.add_observer(new)?;
            // Nested dispatch completes without holding any registry lock.
            current.notify(Event::Changed, Some("nested"), 1010123)
        }));
        registry.notify(Event::Added, Some("outer"), 10123).unwrap();
        let records = log.lock().unwrap();
        let outer: Vec<_> = records
            .iter()
            .filter(|r| r.1 == "added")
            .map(|r| r.0)
            .collect();
        assert_eq!(outer, observers.iter().map(|o| o.id).collect::<Vec<_>>());
        let nested: Vec<_> = records
            .iter()
            .filter(|r| r.1 == "changed")
            .map(|r| r.0)
            .collect();
        let snapshot = registry.snapshot().unwrap();
        assert_eq!(nested.len(), snapshot.len());
        assert!(nested.contains(&99));
        assert!(observers.iter().all(|o| nested.contains(&o.id)));
        assert_eq!(records.first().unwrap().1, "added");
        assert_eq!(records.last().unwrap().1, "added");
    }

    #[test]
    fn package_observers_revocation_finishes_captured_delivery_and_closes_epoch() {
        let registry = Arc::new(Registry::default());
        let (observers, log) = observers();
        for o in &observers {
            registry.add_observer(o.clone()).unwrap();
        }
        let current = registry.clone();
        *observers[0].action.lock().unwrap() = Some(Box::new(move || {
            current.close();
            current.close();
            Ok(())
        }));
        registry
            .notify(Event::Removed, Some("a.b"), 1010123)
            .unwrap();
        assert_eq!(log.lock().unwrap().len(), 3);
        assert_eq!(
            registry.notify(Event::Added, Some("a.b"), 10123),
            Err(closed())
        );
        assert_eq!(registry.add_observer(observers[0].clone()), Err(closed()));
        let next = Arc::new(Registry::default());
        next.add_observer(observers[0].clone()).unwrap();
        assert!(next.notify(Event::Added, Some("a.b"), 10123).is_ok());
    }

    #[test]
    fn package_observers_bridge_transport_and_epoch() {
        crate::system::with_discovery_session(|host, _, _| bridge_transport(host));
    }

    fn bridge_transport(host: &Arc<AppHost>) {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
        let process = host.process.clone();
        for relative in [
            "misc/apexdata/com.android.permission/access.abx",
            "misc_de/0/apexdata/com.android.permission/access.abx",
        ] {
            let path = host.data().unwrap().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let root =
                aim_android_xml::read(b"<access><permissions/><app-id-permissions/></access>")
                    .unwrap();
            std::fs::write(path, aim_android_xml::abx::write(&root).unwrap()).unwrap();
        }
        assert!(
            host.permission_manager
                .set(Arc::new(
                    super::super::permission_manager::PermissionManager::new(host.clone())
                ))
                .is_ok()
        );
        let bridge = super::super::package_bootstrap::PackageBootstrap::new(host.clone());
        let registry = host.package_observers.lock().unwrap().upgrade().unwrap();
        let observer = Arc::new(Observer::default());
        registry.add_observer(observer.clone()).unwrap();
        let get = || {
            let mut p = Parcel::new();
            bootstrap::GetPackageObserverEventsBridge {}.write(&mut p);
            let response = bridge
                .transact(&mut Call {
                    code: bootstrap::GET_PACKAGE_OBSERVER_EVENTS_BRIDGE,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: SYSTEM_UID,
                    data: p.reader(),
                })
                .unwrap();
            let mut r = response.reader();
            let binder = bootstrap::read_get_package_observer_events_bridge_reply(&mut r)
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(r.remaining(), 0);
            binder
        };
        let binder = get();
        assert_eq!(get(), binder);
        let Binder::Local(ptr) = binder else {
            unreachable!()
        };
        let owner = crate::package::observers::Owner::new(ServiceOwner::Local(
            process.local_service(ptr).unwrap(),
        ));
        let params = host.tasks.launch_params(host.data().unwrap()).unwrap();
        let saved = host
            .data()
            .unwrap()
            .join("system_ce/0/launch_params/a.b-.Main.xml");
        std::fs::create_dir_all(saved.parent().unwrap().parent().unwrap()).unwrap();
        params
            .save(
                0,
                &crate::package::preferred::unflatten("a.b/.Main").unwrap(),
                super::super::launch_params::Params {
                    windowing_mode: 5,
                    ..Default::default()
                },
            )
            .unwrap();
        params.flush();
        assert!(saved.exists());
        owner.added("a.b", 10123).unwrap();
        owner.changed("a.b", 1010123).unwrap();
        assert!(saved.exists());
        owner.removed("a.b", 1010123).unwrap();
        params.flush();
        assert!(!saved.exists());
        assert_eq!(
            *observer.log.lock().unwrap(),
            vec![
                (0, "added", Some("a.b".into()), 10123),
                (0, "changed", Some("a.b".into()), 1010123),
                (0, "removed", Some("a.b".into()), 1010123),
            ]
        );
        // Transport authorization precedes decoding and closed-epoch checks.
        for uid in [0, 2000, 10123, 1001000] {
            for code in [
                events::PACKAGE_ADDED,
                events::PACKAGE_CHANGED,
                events::PACKAGE_REMOVED,
                events::REVOKE,
            ] {
                let p = Parcel::new();
                let p = Events(registry.clone())
                    .transact(&mut Call {
                        code,
                        flags: 0,
                        sender_pid: 1,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap();
                let error = p.reader().read_exception().unwrap().unwrap_err();
                assert_eq!(error.code, EX_SECURITY);
                assert_eq!(
                    error.message,
                    "native package observer transport requires system UID"
                );
            }
        }
        let error = Exception::new(EX_ILLEGAL_STATE, "subscriber failed");
        *observer.action.lock().unwrap() = Some(Box::new({
            let e = error.clone();
            move || Err(e)
        }));
        assert_eq!(owner.changed("a.b", 10123), Err(error));
        owner.close().unwrap();
        owner.close().unwrap();
        assert_eq!(
            owner.added("a.b", 10123).unwrap_err().message,
            "package observer epoch closed"
        );
        assert_eq!(
            registry.notify(Event::Changed, Some("a.b"), 10123),
            Err(closed())
        );
        // A retained old leaf cannot dispatch to the replacement registry.
        let _next_bridge = super::super::package_bootstrap::PackageBootstrap::new(host.clone());
        let next = host.package_observers.lock().unwrap().upgrade().unwrap();
        next.add_observer(observer.clone()).unwrap();
        let mut p = Parcel::new();
        events::PackageAdded {
            package_name: Some("old".into()),
            uid: 10123,
        }
        .write(&mut p);
        let p = process
            .local_service(ptr)
            .unwrap()
            .transact(events::PACKAGE_ADDED, &p, false)
            .unwrap();
        assert_eq!(p.reader().read_exception().unwrap(), Err(closed()));
        assert_eq!(next.snapshot().unwrap().len(), 4); // discovery, permission policy, launch params
    }
}
