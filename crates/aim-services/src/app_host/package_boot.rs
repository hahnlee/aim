//! The native package boot, run by the app host as system_server's
//! `NativePackageBootstrap` runs it (`begin`, then `Construction.finish`):
//! the app host's own bootstrap bridge attached as this process's node,
//! the boot session begun, its scans finished and its construction
//! finished.

use std::sync::{Arc, OnceLock};

use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagebootsession as session;

use super::AppHost;
use super::package_bootstrap::PackageBootstrap;
use crate::system::{ServiceOwner, System};

/// PMS publishes its constructed query owners before waiting for deferred
/// app-data preparation. Native metadata consumers (including audioserver's
/// UID observer) need those owners, not every application's data directory.
pub(super) fn prepare(
    constructed: &OnceLock<Result<(), Exception>>,
    construction: impl FnOnce() -> Result<(), Exception>,
    app_data: impl FnOnce() -> Result<(), Exception>,
) -> Result<(), Exception> {
    constructed.get_or_init(construction).clone()?;
    app_data()
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
pub(super) fn run(host: &Arc<AppHost>, system: &Arc<System>) -> Result<(), Exception> {
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
    let Binder::Local(ptr) = system.begin_package_manager_boot(false)? else {
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
    call(
        &boot,
        session::FINISH_CONSTRUCTION,
        |p| session::FinishConstruction {}.write(p),
        session::read_finish_construction_reply,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn metadata_queries_finish_while_deferred_app_data_is_pending() {
        crate::system::with_late_package_construction(|host, construct| {
            let (sent, received) = mpsc::channel();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    // AppOps' first flag read in registerUidObserver uses this
                    // retained query owner, even for audioserver's non-app UID.
                    let result = host.packages().map(|(state, _)| state.packages.len());
                    sent.send(result).unwrap();
                });
                assert_eq!(
                    received.recv_timeout(Duration::from_millis(50)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                );
                let error = Exception::new(EX_ILLEGAL_STATE, "deferred app-data failure");
                let mut metadata = None;
                let result = prepare(
                    &host.package_boot,
                    || {
                        construct();
                        Ok(())
                    },
                    || {
                        // Keep the data-preparation barrier held until the real
                        // metadata consumer has completed. Do not lose its error.
                        metadata = Some(received.recv_timeout(Duration::from_secs(5)));
                        assert!(host.processes_ready.get().is_none());
                        Err(error.clone())
                    },
                );
                // Release the reader even if a regression put the app-data
                // wait back inside the construction gate.
                let _ = host.package_boot.set(Err(error.clone()));
                assert!(metadata.unwrap().unwrap().unwrap() > 0);
                assert_eq!(result, Err(error));
                assert_eq!(host.package_boot.get(), Some(&Ok(())));
            });
        });
    }

    #[test]
    fn construction_failure_releases_queries_without_preparing_data() {
        let constructed = OnceLock::new();
        let error = Exception::new(EX_ILLEGAL_STATE, "construction failure");
        assert_eq!(
            prepare(&constructed, || Err(error.clone()), || panic!("app data")),
            Err(error.clone())
        );
        assert_eq!(constructed.get(), Some(&Err(error)));
    }
}
