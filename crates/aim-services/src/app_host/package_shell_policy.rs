//! Package shell's property, permission and UserManager owners (android-16.0.0_r1,
//! PackageShellPolicyBridge / UserManagerServiceShellCommand, AOSP Apache-2.0).
use super::{AppHost, reply};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, dev_aim_server_ipackageshellpolicybridge as policy};
use std::sync::Arc;

struct Descriptor(u32);
impl ReadParcelable for Descriptor {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        if r.read_i32()? != 0 {
            return Err(BAD_VALUE);
        }
        r.read_fd().map(Self)
    }
}
pub struct ShellPolicy(pub Arc<AppHost>);
impl ShellPolicy {
    fn permissions(&self) -> Result<&super::permission_manager::PermissionManager, Exception> {
        self.0
            .permission_manager
            .get()
            .map(Arc::as_ref)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable"))
    }
}
impl Service for ShellPolicy {
    fn descriptor(&self) -> &str {
        policy::DESCRIPTOR
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = if call.code == policy::HANDLE_LIST_USERS_COMMAND {
            // This leaf, unlike its factory, executes as the actual shell.
            // The user owner checks MANAGE_USERS/CREATE_USERS on that caller.
            let a = policy::HandleListUsersCommand::<Descriptor>::read(&mut call.data)
                .map_err(|_| BAD_VALUE)?;
            (|| {
                if a.input.is_none() || a.error.is_none() {
                    return Err(Exception::illegal_argument("missing user shell descriptor"));
                }
                let fd = a
                    .output
                    .ok_or_else(|| Exception::illegal_argument("missing user shell output"))?
                    .0;
                let mut output = self
                    .0
                    .process
                    .file(fd)
                    .and_then(|file| aim_binder_host::server::file_fd(&file))
                    .ok_or_else(|| Exception::illegal_argument("invalid user shell output"))?;
                super::user_service::list_shell(
                    self.0.clone(),
                    call.sender_euid as i32,
                    &mut output,
                )
                .map(|v| policy::write_handle_list_users_command_reply(&mut p, v))
            })()
        } else if call.sender_euid != 1000 {
            Err(Exception::security("the bridge serves the system uid only"))
        } else {
            match call.code {
                policy::RUNTIME_PERMISSIONS => {
                    let a =
                        policy::RuntimePermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    self.permissions()
                        .and_then(|owner| owner.runtime_permissions(a.requested.as_deref()))
                        .map(|names| policy::write_runtime_permissions_reply(&mut p, &Some(names)))
                }
                policy::RESET_RUNTIME_PERMISSIONS => {
                    let a = policy::ResetRuntimePermissions::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    self.permissions()
                        .and_then(|owner| {
                            owner.reset_all_runtime_permissions(a.caller_pid, a.caller_uid)
                        })
                        .map(|()| policy::write_reset_runtime_permissions_reply(&mut p))
                }
                policy::BOOT_COMPLETED => {
                    policy::BootCompleted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    policy::write_boot_completed_reply(
                        &mut p,
                        super::system_boolean(
                            self.0.property("sys.boot_completed").as_deref(),
                            false,
                        ),
                    );
                    Ok(())
                }
                code => Err(self
                    .0
                    .unimplemented("package shell policy", policy::DESCRIPTOR, code)),
            }
        };
        reply(result.map(|()| p))
    }
}
