//! `IInstallerPermissionBridge` for the app host: the install
//! environment's permission owner, which system_server's
//! `InstallerPermissionBridge` serves, to the system uid only. An
//! install's private access candidate, prepared and discarded through
//! PermissionManager (#2161).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerpermissionbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct InstallerPermissions {
    host: Arc<AppHost>,
}

impl InstallerPermissions {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerPermissions { host }
    }
}

impl Service for InstallerPermissions {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "native package leaf requires system UID",
            )));
        }
        let owner = self.host.permission_manager.get().ok_or(BAD_VALUE)?;
        let required = |value: Option<aim_binder_host::parcel::Binder>| {
            value.ok_or_else(|| Exception::illegal_argument("missing permission scope"))
        };
        match call.code {
            bridge::PREPARE => {
                let a = bridge::Prepare::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let bytes = owner.prepare_install(
                        required(a.candidate)?,
                        required(a.previous)?,
                        a.installation.as_deref().ok_or_else(|| {
                            Exception::illegal_argument("missing permission installation")
                        })?,
                    )?;
                    let mut out = aim_binder_host::parcel::Parcel::new();
                    bridge::write_prepare_reply(&mut out, &Some(bytes));
                    Ok(out)
                })())
            }
            bridge::ROLLBACK => {
                let a = bridge::Rollback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    owner.discard_install(
                        required(a.candidate)?,
                        a.package_names.as_deref().ok_or_else(|| {
                            Exception::illegal_argument("missing permission rollback names")
                        })?,
                    )?;
                    let mut out = aim_binder_host::parcel::Parcel::new();
                    bridge::write_rollback_reply(&mut out);
                    Ok(out)
                })())
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}
