//! `permission`: the permission controller native services ask
//! (`IPermissionController`, which ActivityManager publishes at
//! `android-16.0.0_r1`): `checkPermission` as ActivityManager decides it,
//! `noteOp` as AppOps notes it, and PackageManager's answers about uids
//! and permissions. PermissionControllerService's staged-backup requests
//! bind the app's `android.permission.IPermissionController` separately.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::android_os_ipermissioncontroller as pc;
use aim_service_aidl::android_permission_ipermissioncontroller as controller;
use aim_service_aidl::com_android_internal_infra_iandroidfuture as future;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::services::HostConnection;
use super::{AppHost, reply};
use crate::package::component_resolver::Info;
use crate::package::installer::removal::UserHandle;
use crate::package::intent::Intent;
use crate::system::ServiceOwner;

const SERVICE_INTERFACE: &str = "android.permission.PermissionControllerService";
/// `PermissionControllerManager.REQUEST_TIMEOUT_MILLIS`.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// `PermissionControllerManager.applyStagedRuntimePermissionBackup`.
/// No staged backup is assumed: the module's callback owns that answer.
pub(super) fn apply_staged_runtime_permission_backup(
    host: &Arc<AppHost>,
    package: &str,
    user: i32,
    completed: impl FnOnce(bool) + Send + 'static,
) -> Result<(), Exception> {
    let (state, resolution) = host.packages()?;
    let controller_package = state
        .system
        .permission_controller_package
        .as_ref()
        .and_then(|p| p.as_ref())
        .ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "permission controller package unavailable",
            )
        })?;
    let intent = Intent {
        action: Some(SERVICE_INTERFACE.into()),
        package: Some(controller_package.clone()),
        ..Default::default()
    };
    // PermissionService creates its manager with the system context;
    // the requested backup user is an argument, not the binding's user.
    let service = resolution
        .resolve_service(&intent, None, 0, 0, 1000)
        .map_err(|e| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("permission controller resolve: {e:?}"),
            )
        })?;
    let Some(Info::Service(service)) = service.map(|s| s.info) else {
        return Err(Exception::new(
            EX_ILLEGAL_STATE,
            "permission controller service unavailable",
        ));
    };
    let class = service.info.item.name.as_deref().ok_or_else(|| {
        Exception::new(EX_ILLEGAL_STATE, "permission controller class unavailable")
    })?;
    let (sender, connected) = mpsc::channel();
    let connection: Arc<dyn HostConnection> = Arc::new(BackupConnection(sender));
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let bound = host.bind_service_from_host(
        Some(SERVICE_INTERFACE),
        controller_package,
        class,
        0,
        1,
        0,
        connection.clone(),
    );
    match bound {
        Ok(true) => {}
        result => {
            host.unbind_service_from_host(&connection);
            return result.and_then(|_| {
                Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "permission controller bind failed",
                ))
            });
        }
    }
    let host = host.clone();
    let package = package.to_owned();
    std::thread::spawn(move || {
        let result = (|| {
            let binder = connected
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| format!("permission controller connection: {e}"))?
                .ok_or("permission controller binding died")?;
            apply_backup(&host, binder, &package, user, deadline)
        })();
        host.unbind_service_from_host(&connection);
        match result {
            Ok(has_more) => completed(has_more),
            Err(error) => {
                // The manager reports true on error, so PermissionService
                // leaves this user's delayed restore open for a later call.
                eprintln!(
                    "app host: permission controller: Error restoring delayed permissions for {package}: {error}"
                );
                completed(true);
            }
        }
    });
    Ok(())
}

struct BackupConnection(mpsc::Sender<Option<Binder>>);
impl HostConnection for BackupConnection {
    fn connected(&self, service: Option<Binder>, dead: bool) {
        // Called under ActiveServices' lock; the request runs on its worker.
        let _ = self.0.send(if dead { None } else { service });
    }
}

/// `AndroidFuture.writeToParcel`: an unfinished future's remote origin.
struct PendingBackup(Binder);
impl WriteParcelable for PendingBackup {
    fn write_to(&self, p: &mut Parcel) {
        p.write_bool(false);
        p.write_binder(Some(self.0));
    }
}

