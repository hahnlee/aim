//! `permission`: the permission controller native services ask
//! (`IPermissionController`, which ActivityManager publishes at
//! `android-16.0.0_r1`): `checkPermission` as ActivityManager decides it,
//! `noteOp` as AppOps notes it, and PackageManager's answers about uids
//! and permissions.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
use aim_service_aidl::android_os_ipermissioncontroller as pc;

use super::{AppHost, reply};

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
                let runtime = match host.inputs() {
                    Ok(inputs) => args.permission.is_some_and(|name| {
                        inputs.packages.permissions().iter().any(|perm| {
                            perm.name == name
                                && perm.protection_level & PROTECTION_MASK_BASE
                                    == PROTECTION_DANGEROUS
                        })
                    }),
                    Err(e) => return reply(Err(e)),
                };
                pc::write_is_runtime_permission_reply(&mut p, runtime);
            }
            code => return reply(Err(host.unimplemented("permission", code))),
        }
        Ok(p)
    }
}
