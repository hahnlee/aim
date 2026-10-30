//! The native package boot, run by the app host as system_server's
//! `NativePackageBootstrap` runs it (`begin`, then `Construction.finish`):
//! the app host's own bootstrap bridge attached as this process's node,
//! the boot session begun, its scans and construction finished, then its
//! deferred app-data preparation joined. Each stage publishes its result.

use std::sync::{Arc, OnceLock};

use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagebootsession as session;

use super::AppHost;
use super::package_bootstrap::PackageBootstrap;
use crate::system::{ServiceOwner, System};

/// Each result releases only consumers of the state that stage publishes.
#[derive(Default)]
pub(super) struct Readiness {
    pub scan: OnceLock<Result<(), Exception>>,
    pub runtime: OnceLock<Result<(), Exception>>,
    pub storage: OnceLock<Result<(), Exception>>,
}

impl Readiness {
    pub fn wait_scan(&self) -> Result<(), Exception> {
        self.scan.wait().clone()?;
        // A later failed construction revokes the publication. Do not admit
        // new scan readers after its failure is known.
        if let Some(result) = self.runtime.get() {
            result.clone()?;
        }
        Ok(())
    }
}

pub(super) fn prepare<T>(
    ready: &Readiness,
    scan: impl FnOnce() -> Result<T, Exception>,
    construction: impl FnOnce(T) -> Result<(), Exception>,
    app_data: impl FnOnce() -> Result<(), Exception>,
) -> Result<(), Exception> {
    let runtime = ready.runtime.get_or_init(|| {
        let scanned = scan();
        let _ = ready
            .scan
            .set(scanned.as_ref().map(|_| ()).map_err(Clone::clone));
        construction(scanned?)
    });
    ready
        .storage
        .get_or_init(|| {
            runtime.clone()?;
            app_data()
        })
        .clone()
}

