//! The native package boot, run by the app host as system_server's
//! `NativePackageBootstrap` runs it (`begin`, then `Construction.finish`):
//! the app host's own bootstrap bridge attached as this process's node,
//! the boot session begun, its scans finished and its construction
//! finished.

use std::sync::Arc;

use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagebootsession as session;

use super::AppHost;
use super::package_bootstrap::PackageBootstrap;
use crate::system::{ServiceOwner, System};

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
