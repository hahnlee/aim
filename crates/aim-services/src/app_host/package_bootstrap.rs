//! `IPackageBootstrapBridge` for the app host: the outside decisions and
//! the leaves the native package boot reads, answered as system_server's
//! `PackageBootstrapBridge` answers them (`android-16.0.0_r1`), from the
//! app host's own owners (`boot_policy`'s decisions, the boot leaves) for the
//! native package boot it runs (served as its own node). The bridge's
//! other owners are not implemented (#2022).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;

use super::boot_policy;
use super::{AppHost, reply};
use crate::package::bootstrap::ScanUsers;
use crate::package::pkg::AndroidPackage;
use crate::package::scan::SharedUidMigration;
use crate::package::system_config::Sdk;

const SYSTEM_UID: i32 = 1000;

pub struct PackageBootstrap {
    host: Arc<AppHost>,
    /// ApexManager's `mApexSystemServices`' names, kept across its
    /// notifications.
    apex_system_services: Mutex<Vec<String>>,
}

/// `PackageCacher.fromCacheEntryStatic` of a call's parsed package.
fn cached_package(cache: Option<Vec<u8>>) -> Result<AndroidPackage, Exception> {
    let cache = cache.ok_or_else(|| Exception::illegal_argument("missing parsed package"))?;
    AndroidPackage::read_cache_entry(&cache)
        .map_err(|e| Exception::illegal_argument(format!("parsed package: {e:?}")))
}

impl PackageBootstrap {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageBootstrap {
            host,
            apex_system_services: Mutex::new(Vec::new()),
        }
    }

    /// A leaf of this process, a new one for each call as the original
    /// makes them.
    fn leaf(&self, service: Arc<dyn Service>) -> aim_binder_host::parcel::Binder {
        self.host.process.add_service(service)
    }
}