pub(super) fn wait_for(
    name: &str,
    ready: impl Fn() -> Result<bool, Exception>,
) -> Result<(), Exception> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !ready()? {
        if std::time::Instant::now() >= deadline {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("{name} did not become ready"),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// A call to the session, which is this process's node.
fn call<T>(
    boot: &aim_binder_host::local::LocalService,
    code: u32,
    write: impl FnOnce(&mut Parcel),
    read: impl FnOnce(
        &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Result<T, Exception>>,
) -> Result<T, Exception> {
    let mut p = Parcel::new();
    write(&mut p);
    let state = |s: i32| Exception::new(EX_ILLEGAL_STATE, format!("package boot session: {s}"));
    let reply = boot.transact(code, &p, false).map_err(state)?;
    read(&mut reply.reader()).map_err(state)?
}

/// `NativePackageBootstrap.begin`, and `finish` through its construction.
pub(crate) fn run(host: &Arc<AppHost>) -> Result<(), Exception> {
    prepare(
        &host.package_boot,
        || scan(host, host.package_system()?),
        |boot| {
            call(
                &boot,
                session::FINISH_CONSTRUCTION,
                |p| session::FinishConstruction {}.write(p),
                session::read_finish_construction_reply,
            )
        },
        || host.package_system()?.wait_native_package_app_data(),
    )
}

fn scan(
    host: &Arc<AppHost>,
    system: &Arc<System>,
) -> Result<aim_binder_host::local::LocalService, Exception> {
    wait_for("persistent properties", || {
        Ok(host.property("ro.persistent_properties.ready").as_deref() == Some("true"))
    })?;
    wait_for("APEX activation", || {
        Ok(host.property("apex.all.ready").as_deref() == Some("true"))
    })?;
    let image_inputs = host.image_inputs()?.clone();
    // Immutable framework resources can load while init is still starting
    // the owners of the live boot configuration.
    image_inputs
        .platform()
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
    let Binder::Local(ptr) = host
        .process
        .add_service(Arc::new(PackageBootstrap::new(host.clone())))
    else {
        unreachable!("a service of this process is local")
    };
    let local = |ptr| {
        host.process
            .local_service(ptr)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "a node of this process is gone"))
    };
    system.attach_package_bootstrap_owner(ServiceOwner::Local(local(ptr)?))?;
    // Retain the epoch owner even before the first notification, so session
    // teardown revokes native service registrations too.
    system.package_observer_owner(&system.package_bootstrap()?)?;
    let Binder::Local(ptr) = system.begin_package_manager_boot(false, Some(image_inputs))? else {
        unreachable!("the session is this process's")
    };
    let boot = local(ptr)?;
    call(
        &boot,
        session::FINISH_RAW_SCAN,
        |p| session::FinishRawScan {}.write(p),
        session::read_finish_raw_scan_reply,
    )?;
    call(
        &boot,
        session::FINISH_INITIAL_SCAN,
        |p| session::FinishInitialScan {}.write(p),
        session::read_finish_initial_scan_reply,
    )?;
    Ok(boot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Service};
    use aim_service_aidl::android_content_pm_ipackagemanager as pm;
    use std::sync::mpsc;
    use std::time::Duration;

    fn feature_query(host: &Arc<AppHost>) -> Result<bool, Exception> {
        let service = super::super::package_service::PackageService::new(host.clone());
        let mut request = Parcel::new();
        pm::HasSystemFeature {
            name: Some("android.hardware.audio.output".into()),
            version: 0,
        }
        .write(&mut request);
        let response = service
            .transact(&mut Call {
                code: pm::HAS_SYSTEM_FEATURE,
                flags: 0,
                sender_pid: 42,
                sender_euid: 1000,
                data: request.reader(),
            })
            .unwrap();
        pm::read_has_system_feature_reply(&mut response.reader()).unwrap()
    }

    fn application_info_query(host: &Arc<AppHost>) -> Result<(), Exception> {
        let service = super::super::package_service::PackageService::new(host.clone());
        let mut request = Parcel::new();
        pm::GetApplicationInfo {
            package_name: Some("android".into()),
            flags: 0,
            user_id: 0,
        }
        .write(&mut request);
        let response = service
            .transact(&mut Call {
                code: pm::GET_APPLICATION_INFO,
                flags: 0,
                sender_pid: 42,
                sender_euid: 1000,
                data: request.reader(),
            })
            .unwrap();
        response.reader().read_exception().unwrap()
    }

    #[test]
    fn scan_queries_finish_while_runtime_and_storage_are_pending() {
        crate::system::with_late_package_construction(|host, publish| {
            let (started, arrival) = mpsc::channel();
            let (scan_sent, scan_received) = mpsc::channel();
            let (runtime_sent, runtime_received) = mpsc::channel();
            let (storage_sent, storage_received) = mpsc::channel();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    started.send(()).unwrap();
                    let result = host.packages().map(|(state, _)| state.packages.len());
                    scan_sent.send((result, feature_query(host))).unwrap();
                });
                scope.spawn(|| {
                    started.send(()).unwrap();
                    runtime_sent.send(application_info_query(host)).unwrap();
                });
                scope.spawn(|| {
                    started.send(()).unwrap();
                    storage_sent
                        .send(host.package_boot.storage.wait().clone())
                        .unwrap();
                });
                for _ in 0..3 {
                    arrival.recv().unwrap();
                }
                let before_scan = scan_received.recv_timeout(Duration::from_millis(50));
                let mut during_construction = None;
                let mut during_storage = None;
                let error = Exception::new(EX_ILLEGAL_STATE, "deferred app-data failure");
                let result = prepare(
                    &host.package_boot,
                    || {
                        publish();
                        Ok(())
                    },
                    |()| {
                        // A real AppOps-style read and public package transaction
                        // must complete before this stage is released.
                        during_construction = Some((
                            scan_received.recv_timeout(Duration::from_secs(5)),
                            runtime_received.recv_timeout(Duration::from_millis(50)),
                        ));
                        Ok(())
                    },
                    || {
                        during_storage = Some((
                            runtime_received.recv_timeout(Duration::from_secs(5)),
                            storage_received.recv_timeout(Duration::from_millis(50)),
                        ));
                        assert!(host.processes_ready.get().is_none());
                        Err(error.clone())
                    },
                );
                // Assertions follow publication so regressions cannot strand readers.
                assert!(matches!(before_scan, Err(mpsc::RecvTimeoutError::Timeout)));
                let (scan, runtime) = during_construction.unwrap();
                let (packages, feature) = scan.unwrap();
                assert!(packages.unwrap() > 0);
                assert!(feature.is_ok());
                assert_eq!(runtime, Err(mpsc::RecvTimeoutError::Timeout));
                let (runtime, storage) = during_storage.unwrap();
                assert!(runtime.unwrap().is_ok());
                assert_eq!(storage, Err(mpsc::RecvTimeoutError::Timeout));
                assert_eq!(result, Err(error.clone()));
                assert_eq!(
                    storage_received
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap(),
                    Err(error)
                );
                assert_eq!(host.package_boot.scan.get(), Some(&Ok(())));
                assert_eq!(host.package_boot.runtime.get(), Some(&Ok(())));
            });
        });
    }

    #[test]
    fn published_scan_does_not_hide_a_known_construction_failure() {
        crate::system::with_late_package_construction(|host, publish| {
            let error = Exception::new(EX_ILLEGAL_STATE, "construction failure");
            assert_eq!(
                prepare(
                    &host.package_boot,
                    || {
                        publish();
                        Ok(())
                    },
                    |()| Err(error.clone()),
                    || panic!("app data after construction failure"),
                ),
                Err(error.clone())
            );
            assert_eq!(feature_query(host), Err(error.clone()));
            assert_eq!(application_info_query(host), Err(error.clone()));
            assert_eq!(host.packages().err(), Some(error));
            assert!(host.processes_ready.get().is_none());
        });
    }

    #[test]
    fn failures_release_every_dependent_stage_without_running_later_work() {
        for scan_fails in [true, false] {
            let ready = Readiness::default();
            let error = Exception::new(EX_ILLEGAL_STATE, "package boot failure");
            assert_eq!(
                prepare(
                    &ready,
                    || if scan_fails {
                        Err(error.clone())
                    } else {
                        Ok(())
                    },
                    |()| {
                        assert!(!scan_fails);
                        Err(error.clone())
                    },
                    || panic!("app data after failure"),
                ),
                Err(error.clone())
            );
            assert_eq!(
                ready.scan.get(),
                Some(&if scan_fails {
                    Err(error.clone())
                } else {
                    Ok(())
                })
            );
            assert_eq!(ready.runtime.get(), Some(&Err(error.clone())));
            assert_eq!(ready.storage.get(), Some(&Err(error.clone())));
            assert_eq!(
                prepare(
                    &ready,
                    || -> Result<(), Exception> { panic!("repeat scan") },
                    |()| panic!("repeat construction"),
                    || panic!("repeat app data"),
                ),
                Err(error)
            );
        }
    }
}
