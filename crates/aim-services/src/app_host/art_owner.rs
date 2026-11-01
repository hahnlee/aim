//! Construction acknowledgement only; no maintenance/lifecycle callers are routed yet.
use super::{AppHost, SYSTEM_PROCESS, SYSTEM_UID};
use crate::package::boot_captures::Captures;
use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    generation: i64,
    owner: Option<Attachment>,
}
struct Attachment {
    epoch: Arc<Captures>,
    process: Arc<LocalProcess>,
    strong: Strong,
    death: u64,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        self.process.clear_death(&self.strong, self.death);
    }
}
fn unavailable() -> Exception {
    Exception::new(EX_ILLEGAL_STATE, "ART owner generation unavailable (#2063)")
}
impl AppHost {
    pub(super) fn art_owner_generation(&self, pid: i32) -> Result<i64, Exception> {
        self.apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| {
                !a.removed
                    && a.process == SYSTEM_PROCESS
                    && a.uid == SYSTEM_UID
                    && a.pid == pid
                    && a.thread.is_some()
            })
            .map(|(generation, _)| *generation)
            .ok_or_else(unavailable)
    }
    pub(super) fn attach_art_owner(
        &self,
        pid: i32,
        generation: i64,
        epoch: Arc<Captures>,
        owner: Option<Binder>,
    ) -> Result<(), Exception> {
        epoch.current()?;
        // Neither publication nor acknowledgement calls Java or takes the install lock.
        let apps = self.apps.lock().unwrap();
        if !apps.get(&generation).is_some_and(|a| {
            !a.removed
                && a.process == SYSTEM_PROCESS
                && a.uid == SYSTEM_UID
                && a.pid == pid
                && a.thread.is_some()
        }) {
            return Err(unavailable());
        }
        let Some(Binder::Handle(handle)) = owner else {
            return Err(Exception::illegal_argument(
                "ART deletion owner must be remote",
            ));
        };
        let mut state = self.art_owner.lock().unwrap();
        if generation <= state.generation {
            return Err(unavailable());
        }
        let strong = self.process.strong(handle);
        let host = self.me.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(host) = host.upgrade() {
                    host.art_owner_died(generation);
                }
            }),
        );
        *state = State {
            generation,
            owner: Some(Attachment {
                epoch,
                process: self.process.clone(),
                strong,
                death,
            }),
        };
        Ok(())
    }
    pub(super) fn check_art_owner(
        &self,
        pid: i32,
        generation: i64,
        epoch: &Arc<Captures>,
    ) -> Result<(), Exception> {
        if let Err(error) = epoch.current() {
            self.art_owner_died(generation);
            return Err(error);
        }
        if self.art_owner_generation(pid)? != generation {
            return Err(unavailable());
        }
        let state = self.art_owner.lock().unwrap();
        if state.generation == generation
            && state
                .owner
                .as_ref()
                .is_some_and(|a| Arc::ptr_eq(&a.epoch, epoch))
        {
            Ok(())
        } else {
            Err(unavailable())
        }
    }
    pub(super) fn art_owner_died(&self, generation: i64) {
        let mut state = self.art_owner.lock().unwrap();
        if state.generation == generation {
            state.owner.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::Parcel;
    use aim_service_aidl::dev_aim_server_idexusepackages as api;
    struct NoCalls;
    impl Service for NoCalls {
        fn descriptor(&self) -> &str {
            "dev.aim.server.IArtDeletion"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            panic!("construction called Java")
        }
    }
    #[test]
    fn retired_package_epoch_revokes_art_readiness() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let system = host.package_system().unwrap();
            let bridge = system.package_bootstrap().unwrap();
            let epoch = Arc::new(Captures::new(system, bridge.clone()));
            let owner = export(Arc::new(NoCalls));
            super::super::sync_owner::tests::install(host, export(Arc::new(NoCalls)));
            host.attach_art_owner(42, 100, epoch.clone(), Some(owner.binder()))
                .unwrap();
            system.detach_package_bootstrap(&bridge).unwrap();
            assert!(host.check_art_owner(42, 100, &epoch).is_err());
            assert!(host.art_owner.lock().unwrap().owner.is_none());
            assert!(
                host.attach_art_owner(42, 100, epoch, Some(owner.binder()))
                    .is_err()
            );
        });
    }
    #[test]
    fn art_owner_guards_identity_epoch_and_death_without_install_lock_callbacks() {
        crate::system::with_discovery_binders_lifecycle(
            &[],
            true,
            |host, publish, _, export, die| {
                let system = host.package_system().unwrap();
                let bridge = system.package_bootstrap().unwrap();
                let epoch = Arc::new(Captures::new(system, bridge.clone()));
                let backend = super::super::package_maintenance::DexUsePackages(
                    epoch.clone(),
                    host.me.clone(),
                );
                let owner = export(Arc::new(NoCalls));
                assert!(
                    host.attach_art_owner(42, 100, epoch.clone(), Some(owner.binder()))
                        .is_err()
                );
                super::super::sync_owner::tests::install(host, export(Arc::new(NoCalls)));
                for uid in [0, 10100, 1000] {
                    let mut request = Parcel::new();
                    api::GetArtOwnerGeneration {}.write(&mut request);
                    let reply = backend
                        .transact(&mut Call {
                            code: api::GET_ART_OWNER_GENERATION,
                            flags: 0,
                            sender_pid: 42,
                            sender_euid: uid,
                            data: request.reader(),
                        })
                        .unwrap();
                    let result =
                        api::read_get_art_owner_generation_reply(&mut reply.reader()).unwrap();
                    assert_eq!(result.is_ok(), uid == 1000);
                    let mut request = Parcel::new();
                    api::AttachArtOwner {
                        generation: 99,
                        owner: Some(owner.binder()),
                    }
                    .write(&mut request);
                    let reply = backend
                        .transact(&mut Call {
                            code: api::ATTACH_ART_OWNER,
                            flags: 0,
                            sender_pid: 42,
                            sender_euid: uid,
                            data: request.reader(),
                        })
                        .unwrap();
                    assert!(
                        api::read_attach_art_owner_reply(&mut reply.reader())
                            .unwrap()
                            .is_err()
                    );
                }
                for (pid, generation, binder) in [
                    (43, 100, Some(owner.binder())),
                    (42, 99, Some(owner.binder())),
                    (42, 100, None),
                    (42, 100, Some(Binder::Local(1))),
                ] {
                    assert!(
                        host.attach_art_owner(pid, generation, epoch.clone(), binder)
                            .is_err()
                    );
                }
                {
                    let _install = system.package_install_guard();
                    host.attach_art_owner(42, 100, epoch.clone(), Some(owner.binder()))
                        .unwrap();
                    host.check_art_owner(42, 100, &epoch).unwrap();
                }
                assert!(
                    host.attach_art_owner(42, 100, epoch.clone(), Some(owner.binder()))
                        .is_err()
                );
                assert!(host.check_art_owner(42, 99, &epoch).is_err());
                let foreign = Arc::new(Captures::new(system, bridge.clone()));
                assert!(host.check_art_owner(42, 100, &foreign).is_err());
                publish(&mut |_| {}); // Ordinary publication does not withdraw ART.
                host.check_art_owner(42, 100, &epoch).unwrap();
                host.art_owner_died(100);
                assert!(host.check_art_owner(42, 100, &epoch).is_err());
                assert!(
                    host.attach_art_owner(42, 100, epoch.clone(), Some(owner.binder()))
                        .is_err()
                );
                let mut apps = host.apps.lock().unwrap();
                let mut app = apps.remove(&100).unwrap();
                app.pid = 43;
                apps.insert(101, app);
                drop(apps);
                host.attach_art_owner(43, 101, epoch.clone(), Some(owner.binder()))
                    .unwrap();
                host.art_owner_died(100);
                host.check_art_owner(43, 101, &epoch).unwrap();
                die();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while host.art_owner.lock().unwrap().owner.is_some() {
                    assert!(
                        std::time::Instant::now() < until,
                        "ART Binder death not delivered"
                    );
                    std::thread::yield_now();
                }
                assert!(host.check_art_owner(43, 101, &epoch).is_err());
                assert!(
                    host.attach_art_owner(43, 102, epoch, Some(owner.binder()))
                        .is_err()
                );
            },
        );
    }
}
