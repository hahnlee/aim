//! `IInstallerCompletionBridge` for the app host: an install's completion
//! owners, which system_server's `InstallerCompletionBridge` serves, to
//! the system uid only. An instant app, an APEX or a rollback from the
//! platform is not dexopted, as the original answers it. The app host
//! has no ART Service (#2063), so no install is dexopted: the app runs
//! interpreted and JIT-compiled, as an app whose dexopt was skipped does
//! (the maintainer's install policy until #2063); the app-op owner's
//! package-added state is part of committing an install (#1995).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallercompletionbridge as bridge;

use super::{AppHost, reply};
use crate::package::installer::install_warnings::DexoptResult;

const SYSTEM_UID: u32 = 1000;

pub struct InstallerCompletion {
    host: Arc<AppHost>,
}

impl InstallerCompletion {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerCompletion { host }
    }

    /// `dexoptInstalledResult`'s answer: none for what is not dexopted,
    /// and without ART Service (#2063) nothing else is.
    fn dexopt(&self, instant: bool, apex: bool, rollback: bool) -> Result<(), Exception> {
        if !(instant || apex || rollback) {
            eprintln!("app host: installer completion: not dexopted without ART Service (#2063)");
        }
        Ok(())
    }
}

impl Service for InstallerCompletion {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforceNativeOwner`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security(
                    "native installer bridge requires system UID",
                ))
            }
        };
        let mut p = Parcel::new();
        match call.code {
            bridge::DEXOPT_INSTALLED => {
                let a = bridge::DexoptInstalled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| self.dexopt(a.instant_app, a.apex, a.rollback_from_platform))
                        .map(|()| {
                            bridge::write_dexopt_installed_reply(&mut p, -1);
                            p
                        }),
                )
            }
            bridge::DEXOPT_INSTALLED_RESULT => {
                let a =
                    bridge::DexoptInstalledResult::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| self.dexopt(a.instant_app, a.apex, a.rollback_from_platform))
                        .map(|()| {
                            bridge::write_dexopt_installed_result_reply::<DexoptResult>(
                                &mut p, None,
                            );
                            p
                        }),
                )
            }
            bridge::PACKAGE_ADDED => {
                bridge::PackageAdded::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    Err(self
                        .host
                        .unimplemented_named("installer completion", "packageAdded (#1995)"))
                }))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_SECURITY;

    /// No install is dexopted without ART Service; the system only asks.
    #[test]
    fn installs_are_not_dexopted_without_art() {
        let node = |euid: u32| {
            let process = aim_binder_host::local::LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid,
                    security_context: None,
                },
            );
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(InstallerCompletion::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let dexopt = |euid: u32, apex: bool| {
            let mut p = Parcel::new();
            bridge::DexoptInstalled {
                package_name: Some("a.b".into()),
                install_scenario: 0,
                install_reason: 0,
                install_flags: 0,
                compiler_filter: None,
                debuggable: false,
                instant_app: false,
                apex,
                rollback_from_platform: false,
            }
            .write(&mut p);
            let reply = node(euid)
                .transact(bridge::DEXOPT_INSTALLED, &p, false)
                .unwrap();
            bridge::read_dexopt_installed_reply(&mut reply.reader()).unwrap()
        };
        assert_eq!(dexopt(SYSTEM_UID, true).unwrap(), -1);
        assert_eq!(dexopt(SYSTEM_UID, false).unwrap(), -1);
        assert_eq!(dexopt(10000, true).unwrap_err().code, EX_SECURITY);
    }
}
