//! Independent original shell syntax/Parcelable/resource leaf with native caller
//! authentication. Package reads and resolution remain the native public owner.
use super::*;
use aim_service_aidl::{ReadParcelable,dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackageshellreadleaf as leaf};

impl System {
    pub fn shell_read_leaf(&self, uid:u32, pid:i32, code:u32, request:Parcel)->Result<Parcel> {
        if self.process.authenticated_inbound_identity()!=Some((pid,uid)) {
            return Err(Exception::security("shell read leaf caller lacks authenticated Binder provenance"));
        }
        let calling_uid=i32::try_from(uid).map_err(|_|Exception::illegal_argument("shell UID exceeds Android range"))?;
        if !request.objects().is_empty() || !request.files().is_empty() {
            return Err(Exception::illegal_argument("shell read leaf does not accept capabilities"));
        }
        let mut reader=request.reader();
        match code {
            leaf::PARSE_INTENT=>{leaf::ParseIntent::read(&mut reader).map_err(shell_read_transport)?;},
            leaf::DUMP_RESOLVE_INFO=>{leaf::DumpResolveInfo::read(&mut reader).map_err(shell_read_transport)?;},
            leaf::LIST_PERMISSIONS=>{
                let args=leaf::ListPermissions::read(&mut reader).map_err(shell_read_transport)?;
                if args.calling_uid!=calling_uid || args.calling_pid!=pid {
                    return Err(Exception::security("permission shell metadata caller differs from authenticated caller"));
                }
            },
            leaf::LIST_INSTRUMENTATION=>{
                let args=leaf::ListInstrumentation::read(&mut reader).map_err(shell_read_transport)?;
                if args.calling_uid!=calling_uid || args.calling_pid!=pid {
                    return Err(Exception::security("instrumentation shell metadata caller differs from authenticated caller"));
                }
            },
            _=>return Err(Exception::illegal_argument("unknown independent shell read operation")),
        }
        if reader.remaining()!=0 {return Err(Exception::illegal_argument("shell read leaf argument tail"));}
        let bridge={
            let state=self.package_bootstrap.lock().unwrap();
            let current=state.current.as_ref().ok_or_else(||shell_read_failure("shell package bootstrap unavailable"))?;
            if current.queries.is_none() {return Err(shell_read_failure("shell full query capture not initialized"));}
            current.bridge.clone()
        };
        self.check_package_bootstrap(&bridge)?;
        // Java installs and retains this helper against the actual full native
        // Store. No callback or remote transaction occurs under bootstrap lock.
        let owner=self.package_bootstrap_binder_leaf(&bridge,bootstrap::GET_PACKAGE_SHELL_READ_LEAF)?;
        let reply=owner.transact(code,&request,false).map_err(shell_read_transport)?;
        self.check_package_bootstrap(&bridge)?;
        let reply=reply.into_parcel();
        if !reply.objects().is_empty() {return Err(shell_read_failure("shell read leaf returned unexpected capabilities"));}
        Ok(reply)
    }
}
fn shell_read_failure(message:impl Into<String>)->Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)
}
fn shell_read_transport(status:i32)->Exception {shell_read_failure(format!("independent shell read transport: {status}"))}
