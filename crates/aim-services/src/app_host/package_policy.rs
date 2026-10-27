//! PackagePolicyBridge's live app-host owners (android-16.0.0_r1).
//! Protection stays with ProtectedPackages/device policy; install decisions
//! use the same Settings, UserManager and AppOps leaves as boot queries.

use super::package_initial_context::InitialContext;
use super::{AppHost, reply};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagepolicybridge as policy;
use std::sync::Arc;

pub struct PackagePolicy(pub Arc<AppHost>);

impl PackagePolicy {
    /// `PackagePolicyBridge.setInstallLocation`.
    fn set_install_location(&self, pid: i32, uid: i32, location: i32) -> Result<bool, Exception> {
        const PERMISSION: &str = "android.permission.WRITE_SECURE_SETTINGS";
        if self.0.check_permission(Some(PERMISSION), uid)? != 0 {
            return Err(Exception::security(format!(
                "setInstallLocation: pid={pid}, uid={uid} requires {PERMISSION}"
            )));
        }
        if InitialContext::new(self.0.clone()).install_location()? == location {
            return Ok(true);
        }
        if !(0..=2).contains(&location) {
            return Ok(false);
        }
        self.0.settings_provider()?.call(
            &super::settings_provider::Caller {
                uid: 1000,
                package: Some("android".into()),
            },
            "PUT_global",
            Some("default_install_location"),
            &std::collections::HashMap::from([
                ("_user".into(), crate::bundle::Value::Int(0)),
                (
                    "value".into(),
                    crate::bundle::Value::String(Some(location.to_string())),
                ),
            ]),
            None,
        )?;
        Ok(true)
    }

