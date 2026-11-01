//! Private retained sync owner construction. This does not enable forwarding.
use super::{AppHost, SYSTEM_PROCESS, SYSTEM_UID};
use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    generation: i64,
    owner: Option<Attachment>,
    ready: bool,
}
struct Attachment {
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
    Exception::new(
        EX_ILLEGAL_STATE,
        "private sync owner generation unavailable (#1460)",
    )
}
impl AppHost {
    pub(super) fn sync_forwarding_owner(&self) -> Result<Strong, Exception> {
        let state = self.sync_owner.lock().unwrap();
        if !state.ready {
            return Err(unavailable());
        }
        let owner = state.owner.as_ref().ok_or_else(unavailable)?;
        Ok(self.process.strong(owner.strong.handle))
    }
    pub(super) fn sync_owner_generation(&self, pid: i32) -> Result<i64, Exception> {
        let apps = self.apps.lock().unwrap();
        apps.iter()
            .find(|(_, a)| {
                !a.removed
                    && a.process == SYSTEM_PROCESS
                    && a.uid == SYSTEM_UID
                    && a.pid == pid
                    && a.thread.is_some()
            })
            .map(|(seq, _)| *seq)
            .ok_or_else(unavailable)
    }
    pub(super) fn attach_sync_owner(
        &self,
        pid: i32,
        generation: i64,
        owner: Option<Binder>,
    ) -> Result<(), Exception> {
        // Keep process identity stable through publication; no Java call under either lock.
        let apps = self.apps.lock().unwrap();
        let valid = apps.get(&generation).is_some_and(|a| {
            !a.removed
                && a.process == SYSTEM_PROCESS
                && a.uid == SYSTEM_UID
                && a.pid == pid
                && a.thread.is_some()
        });
        if !valid {
            return Err(unavailable());
        }
        let Some(Binder::Handle(handle)) = owner else {
            return Err(Exception::illegal_argument(
                "private sync owner must be remote",
            ));
        };
        let mut state = self.sync_owner.lock().unwrap();
        if generation <= state.generation {
            return Err(unavailable());
        }
        let strong = self.process.strong(handle);
        let host = self.me.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(host) = host.upgrade() {
                    host.sync_owner_died(generation);
                }
            }),
        );
        *state = State {
            generation,
            ready: false,
            owner: Some(Attachment {
                process: self.process.clone(),
                strong,
                death,
            }),
        };
        Ok(())
    }
    pub(super) fn sync_owner_attached(&self, pid: i32, generation: i64) -> Result<bool, Exception> {
        if self.sync_owner_generation(pid)? != generation {
            return Err(unavailable());
        }
        let state = self.sync_owner.lock().unwrap();
        Ok(state.generation == generation && state.owner.is_some())
    }
    pub(super) fn sync_owner_user_unlocked(
        &self,
        pid: i32,
        generation: i64,
    ) -> Result<bool, Exception> {
        if !self.sync_owner_attached(pid, generation)? {
            return Err(unavailable());
        }
        self.user_unlocking_or_unlocked(0)
    }
    pub(super) fn complete_sync_owner_replay(
        &self,
        pid: i32,
        generation: i64,
    ) -> Result<(), Exception> {
        if !self.sync_owner_user_unlocked(pid, generation)? {
            return Err(unavailable());
        }
        let apps = self.apps.lock().unwrap();
        if !apps
            .get(&generation)
            .is_some_and(|a| !a.removed && a.pid == pid && a.thread.is_some())
        {
            return Err(unavailable());
        }
        let mut state = self.sync_owner.lock().unwrap();
        if state.generation != generation || state.owner.is_none() || state.ready {
            return Err(unavailable());
        }
        state.ready = true;
        drop(state);
        drop(apps);
        if let Some(jobs) = self.job_scheduler.get() {
            jobs.sync_owner_changed();
        }
        Ok(())
    }
    pub(super) fn sync_owner_ready(&self) -> bool {
        let state = self.sync_owner.lock().unwrap();
        state.owner.is_some() && state.ready
    }
    pub(super) fn sync_owner_died(&self, generation: i64) {
        let mut state = self.sync_owner.lock().unwrap();
        if state.generation == generation {
            state.owner.take();
            state.ready = false;
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Service};
    use aim_binder_host::parcel::{EX_SECURITY, Parcel};
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    fn request(host: &Arc<AppHost>, code: u32, data: &Parcel, uid: u32, pid: i32) -> Parcel {
        super::super::system_process::SystemProcess::new(host.clone())
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: pid,
                sender_euid: uid,
                data: data.reader(),
            })
            .unwrap()
    }
    pub(in crate::app_host) fn install(host: &AppHost, thread: Strong) {
        host.apps.lock().unwrap().insert(
            100,
            super::super::App {
                package: "android".into(),
                packages: vec!["android".into()],
                process: "system".into(),
                uid: 1000,
                package_uid: 1000,
                isolated: false,
                entry_point: None,
                crash_handler: None,
                application_info: None,
                pid: 42,
                started: std::time::Instant::now(),
                start_elapsed: 0,
                start_uptime: 0,
                thread: Some(thread),
                activities: vec![],
                config: None,
                providers: vec![],
                removed: false,
                first_frame: None,
                last_activity: 0,
                persistent: true,
                package_deps: vec![],
                last_mem_info: None,
                proc_state: 2,
                oom_adj: 0,
                instrumentation: None,
            },
        );
    }
    #[test]
    fn construction_attachment_guards_generation_and_restart() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let mut get = Parcel::new();
            sp::GetSyncOwnerGeneration {}.write(&mut get);
            assert!(
                sp::read_get_sync_owner_generation_reply(
                    &mut request(host, sp::GET_SYNC_OWNER_GENERATION, &get, 1000, 42).reader()
                )
                .unwrap()
                .is_err()
            );
            install(host, export(super::super::test_activity_service(host)));
            let owner = export(super::super::test_activity_service(host));
            for (uid, pid, generation, binder) in [
                (10100, 42, 100, Some(owner.binder())),
                (1000, 43, 100, Some(owner.binder())),
                (1000, 42, 99, Some(owner.binder())),
                (1000, 42, 100, None),
                (1000, 42, 100, Some(Binder::Local(1))),
            ] {
                let mut p = Parcel::new();
                sp::AttachSyncOwner {
                    generation,
                    owner: binder,
                }
                .write(&mut p);
                let e = sp::read_attach_sync_owner_reply(
                    &mut request(host, sp::ATTACH_SYNC_OWNER, &p, uid, pid).reader(),
                )
                .unwrap()
                .unwrap_err();
                assert_eq!(e.code == EX_SECURITY, uid != 1000);
                assert!(!host.sync_owner_attached(42, 100).unwrap());
            }
            assert_eq!(
                sp::read_get_sync_owner_generation_reply(
                    &mut request(host, sp::GET_SYNC_OWNER_GENERATION, &get, 1000, 42).reader()
                )
                .unwrap()
                .unwrap(),
                100
            );
            let mut p = Parcel::new();
            sp::AttachSyncOwner {
                generation: 100,
                owner: Some(owner.binder()),
            }
            .write(&mut p);
            sp::read_attach_sync_owner_reply(
                &mut request(host, sp::ATTACH_SYNC_OWNER, &p, 1000, 42).reader(),
            )
            .unwrap()
            .unwrap();
            assert!(host.sync_owner_attached(42, 100).unwrap());
            assert!(
                host.attach_sync_owner(42, 100, Some(owner.binder()))
                    .is_err()
            );
            host.sync_owner_died(100);
            assert!(!host.sync_owner_attached(42, 100).unwrap());
            assert!(
                host.attach_sync_owner(42, 100, Some(owner.binder()))
                    .is_err()
            );
            let mut apps = host.apps.lock().unwrap();
            let mut app = apps.remove(&100).unwrap();
            app.pid = 43;
            apps.insert(101, app);
            drop(apps);
            assert!(host.sync_owner_attached(42, 100).is_err());
            host.attach_sync_owner(43, 101, Some(owner.binder()))
                .unwrap();
            host.sync_owner_died(100); // A queued old death cannot withdraw the new owner.
            let mut p = Parcel::new();
            sp::IsSyncOwnerAttached { generation: 101 }.write(&mut p);
            assert!(
                sp::read_is_sync_owner_attached_reply(
                    &mut request(host, sp::IS_SYNC_OWNER_ATTACHED, &p, 1000, 43).reader()
                )
                .unwrap()
                .unwrap()
            );
        });
    }
    #[test]
    fn replay_completion_requires_live_unlocked_generation() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            install(host, export(super::super::test_activity_service(host)));
            let owner = export(super::super::test_activity_service(host));
            host.attach_sync_owner(42, 100, Some(owner.binder()))
                .unwrap();
            host.set_user_unlocked_for_package_test(0, false);
            assert!(!host.sync_owner_user_unlocked(42, 100).unwrap());
            assert!(host.sync_forwarding_owner().is_err());
            assert!(host.complete_sync_owner_replay(42, 100).is_err());
            host.set_user_unlocked_for_package_test(0, true);
            for (uid, pid, generation) in [(10100, 42, 100), (1000, 43, 100), (1000, 42, 99)] {
                let mut p = Parcel::new();
                sp::CompleteSyncOwnerReplay { generation }.write(&mut p);
                assert!(
                    sp::read_complete_sync_owner_replay_reply(
                        &mut request(host, sp::COMPLETE_SYNC_OWNER_REPLAY, &p, uid, pid).reader()
                    )
                    .unwrap()
                    .is_err()
                );
                assert!(!host.sync_owner_ready());
            }
            let mut p = Parcel::new();
            sp::CompleteSyncOwnerReplay { generation: 100 }.write(&mut p);
            sp::read_complete_sync_owner_replay_reply(
                &mut request(host, sp::COMPLETE_SYNC_OWNER_REPLAY, &p, 1000, 42).reader(),
            )
            .unwrap()
            .unwrap();
            assert!(host.sync_owner_ready());
            assert!(host.sync_forwarding_owner().is_ok());
            assert!(host.complete_sync_owner_replay(42, 100).is_err());
            host.sync_owner_died(100);
            assert!(!host.sync_owner_ready());
            assert!(host.sync_forwarding_owner().is_err());
            assert!(host.complete_sync_owner_replay(42, 100).is_err());
        });
    }
    #[test]
    fn binder_death_withdraws_construction_readiness() {
        crate::system::with_discovery_binders_lifecycle(&[], true, |host, _, _, export, die| {
            install(host, export(super::super::test_activity_service(host)));
            let owner = export(super::super::test_activity_service(host));
            host.attach_sync_owner(42, 100, Some(owner.binder()))
                .unwrap();
            die();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while host.sync_owner_attached(42, 100).unwrap() {
                assert!(std::time::Instant::now() < until, "death not delivered");
                std::thread::yield_now();
            }
            assert!(
                host.attach_sync_owner(42, 100, Some(owner.binder()))
                    .is_err()
            );
        });
    }
}