/// A completed `AndroidFuture<Boolean>`: done, success, `writeValue`.
struct BackupResult(bool);
impl ReadParcelable for BackupResult {
    fn read_from(r: &mut Reader<'_>) -> Result<Self, i32> {
        if !r.read_bool()? || r.read_bool()? {
            return Err(BAD_VALUE);
        }
        Ok(Self(match r.read_i32()? {
            -1 => false,         // Boolean.TRUE.equals(null)
            9 => r.read_bool()?, // Parcel.VAL_BOOLEAN
            _ => return Err(BAD_VALUE),
        }))
    }
}

struct BackupCallback(mpsc::Sender<Result<bool, String>>);
impl Service for BackupCallback {
    fn descriptor(&self) -> &str {
        future::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != future::COMPLETE {
            return Err(BAD_VALUE);
        }
        let result = future::Complete::<BackupResult>::read(&mut call.data).and_then(|a| {
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            a.result_container.map(|r| r.0).ok_or(BAD_VALUE)
        });
        let _ = self
            .0
            .send(result.map_err(|_| "failed or invalid permission restore future".into()));
        result.map(|_| Parcel::new())
    }
}

fn apply_backup(
    host: &AppHost,
    binder: Binder,
    package: &str,
    user: i32,
    deadline: Instant,
) -> Result<bool, String> {
    let owner = match binder {
        Binder::Handle(h) => ServiceOwner::Remote(Arc::new(host.process.strong(h))),
        Binder::Local(ptr) => ServiceOwner::Local(
            host.process
                .local_service(ptr)
                .ok_or("permission controller node unavailable")?,
        ),
    };
    let (sender, result) = mpsc::channel();
    let callback = host.process.add_callback(Arc::new(BackupCallback(sender)));
    let mut request = Parcel::new();
    controller::ApplyStagedRuntimePermissionBackup {
        package_name: Some(package.into()),
        user: Some(UserHandle(user)),
        callback: Some(PendingBackup(callback)),
    }
    .write(&mut request);
    owner
        .transact(
            controller::APPLY_STAGED_RUNTIME_PERMISSION_BACKUP,
            &request,
            true,
        )
        .map_err(|e| format!("permission controller restore transport {e}"))?;
    result
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|e| format!("permission controller restore callback: {e}"))?
}

#[cfg(test)]
mod backup_tests {
    use super::*;

    impl ReadParcelable for PendingBackup {
        fn read_from(r: &mut Reader<'_>) -> Result<Self, i32> {
            assert!(!r.read_bool()?);
            Ok(Self(r.read_binder()?.ok_or(BAD_VALUE)?))
        }
    }

    /// The module's callback crosses the same AndroidFuture wire as
    /// PermissionControllerService, rather than assuming an empty backup.
    struct Module {
        host: std::sync::Weak<AppHost>,
        result: Option<bool>,
    }
    impl Service for Module {
        fn descriptor(&self) -> &str {
            controller::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(
                call.code,
                controller::APPLY_STAGED_RUNTIME_PERMISSION_BACKUP
            );
            let a =
                controller::ApplyStagedRuntimePermissionBackup::<UserHandle, PendingBackup>::read(
                    &mut call.data,
                )?;
            assert_eq!(a.package_name.as_deref(), Some("installed.package"));
            assert_eq!(a.user.unwrap().0, 10);
            assert_eq!(call.data.remaining(), 0);
            let mut p = Parcel::new();
            p.write_interface_token(future::DESCRIPTOR);
            p.write_i32(1); // typed AndroidFuture
            p.write_bool(true); // done
            p.write_bool(false); // success
            match self.result {
                Some(value) => {
                    p.write_i32(9);
                    p.write_bool(value);
                }
                None => p.write_i32(-1),
            }
            let Binder::Local(ptr) = a.callback.unwrap().0 else {
                panic!()
            };
            self.host
                .upgrade()
                .unwrap()
                .process
                .local_service(ptr)
                .unwrap()
                .transact(future::COMPLETE, &p, true)
                .map(|_| Parcel::new())
        }
    }

