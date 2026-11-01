//! SystemServiceManager's onUserUnlocking precedes UserController's USER_UNLOCKED.
//! The system process subscribes before entering its main looper and replays the snapshot.
use super::AppHost;
use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Binder, DEAD_OBJECT, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_isystemuserlifecycle as lifecycle;

#[derive(Default)]
pub(super) struct State {
    generation: i64,
    owner: Option<Strong>,
    death: Option<u64>,
}

impl AppHost {
    pub(super) fn before_user_unlocked(&self, broadcast: impl FnOnce()) -> Result<(), String> {
        self.deliver_user_unlocking(0)?;
        broadcast();
        Ok(())
    }

    pub(super) fn attach_system_user_lifecycle(
        &self,
        pid: i32,
        generation: i64,
        owner: Option<Binder>,
    ) -> Result<bool, Exception> {
        let apps = self.apps.lock().unwrap();
        if !apps.get(&generation).is_some_and(|app| {
            !app.removed
                && app.process == super::SYSTEM_PROCESS
                && app.uid == super::SYSTEM_UID
                && app.pid == pid
                && app.thread.is_some()
        }) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "system lifecycle generation unavailable",
            ));
        }
        let Some(Binder::Handle(handle)) = owner else {
            return Err(Exception::illegal_argument(
                "system lifecycle must be remote",
            ));
        };
        let mut state = self.user_lifecycle.lock().unwrap();
        if generation <= state.generation {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "system lifecycle already attached",
            ));
        }
        if let (Some(owner), Some(death)) = (state.owner.take(), state.death.take()) {
            self.process.clear_death(&owner, death);
        }
        let owner = self.process.strong(handle);
        let host = self.me.clone();
        let death = self.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(host) = host.upgrade() {
                    host.system_user_lifecycle_died(generation);
                }
            }),
        );
        *state = State {
            generation,
            owner: Some(owner),
            death: Some(death),
        };
        self.user_unlocking_or_unlocked(0)
    }

    pub(super) fn system_user_lifecycle_died(&self, generation: i64) {
        let mut state = self.user_lifecycle.lock().unwrap();
        if state.generation == generation {
            if let (Some(owner), Some(death)) = (state.owner.take(), state.death.take()) {
                self.process.clear_death(&owner, death);
            }
        }
    }

    pub(super) fn deliver_user_unlocking(&self, user: i32) -> Result<(), String> {
        let target = {
            let state = self.user_lifecycle.lock().unwrap();
            state
                .owner
                .as_ref()
                .map(|owner| (state.generation, self.process.strong(owner.handle)))
        };
        if let Some((generation, owner)) = target {
            let mut p = Parcel::new();
            lifecycle::OnUserUnlocking { user_id: user }.write(&mut p);
            // No native lock spans Java, whose lifecycle can reenter native owners.
            match owner.transact(lifecycle::ON_USER_UNLOCKING, &p, false) {
                Ok(reply) => lifecycle::read_on_user_unlocking_reply(&mut reply.reader())
                    .map_err(|e| format!("user unlocking reply: {e}"))?
                    .map_err(|e| e.message)?,
                Err(DEAD_OBJECT) => self.system_user_lifecycle_died(generation),
                Err(e) => return Err(format!("user unlocking: {e}")),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::EX_SECURITY;
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    use std::sync::{Arc, Mutex};

    struct Lifecycle(Arc<Mutex<Vec<&'static str>>>);
    impl Service for Lifecycle {
        fn descriptor(&self) -> &str {
            lifecycle::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, lifecycle::ON_USER_UNLOCKING);
            assert_eq!(lifecycle::OnUserUnlocking::read(&mut call.data)?.user_id, 0);
            self.0.lock().unwrap().push("lifecycle");
            let mut p = Parcel::new();
            lifecycle::write_on_user_unlocking_reply(&mut p);
            Ok(p)
        }
    }
    fn attach(
        host: &Arc<AppHost>,
        pid: i32,
        generation: i64,
        owner: Binder,
        uid: u32,
    ) -> Result<bool, Exception> {
        let mut data = Parcel::new();
        sp::AttachSystemUserLifecycle {
            generation,
            lifecycle: Some(owner),
        }
        .write(&mut data);
        let reply = super::super::system_process::SystemProcess::new(host.clone())
            .transact(&mut Call {
                code: sp::ATTACH_SYSTEM_USER_LIFECYCLE,
                flags: 0,
                sender_pid: pid,
                sender_euid: uid,
                data: data.reader(),
            })
            .unwrap();
        sp::read_attach_system_user_lifecycle_reply(&mut reply.reader()).unwrap()
    }
    #[test]
    fn lifecycle_reply_precedes_broadcast_and_restart_replays_unlocked_snapshot() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            super::super::sync_owner::tests::install(
                host,
                export(super::super::test_activity_service(host)),
            );
            let events = Arc::new(Mutex::new(Vec::new()));
            let owner = export(Arc::new(Lifecycle(events.clone())));
            host.set_user_unlocked_for_package_test(0, false);
            assert_eq!(
                attach(host, 42, 100, owner.binder(), 10100)
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
            assert!(attach(host, 43, 100, owner.binder(), 1000).is_err());
            assert!(!attach(host, 42, 100, owner.binder(), 1000).unwrap());
            assert!(attach(host, 42, 100, owner.binder(), 1000).is_err());
            host.set_user_unlocked_for_package_test(0, true);
            host.before_user_unlocked(|| events.lock().unwrap().push("broadcast"))
                .unwrap();
            assert_eq!(*events.lock().unwrap(), ["lifecycle", "broadcast"]);
            // Death between lifecycle delivery and broadcast; replacement starts already unlocked.
            host.system_user_lifecycle_died(100);
            let mut apps = host.apps.lock().unwrap();
            let mut app = apps.remove(&100).unwrap();
            app.pid = 43;
            apps.insert(101, app);
            drop(apps);
            assert!(attach(host, 42, 100, owner.binder(), 1000).is_err());
            assert!(attach(host, 43, 101, owner.binder(), 1000).unwrap());
            host.system_user_lifecycle_died(100); // Stale death cannot detach the replacement.
            host.before_user_unlocked(|| events.lock().unwrap().push("broadcast"))
                .unwrap();
            assert_eq!(
                *events.lock().unwrap(),
                ["lifecycle", "broadcast", "lifecycle", "broadcast"]
            );
        });
    }

    #[test]
    fn binder_death_between_delivery_and_dispatch_does_not_block_broadcast() {
        crate::system::with_discovery_binders_lifecycle(&[], true, |host, _, _, export, die| {
            super::super::sync_owner::tests::install(
                host,
                export(super::super::test_activity_service(host)),
            );
            let events = Arc::new(Mutex::new(Vec::new()));
            let owner = export(Arc::new(Lifecycle(events.clone())));
            attach(host, 42, 100, owner.binder(), 1000).unwrap();
            host.set_user_unlocked_for_package_test(0, true);
            host.before_user_unlocked(|| {
                die();
                events.lock().unwrap().push("broadcast");
            })
            .unwrap();
            assert_eq!(*events.lock().unwrap(), ["lifecycle", "broadcast"]);
            host.before_user_unlocked(|| events.lock().unwrap().push("after-death"))
                .unwrap();
            assert_eq!(events.lock().unwrap().last(), Some(&"after-death"));
        });
    }
}
