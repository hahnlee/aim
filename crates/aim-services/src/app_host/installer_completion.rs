//! `IInstallerCompletionBridge` for the app host: an install's completion
//! owners, which system_server's `InstallerCompletionBridge` serves, to
//! the system uid only. An instant app, an APEX or a rollback from the
//! platform is not dexopted, as the original answers it. The app host
//! has no ART Service (#2063): the original finds no `ArtManagerLocal`,
//! which with `improve_install_freeze` it reports as the observed
//! exception and the install completes undexopted, its app interpreted and
//! JIT-compiled; without the flag the exception fails the install, as there.
//! The app-op owner's package-added state is part of committing an
//! install (#1995).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION,
};
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

    /// `dexoptInstalledResult`: none for what is not dexopted; otherwise
    /// `observeException` of the missing ART owner.
    fn dexopt(
        &self,
        instant: bool,
        apex: bool,
        rollback: bool,
    ) -> Result<Option<DexoptResult>, Exception> {
        if instant || apex || rollback {
            return Ok(None);
        }
        const UNAVAILABLE: &str = "original ART/package snapshot owner unavailable (#2063)";
        let improve_install_freeze = self
            .host
            .framework()?
            .0
            .flags
            .get("android.content.pm.improve_install_freeze")
            == Some(&true);
        if !improve_install_freeze {
            return Err(Exception::new(EX_ILLEGAL_STATE, UNAVAILABLE));
        }
        eprintln!(
            "app host: installer completion: Dexopt encountered a fatal error: {UNAVAILABLE}"
        );
        Ok(Some(DexoptResult::ObservedException {
            class: "java.lang.IllegalStateException".into(),
            message: Some(UNAVAILABLE.into()),
        }))
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
                        .map(|_| {
                            // `dexoptInstalled`: an observed exception has no result.
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
                        .map(|result| {
                            bridge::write_dexopt_installed_result_reply(&mut p, result.as_ref());
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

    /// What the original does not dexopt has no result; the rest needs
    /// the image's `improve_install_freeze` flag; the system only asks.
    #[test]
    fn dexopt_without_art() {
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
        // An unconfigured host has no image flags to decide by.
        assert!(dexopt(SYSTEM_UID, false).is_err());
        assert_eq!(dexopt(10000, true).unwrap_err().code, EX_SECURITY);
    }
}
