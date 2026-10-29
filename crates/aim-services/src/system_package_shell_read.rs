//! Independent original shell syntax/Parcelable/resource leaf with native caller
//! authentication. Package reads and resolution remain the native public owner.
use super::*;
use aim_service_aidl::{
    WriteParcelable, dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackageshellreadleaf as leaf,
};

struct Descriptor(aim_binder_driver::File);
impl WriteParcelable for Descriptor {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_i32(0);
        parcel.write_file(self.0.clone());
    }
}

impl System {
    pub fn shell_read_command(
        &self,
        uid: u32,
        pid: i32,
        arguments: Vec<String>,
        descriptors: [aim_binder_host::server::RetainedFd; 3],
    ) -> Result<i32> {
        if self.process.authenticated_inbound_identity() != Some((pid, uid))
            || !matches!(uid, 0 | 2000)
        {
            return Err(Exception::security(
                "read shell caller lacks authenticated provenance",
            ));
        }
        let bridge = self.package_bootstrap()?;
        self.check_package_bootstrap(&bridge)?;
        let owner = self.package_bootstrap_node(&bridge, bootstrap::GET_PACKAGE_SHELL_READ_LEAF)?;
        let [input, output, error] = descriptors.map(|fd| Descriptor(fd.into_file_owner()));
        let mut request = Parcel::new();
        leaf::RunReadCommand {
            arguments: Some(arguments.into_iter().map(Some).collect()),
            input: Some(input),
            output: Some(output),
            error: Some(error),
        }
        .write(&mut request);
        let reply = match owner {
            ServiceOwner::Remote(owner) => self.process.transact_preserving_inbound(
                owner.handle,
                leaf::RUN_READ_COMMAND,
                &request,
            ),
            ServiceOwner::Local(owner) => owner.transact(leaf::RUN_READ_COMMAND, &request, false),
        }
        .map_err(shell_read_transport)?;
        self.check_package_bootstrap(&bridge)?;
        let mut reader = reply.reader();
        let status =
            leaf::read_run_read_command_reply(&mut reader).map_err(shell_read_transport)??;
        if reader.remaining() != 0 {
            return Err(shell_read_failure("read shell reply tail"));
        }
        Ok(status)
    }
    pub fn shell_read_leaf(
        &self,
        uid: u32,
        pid: i32,
        code: u32,
        request: Parcel,
    ) -> Result<Parcel> {
        if self.process.authenticated_inbound_identity() != Some((pid, uid)) {
            return Err(Exception::security(
                "shell read leaf caller lacks authenticated Binder provenance",
            ));
        }
        let calling_uid = i32::try_from(uid)
            .map_err(|_| Exception::illegal_argument("shell UID exceeds Android range"))?;
        if !request.objects().is_empty() || !request.files().is_empty() {
            return Err(Exception::illegal_argument(
                "shell read leaf does not accept capabilities",
            ));
        }
        let mut reader = request.reader();
        match code {
            leaf::GET_MAX_SUPPORTED_USERS => {
                leaf::GetMaxSupportedUsers::read(&mut reader).map_err(shell_read_transport)?;
            }
            leaf::PARSE_INTENT => {
                leaf::ParseIntent::read(&mut reader).map_err(shell_read_transport)?;
            }
            leaf::DUMP_RESOLVE_INFO => {
                leaf::DumpResolveInfo::read(&mut reader).map_err(shell_read_transport)?;
            }
            leaf::LIST_PERMISSION_GROUPS => {
                let args =
                    leaf::ListPermissionGroups::read(&mut reader).map_err(shell_read_transport)?;
                if args.calling_uid != calling_uid || args.calling_pid != pid {
                    return Err(Exception::security(
                        "permission groups caller differs from authenticated caller",
                    ));
                }
            }
            leaf::LIST_PERMISSIONS => {
                let args =
                    leaf::ListPermissions::read(&mut reader).map_err(shell_read_transport)?;
                if args.calling_uid != calling_uid || args.calling_pid != pid {
                    return Err(Exception::security(
                        "permission shell metadata caller differs from authenticated caller",
                    ));
                }
            }
            leaf::LIST_INSTRUMENTATION => {
                let args =
                    leaf::ListInstrumentation::read(&mut reader).map_err(shell_read_transport)?;
                if args.calling_uid != calling_uid || args.calling_pid != pid {
                    return Err(Exception::security(
                        "instrumentation shell metadata caller differs from authenticated caller",
                    ));
                }
            }
            _ => {
                return Err(Exception::illegal_argument(
                    "unknown independent shell read operation",
                ));
            }
        }
        if reader.remaining() != 0 {
            return Err(Exception::illegal_argument("shell read leaf argument tail"));
        }
        let bridge = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_ref()
                .ok_or_else(|| shell_read_failure("shell package bootstrap unavailable"))?;
            if current.queries.is_none() {
                return Err(shell_read_failure(
                    "shell full query capture not initialized",
                ));
            }
            current.bridge.clone()
        };
        self.check_package_bootstrap(&bridge)?;
        // Java installs and retains this helper against the actual full native
        // Store. No callback or remote transaction occurs under bootstrap lock.
        let owner = self.package_bootstrap_node(&bridge, bootstrap::GET_PACKAGE_SHELL_READ_LEAF)?;
        let reply = owner
            .transact_as_owner(code, &request, false)
            .map_err(shell_read_transport)?;
        self.check_package_bootstrap(&bridge)?;
        let reply = reply.into_parcel();
        if !reply.objects().is_empty() {
            return Err(shell_read_failure(
                "shell read leaf returned unexpected capabilities",
            ));
        }
        Ok(reply)
    }
}
fn shell_read_failure(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}
fn shell_read_transport(status: i32) -> Exception {
    shell_read_failure(format!("independent shell read transport: {status}"))
}