impl Service for PackageBootstrap {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforceSystemUid`.
        if call.sender_euid as i32 != SYSTEM_UID {
            return reply(Err(Exception::security(
                "the package bridge serves the system uid only",
            )));
        }
        let mut p = Parcel::new();
        let host = &self.host;
        let result: Result<(), Exception> = match call.code {
            bridge::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                bridge::IsTestBaseOnBootclasspath::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.root()
                    .and_then(|root| {
                        boot_policy::test_base_on_bootclasspath(root)
                            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    })
                    .map(|v| bridge::write_is_test_base_on_bootclasspath_reply(&mut p, v))
            }
            // `Build.isDebuggable()`.
            bridge::IS_SIGNING_DEBUGGABLE => {
                bridge::IsSigningDebuggable::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let debuggable = host.property("ro.debuggable").as_deref() == Some("1");
                bridge::write_is_signing_debuggable_reply(&mut p, debuggable);
                Ok(())
            }
            bridge::IS_SHARED_UID_MIGRATION_BEST_EFFORT => {
                bridge::IsSharedUidMigrationBestEffort::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let migration = boot_policy::shared_uid_migration(
                    host.property("ro.build.type").as_deref(),
                    host.property(boot_policy::SHARED_UID_MIGRATION_STRATEGY)
                        .as_deref(),
                );
                bridge::write_is_shared_uid_migration_best_effort_reply(
                    &mut p,
                    migration == SharedUidMigration::BestEffort,
                );
                Ok(())
            }
            bridge::GENERATE_NEW_DOMAIN_ID => {
                bridge::GenerateNewDomainId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `DomainVerificationService.generateNewId`: `UUID.randomUUID()`.
                let id = boot_policy::random_uuid();
                bridge::write_generate_new_domain_id_reply(&mut p, &Some(id.to_vec()));
                Ok(())
            }
            bridge::GET_PACKAGE_SCAN_USERS => {
                bridge::GetPackageScanUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::scan_users(host).map(|users| {
                    let record = ScanUsers { users: Some(users) }.write_original_record();
                    bridge::write_get_package_scan_users_reply(&mut p, &Some(record))
                })
            }
            bridge::GET_APEX_BOOT_INVENTORY => {
                bridge::GetApexBootInventory::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::apex_manager::inventory(&host.process)
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    .map(|inventory| {
                        bridge::write_get_apex_boot_inventory_reply(
                            &mut p,
                            &Some(inventory.write_original_record()),
                        )
                    })
            }
            bridge::NOTIFY_APEX_SCAN_RESULTS => {
                let a =
                    bridge::NotifyApexScanResults::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.scan_results {
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                    Some(bytes) if bytes.len() < 4 => Err(Exception::new(
                        EX_ILLEGAL_ARGUMENT,
                        "missing APEX result count",
                    )),
                    Some(bytes) if bytes.len() & 3 != 0 => Err(Exception::new(
                        EX_ILLEGAL_ARGUMENT,
                        "unaligned APEX scan results",
                    )),
                    Some(bytes) => boot_policy::notify_apex_payload(
                        &Sdk::new(&|name| host.property(name)),
                        &mut self.apex_system_services.lock().unwrap(),
                        &bytes,
                    )
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    .map(|()| bridge::write_notify_apex_scan_results_reply(&mut p)),
                }
            }
            bridge::IS_TEST_BASE_LIBRARY_CHANGE_ENABLED => {
                let a = bridge::IsTestBaseLibraryChangeEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                cached_package(a.package_cache)
                    .and_then(|pkg| {
                        Ok(boot_policy::test_base_change(
                            &host.platform()?.compat,
                            &pkg,
                        ))
                    })
                    .map(|v| bridge::write_is_test_base_library_change_enabled_reply(&mut p, v))
            }
            bridge::GET_SE_INFO_TARGET_SDK_VERSION => {
                let a = bridge::GetSeInfoTargetSdkVersion::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                cached_package(a.package_cache)
                    .and_then(|pkg| {
                        Ok(boot_policy::seinfo_target_sdk(
                            &host.platform()?.compat,
                            &pkg,
                        ))
                    })
                    .map(|v| bridge::write_get_se_info_target_sdk_version_reply(&mut p, v))
            }
            bridge::GET_PACKAGE_INITIAL_CONTEXT_LEAF => {
                bridge::GetPackageInitialContextLeaf::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(
                    super::package_initial_context::InitialContext::new(host.clone()),
                ));
                bridge::write_get_package_initial_context_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF => {
                let a = bridge::GetPackageBootConfigurationLeaf::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(
                    super::package_boot_configuration::BootConfiguration::new(
                        host.clone(),
                        a.factory_test,
                    ),
                ));
                bridge::write_get_package_boot_configuration_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_BOOT_CONTEXT_LEAF => {
                bridge::GetPackageBootContextLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(super::package_boot_context::BootContext::new(host.clone()));
                bridge::write_get_package_boot_context_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_LIFECYCLE_LEAF => {
                bridge::GetPackageLifecycleLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(super::package_boot_context::Lifecycle::new(
                    host.clone(),
                )));
                bridge::write_get_package_lifecycle_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_BOOT_LIFECYCLE_LEAF => {
                bridge::GetPackageBootLifecycleLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(super::package_boot_lifecycle::BootLifecycle::new(
                    host.clone(),
                )));
                bridge::write_get_package_boot_lifecycle_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(host.unimplemented_named("package bootstrap", &format!("{name} (#2022)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::bootstrap::Bridge;
    use crate::system::ServiceOwner;

    /// The native boot's own bridge (`Bridge`) attached to this one as the
    /// process's node: its handshake and decisions, the users, a parsed
    /// package's decisions, the leaves, and a call not yet served.
    #[test]
    fn the_native_bridge_reads_the_app_hosts_bridge() {
        let Some(root) = aim_paths::original_image_with("system/etc/classpaths/bootclasspath.pb")
        else {
            return;
        };
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID as u32,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let data = std::env::temp_dir().join(format!("aim-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(data.join("system")).unwrap();
        host.configure(
            &root,
            &data,
            &data,
            Box::new(|name| {
                Some(
                    match name {
                        "ro.build.version.sdk" => "36",
                        "ro.build.version.codename" => "REL",
                        "persist.sys.locale" => "en-US",
                        "ro.sf.lcd_density" => "320",
                        "ro.build.type" => "user",
                        "ro.debuggable" => "0",
                        _ => return None,
                    }
                    .into(),
                )
            }),
            Box::new(|_, _| {}),
            Box::new(|| Err(std::io::Error::other("no zygote"))),
        )
        .unwrap();
        super::super::user_service::ensure_system_user(&host).unwrap();
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(PackageBootstrap::new(host.clone())))
        else {
            unreachable!()
        };
        let owner = ServiceOwner::Local(process.local_service(ptr).unwrap());
        let bridge = Bridge::new(owner).unwrap();
        assert!(!bridge.signing_debuggable);
        assert_eq!(
            bridge.shared_uid_migration().unwrap(),
            SharedUidMigration::NewInstallOnly
        );
        let users = bridge.scan_users().unwrap().users.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].id, 0);
        let (a, b) = (
            bridge.new_domain_id().unwrap(),
            bridge.new_domain_id().unwrap(),
        );
        assert_ne!(a, b);
        assert_eq!(a[6] >> 4, 4);
        let package = AndroidPackage {
            package_name: "a.b".into(),
            target_sdk_version: 29,
            feature_flag_state: Some(Vec::new()),
            ..Default::default()
        };
        assert!(bridge.seinfo_target_sdk(&package).unwrap() >= 29);
        bridge.test_base_change(&package).unwrap();
        let call = |code: u32| {
            let mut p = Parcel::new();
            p.write_interface_token(bridge::DESCRIPTOR);
            if code == bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF {
                p.write_bool(false);
            }
            match &bridge.owner {
                ServiceOwner::Local(node) => node.transact(code, &p, false).unwrap(),
                ServiceOwner::Remote(_) => unreachable!(),
            }
        };
        for code in [
            bridge::GET_PACKAGE_INITIAL_CONTEXT_LEAF,
            bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF,
            bridge::GET_PACKAGE_BOOT_CONTEXT_LEAF,
            bridge::GET_PACKAGE_LIFECYCLE_LEAF,
            bridge::GET_PACKAGE_BOOT_LIFECYCLE_LEAF,
        ] {
            let reply = call(code);
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert!(matches!(
                r.read_binder().unwrap(),
                Some(aim_binder_host::parcel::Binder::Local(_))
            ));
        }
        let reply = call(bridge::GET_CURRENT_PACKAGE_VERSION);
        let e = reply.reader().read_exception().unwrap().unwrap_err();
        assert!(e.message.contains("#2022"), "{}", e.message);
        std::fs::remove_dir_all(&data).unwrap();
    }
}