    /// `PackagePolicyBridge.isPackageDeviceAdmin`.
    fn device_admin(&self, package: Option<&str>, exists: bool) -> Result<bool, Exception> {
        let package = package.ok_or_else(|| {
            Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName")
        })?;
        let policies = self.0.device_policies()?;
        if policies.package_is_admin(package) {
            return Ok(true);
        }
        if exists {
            let roles = super::role_service::RoleManager::new(self.0.clone());
            for &user in policies.managed_users() {
                if roles
                    .role_holders(
                        1000,
                        Some("android.app.role.DEVICE_POLICY_MANAGEMENT".into()),
                        user,
                    )?
                    .first()
                    .is_some_and(|p| p == package)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

impl Service for PackagePolicy {
    fn descriptor(&self) -> &str {
        policy::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !matches!(call.sender_euid, 0 | 1000) {
            return reply(Err(Exception::security(
                "native package policy caller must be system or root",
            )));
        }
        let host = &self.0;
        let mut p = Parcel::new();
        let result = match call.code {
            policy::REPORT_ADMIN_PERMISSION_DENIED => {
                policy::ReportAdminPermissionDenied::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Err(host.unimplemented_named(
                    "package policy",
                    "reportAdminPermissionDenied EventLog writer (#2188)",
                ))
            }
            policy::QUERY_PACKAGE_STATE_PROTECTED | policy::QUERY_PACKAGE_DATA_PROTECTED => {
                let name = if call.code == policy::QUERY_PACKAGE_STATE_PROTECTED {
                    let a = policy::QueryPackageStateProtected::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    a.package_name
                } else {
                    let a = policy::QueryPackageDataProtected::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    a.package_name
                };
                // `ProtectedPackages`: state and data protection are identical.
                name.as_deref()
                    .map_or(Ok(false), |name| host.data_protected(name))
                    .map(|value| {
                        if call.code == policy::QUERY_PACKAGE_STATE_PROTECTED {
                            policy::write_query_package_state_protected_reply(&mut p, value);
                        } else {
                            policy::write_query_package_data_protected_reply(&mut p, value);
                        }
                    })
            }
            policy::IS_INSTALL_DISABLED => {
                let a = policy::IsInstallDisabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                InitialContext::new(host.clone())
                    .install_disabled(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.package_uid,
                        a.user_id,
                    )
                    .map(|value| policy::write_is_install_disabled_reply(&mut p, value))
            }
            policy::GET_INSTALL_LOCATION => {
                policy::GetInstallLocation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                InitialContext::new(host.clone())
                    .install_location()
                    .map(|value| policy::write_get_install_location_reply(&mut p, value))
            }
            policy::SET_INSTALL_LOCATION => {
                let a = policy::SetInstallLocation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_install_location(a.caller_pid, a.caller_uid, a.location)
                    .map(|value| policy::write_set_install_location_reply(&mut p, value))
            }
            policy::IS_PACKAGE_DEVICE_ADMIN => {
                let a =
                    policy::IsPackageDeviceAdmin::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.device_admin(a.package_name.as_deref(), a.package_exists)
                    .map(|value| policy::write_is_package_device_admin_reply(&mut p, value))
            }
            policy::IS_SHELL_DEBUGGING_RESTRICTED => {
                let a = policy::IsShellDebuggingRestricted::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.has_user_restriction("no_debugging_features", a.user_id)
                    .map(|value| policy::write_is_shell_debugging_restricted_reply(&mut p, value))
            }
            policy::IS_STORAGE_LOW => {
                policy::IsStorageLow::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `DeviceStorageMonitorService.isMemoryLow`.
                host.data_space().map(|(total, usable)| {
                    policy::write_is_storage_low_reply(
                        &mut p,
                        usable < host.storage_low_bytes(total),
                    );
                })
            }
            code => Err(host.unimplemented("package policy", policy::DESCRIPTOR, code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};
    use std::fs;

    #[test]
    fn policy_leaf_uses_the_platform_owners() {
        crate::system::with_discovery_session(|host, _, _| {
            let data = host.data().unwrap();
            let permission = data.join("misc_de/0/apexdata/com.android.permission");
            fs::create_dir_all(&permission).unwrap();
            fs::write(permission.join("access.abx"), b"<access/>").unwrap();
            fs::write(permission.join("roles.xml"), br#"<roles version="1">
                <role name="android.app.role.DEVICE_POLICY_MANAGEMENT"><holder name="manager"/></role>
                </roles>"#).unwrap();
            fs::write(data.join("system/device_owner_2.xml"),
                br#"<root><device-owner component="owner/.Admin"/><device-owner-context userId="10"/></root>"#).unwrap();
            fs::create_dir_all(data.join("system/users/10")).unwrap();
            fs::write(data.join("system/users/10/device_policies.xml"),
                br#"<policies><admin name="admin/.Receiver"/><delegation delegatePackage="delegate"/></policies>"#).unwrap();
            host.permission_manager
                .set(Arc::new(
                    super::super::permission_manager::PermissionManager::with_system(
                        host.clone(),
                        Default::default(),
                    ),
                ))
                .ok()
                .unwrap();
            host.package_writes
                .set(super::super::package_writes::Writes::new(host.clone()))
                .ok()
                .unwrap();
            host.user_restrictions
                .set(
                    [
                        "no_debugging_features".into(),
                        "no_install_unknown_sources".into(),
                    ]
                    .into(),
                )
                .unwrap();
            let leaf = PackagePolicy(host.clone());
            let call = |uid, code, args: &dyn Fn(&mut Parcel)| -> Result<i32, Exception> {
                let mut request = Parcel::new();
                request.write_interface_token(policy::DESCRIPTOR);
                args(&mut request);
                let response = leaf
                    .transact(&mut Call {
                        code,
                        flags: 0,
                        sender_pid: 77,
                        sender_euid: uid,
                        data: request.reader(),
                    })
                    .unwrap();
                let mut r = response.reader();
                r.read_exception().unwrap()?;
                let value = r.read_i32().unwrap();
                assert_eq!(r.remaining(), 0);
                Ok(value)
            };
            for &(code, _) in policy::METHODS {
                assert_eq!(call(10000, code, &|_| {}).unwrap_err().code, EX_SECURITY);
            }
            for uid in [0, 1000] {
                assert_eq!(call(uid, policy::GET_INSTALL_LOCATION, &|_| {}).unwrap(), 0);
            }
            for (uid, location, expected) in [(1000, 0, 1), (0, 3, 0)] {
                assert_eq!(
                    call(1000, policy::SET_INSTALL_LOCATION, &|p| {
                        p.write_i32(77);
                        p.write_i32(uid);
                        p.write_i32(location);
                    })
                    .unwrap(),
                    expected
                );
            }
            let error = call(1000, policy::SET_INSTALL_LOCATION, &|p| {
                p.write_i32(77);
                p.write_i32(10100);
                p.write_i32(0);
            })
            .unwrap_err();
            assert_eq!(error.code, EX_SECURITY);
            assert!(error.message.contains("WRITE_SECURE_SETTINGS"));
            for (name, exists, expected) in [
                ("owner", false, 1),
                ("admin", false, 1),
                ("delegate", true, 0),
                ("manager", false, 0),
                ("manager", true, 1),
                ("p", true, 0),
            ] {
                assert_eq!(
                    call(1000, policy::IS_PACKAGE_DEVICE_ADMIN, &|p| {
                        p.write_string16(Some(name));
                        p.write_bool(exists);
                    })
                    .unwrap(),
                    expected,
                    "{name}"
                );
            }
            for code in [
                policy::QUERY_PACKAGE_STATE_PROTECTED,
                policy::QUERY_PACKAGE_DATA_PROTECTED,
            ] {
                assert_eq!(
                    call(1000, code, &|p| {
                        p.write_string16(None);
                        p.write_i32(0);
                    })
                    .unwrap(),
                    0
                );
                let error = call(1000, code, &|p| {
                    p.write_string16(Some("owner"));
                    p.write_i32(0);
                })
                .unwrap_err();
                assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
                assert!(error.message.contains("#1521"));
            }
            for (user, expected) in [(0, 1), (10, 0)] {
                assert_eq!(
                    call(1000, policy::IS_SHELL_DEBUGGING_RESTRICTED, &|p| p
                        .write_i32(user))
                    .unwrap(),
                    expected
                );
            }
            assert_eq!(
                call(1000, policy::IS_INSTALL_DISABLED, &|p| {
                    p.write_string16(Some("p"));
                    p.write_i32(10100);
                    p.write_i32(0);
                })
                .unwrap(),
                1
            );
            // A zero low-space reserve cannot be low; a whole-volume reserve is.
            for (max, expected) in [("0", 0), ("9223372036854775807", 1)] {
                for (name, value) in [
                    ("sys_storage_threshold_percentage", "100"),
                    ("sys_storage_threshold_max_bytes", max),
                ] {
                    host.platform()
                        .unwrap()
                        .settings
                        .insert(
                            super::super::settings::Table::Global,
                            name,
                            &super::super::settings::Change {
                                value: Some(value),
                                tag: None,
                                make_default: false,
                                package: Some("android"),
                                caller_system: true,
                                overrideable_by_restore: false,
                            },
                            false,
                        )
                        .unwrap();
                }
                assert_eq!(
                    call(1000, policy::IS_STORAGE_LOW, &|_| {}).unwrap(),
                    expected
                );
            }
            let error = call(1000, policy::REPORT_ADMIN_PERMISSION_DENIED, &|_| {}).unwrap_err();
            assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
            assert!(error.message.contains("#2188"));
        });
    }
}