    #[test]
    fn staged_backup_reads_the_module_result_and_reports_failed_callbacks() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let host = AppHost::new(process);
        for result in [Some(true), Some(false), None] {
            let binder = host.process.add_service(Arc::new(Module {
                host: Arc::downgrade(&host),
                result,
            }));
            assert_eq!(
                apply_backup(
                    &host,
                    binder,
                    "installed.package",
                    10,
                    Instant::now() + Duration::from_secs(2)
                )
                .unwrap(),
                result == Some(true)
            );
        }
        let (tx, rx) = mpsc::channel();
        let callback = BackupCallback(tx);
        for (done, failed, value) in [(true, true, 9), (false, false, 9), (true, false, 1)] {
            let mut p = Parcel::new();
            p.write_interface_token(future::DESCRIPTOR);
            p.write_i32(1);
            p.write_bool(done);
            p.write_bool(failed);
            p.write_i32(value);
            assert!(
                callback
                    .transact(&mut Call {
                        code: future::COMPLETE,
                        flags: 1,
                        sender_pid: 77,
                        sender_euid: 1000,
                        data: p.reader()
                    })
                    .is_err()
            );
            assert!(rx.recv().unwrap().is_err());
        }
        assert!(
            apply_backup(
                &host,
                Binder::Handle(u32::MAX),
                "installed.package",
                10,
                Instant::now()
            )
            .is_err()
        );
        let (tx, rx) = mpsc::channel();
        let connection = BackupConnection(tx);
        connection.connected(Some(Binder::Local(123)), false);
        assert_eq!(rx.recv().unwrap(), Some(Binder::Local(123)));
        connection.connected(None, true);
        assert_eq!(rx.recv().unwrap(), None);
    }
}

/// `PermissionInfo.PROTECTION_MASK_BASE`, `PROTECTION_DANGEROUS`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_DANGEROUS: i32 = 1;

pub struct PermissionController(pub Arc<AppHost>);

impl Service for PermissionController {
    fn descriptor(&self) -> &str {
        pc::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let host = &self.0;
        let mut p = Parcel::new();
        match call.code {
            pc::CHECK_PERMISSION => {
                let args = pc::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.check_permission(args.permission.as_deref(), args.uid) {
                    Ok(result) => pc::write_check_permission_reply(&mut p, result == 0),
                    Err(e) => return reply(Err(e)),
                }
            }
            pc::GET_PACKAGES_FOR_UID => {
                let args = pc::GetPackagesForUid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (state, _) = match host.packages() {
                    Ok(packages) => packages,
                    Err(e) => return reply(Err(e)),
                };
                let app_id = args.uid.rem_euclid(100_000);
                let names: Vec<Option<String>> = state
                    .packages
                    .values()
                    .filter(|ps| ps.app_id == app_id)
                    .map(|ps| Some(ps.name.clone()))
                    .collect();
                pc::write_get_packages_for_uid_reply(&mut p, &Some(names));
            }
            pc::GET_PACKAGE_UID => {
                let args = pc::GetPackageUid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let (state, _) = match host.packages() {
                    Ok(packages) => packages,
                    Err(e) => return reply(Err(e)),
                };
                let uid = args
                    .package_name
                    .and_then(|name| state.packages.get(&name).map(|ps| ps.app_id))
                    .unwrap_or(-1);
                pc::write_get_package_uid_reply(&mut p, uid);
            }
            pc::IS_RUNTIME_PERMISSION => {
                let args = pc::IsRuntimePermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let level = (|| {
                    let owner = host.permission_manager.get().ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable")
                    })?;
                    args.permission
                        .as_deref()
                        .map(|name| owner.protection_level(name))
                        .transpose()
                        .map(Option::flatten)
                })();
                let runtime = match level {
                    Ok(level) => level
                        .is_some_and(|level| level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS),
                    Err(e) => return reply(Err(e)),
                };
                pc::write_is_runtime_permission_reply(&mut p, runtime);
            }
            code => {
                return reply(Err(host.unimplemented(
                    "permission",
                    self.descriptor(),
                    code,
                )));
            }
        }
        Ok(p)
    }
}
