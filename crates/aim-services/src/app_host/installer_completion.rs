//! `IInstallerCompletionBridge` for the app host: an install's completion
//! owners, which system_server's `InstallerCompletionBridge` serves, to
//! the system uid only. An instant app, an APEX or a rollback from the
//! platform is not dexopted, as the original answers it. The app host
//! has no ART Service (#2063): the original finds no `ArtManagerLocal`,
//! which with `improve_install_freeze` it reports as the observed
//! exception and the install completes undexopted, its app interpreted and
//! JIT-compiled; without the flag the exception fails the install, as there.
//! The app-op owner receives package-added on installs and updates.

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
                let a = bridge::PackageAdded::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    let name = a.package_name.ok_or_else(|| {
                        Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName")
                    })?;
                    super::appops_service::package_added(&self.host, &name, a.app_id)?;
                    bridge::write_package_added_reply(&mut p);
                    Ok(p)
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
            let process = crate::test_process(euid);
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
        let mut p = Parcel::new();
        bridge::PackageAdded {
            package_name: None,
            app_id: 10100,
        }
        .write(&mut p);
        let response = node(10000)
            .transact(bridge::PACKAGE_ADDED, &p, false)
            .unwrap();
        assert_eq!(
            response
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        let response = node(SYSTEM_UID)
            .transact(bridge::PACKAGE_ADDED, &p, false)
            .unwrap();
        assert_eq!(
            response
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_NULL_POINTER
        );
    }

    #[test]
    fn package_added_reaches_appops_without_access_files() {
        crate::system::with_discovery_session(|host, _, _| {
            std::fs::write(host.data().unwrap().join("system/users/0.xml"),
                br#"<user id="0" serialNumber="0" flags="19" type="android.os.usertype.full.SYSTEM"/>"#).unwrap();
            let service = InstallerCompletion::new(host.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                host.process.add_service(Arc::new(service))
            else {
                unreachable!()
            };
            let node = host.process.local_service(ptr).unwrap();
            for _ in 0..2 {
                let mut p = Parcel::new();
                bridge::PackageAdded {
                    package_name: Some("p".into()),
                    app_id: 10100,
                }
                .write(&mut p);
                let response = node.transact(bridge::PACKAGE_ADDED, &p, false).unwrap();
                bridge::read_package_added_reply(&mut response.reader())
                    .unwrap()
                    .unwrap();
            }
            assert!(
                !host
                    .data()
                    .unwrap()
                    .join("misc_de/0/apexdata/com.android.permission/access.abx")
                    .exists()
            );
        });
    }
}
