//! PackageManagerShellCommand's common owners with authenticated inbound identity.
use super::*;
use aim_binder_host::local::{Call,Reply,Service};
use aim_binder_host::parcel::{BAD_VALUE,UNKNOWN_TRANSACTION};
use aim_service_aidl::{dev_aim_server_ipackageshellpolicybridge as policy_api,
    dev_aim_server_ipackagebootstrapbridge as bootstrap_api,
    android_content_pm_ipackagedataobserver as observer_api};
use std::sync::Condvar;

fn failure(message:impl Into<String>)->Exception {Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}
struct ClearObserver {state:Mutex<(bool,Option<bool>)>,wake:Condvar}
impl ClearObserver {
    fn wait(&self)->bool {
        let mut state=self.state.lock().unwrap();
        while state.1.is_none(){state=self.wake.wait(state).unwrap();}
        state.0=false;state.1.unwrap()
    }
    fn retire(&self){self.state.lock().unwrap().0=false;}
}
impl Service for ClearObserver {
    fn descriptor(&self)->&'static str {observer_api::DESCRIPTOR}
    fn transact(&self,call:&mut Call<'_>)->Reply {
        if call.code!=observer_api::ON_REMOVE_COMPLETED{return Err(UNKNOWN_TRANSACTION);}
        let mut reply=Parcel::new();
        if !matches!(call.sender_euid,0|1000){reply.write_exception(&Exception::security("Untrusted shell clear observer caller"));return Ok(reply);}
        call.data.enforce_interface(observer_api::DESCRIPTOR)?;
        let _package=call.data.read_string16()?;let success=call.data.read_bool()?;
        if call.data.remaining()!=0{return Err(BAD_VALUE);}
        let mut state=self.state.lock().unwrap();
        if !state.0{return Err(aim_binder_host::parcel::DEAD_OBJECT);}
        if state.1.is_some(){reply.write_exception(&failure("Shell clear observer already completed"));return Ok(reply);}
        state.1=Some(success);self.wake.notify_all();reply.write_no_exception();Ok(reply)
    }
}
impl System {
    fn authenticate_package_shell(&self,uid:u32,pid:i32)->Result<i32> {
        if pid<=0||self.process.authenticated_inbound_identity()!=Some((pid,uid)) {
            return Err(Exception::security("Package shell caller differs from authenticated inbound identity"));
        }
        i32::try_from(uid).map_err(|_|Exception::illegal_argument("Package shell UID exceeds Android range"))
    }
    fn package_shell_policy(&self)->Result<Strong> {
        let bridge=self.package_bootstrap()?;self.check_package_bootstrap(&bridge)?;
        let mut request=Parcel::new();request.write_interface_token(bootstrap_api::DESCRIPTOR);
        let reply=bridge.owner.transact(bootstrap_api::GET_PACKAGE_SHELL_POLICY_BRIDGE,&request,false)
            .map_err(|status|failure(format!("Shell policy attachment: {status}")))?;
        let mut reader=reply.reader();reader.read_exception().map_err(|status|failure(status.to_string()))??;
        let binder=reader.read_binder().map_err(|status|failure(status.to_string()))?
            .ok_or_else(||failure("Shell policy owner unavailable"))?;
        if reader.remaining()!=0{return Err(failure("Shell policy attachment trailing data"));}
        let owner=reply.retain_remote_binder(binder).map_err(|status|failure(status.to_string()))?;
        self.check_package_bootstrap(&bridge)?;Ok(owner)
    }
    fn shell_policy_call(&self,code:u32,fill:impl FnOnce(&mut Parcel))->Result<Received> {
        let owner=self.package_shell_policy()?;let mut request=Parcel::new();
        request.write_interface_token(policy_api::DESCRIPTOR);fill(&mut request);
        owner.transact(code,&request,false).map_err(|status|failure(format!("Shell policy transport: {status}")))
    }
    pub fn shell_handle_incoming_user(self:&Arc<Self>,pid:i32,uid:u32,user:i32,all_default:i32,operation:&str)->Result<i32> {
        let uid=self.authenticate_package_shell(uid,pid)?;
        let result=if all_default==-10000 {
            self.handle_incoming_user(pid,uid,user,true,operation,Some("pm command"))?
        }else {
            let args=am::HandleIncomingUser {calling_pid:pid,calling_uid:uid,user_id:user,allow_all:true,
                require_full:true,name:Some(operation.into()),caller_package:Some("pm command".into())};
            self.call("activity",am::HANDLE_INCOMING_USER,|request|args.write(request),am::read_handle_incoming_user_reply)?
        };
        Ok(if result==-1{all_default}else{result})
    }
    pub fn shell_boot_completed(&self)->Result<bool> {
        let (pid,uid)=self.process.authenticated_inbound_identity().ok_or_else(||Exception::security("Boot-completed shell query is outside inbound call"))?;
        self.authenticate_package_shell(uid,pid)?;
        let reply=self.shell_policy_call(policy_api::BOOT_COMPLETED,|_|{})?;let mut reader=reply.reader();
        reader.read_exception().map_err(|status|failure(status.to_string()))??;
        let value=reader.read_bool().map_err(|status|failure(status.to_string()))?;
        if reader.remaining()!=0{return Err(failure("Boot-completed reply trailing data"));}Ok(value)
    }
    pub fn shell_permission_call(self:&Arc<Self>,uid:u32,pid:i32,code:u32,request:Parcel)->Result<Parcel> {
        self.authenticate_package_shell(uid,pid)?;
        Reader::new(request.data(),request.objects()).enforce_interface(pm::DESCRIPTOR).map_err(|status|failure(format!("Shell permission interface: {status}")))?;
        if !request.files().is_empty()||!request.objects().is_empty(){return Err(Exception::illegal_argument("Shell permission command carries capabilities"));}
        let reply=self.package_permission_call(code,&request)?;
        let success=reply.reader().read_exception().map_err(|status|failure(status.to_string()))?.is_ok();
        if success&&matches!(code,pm::GRANT_RUNTIME_PERMISSION|pm::REVOKE_RUNTIME_PERMISSION|pm::UPDATE_PERMISSION_FLAGS) {
            let bridge=self.package_bootstrap()?;let capture=self.capture_package_queries()?;
            self.refresh_package_permission_queries(&bridge,&capture)?;
        }
        Ok(reply.into_parcel())
    }
    pub fn shell_reset_runtime_permissions(self:&Arc<Self>,uid:u32,pid:i32)->Result<()> {
        let caller=self.authenticate_package_shell(uid,pid)?;
        if !self.check_permission("android.permission.REVOKE_RUNTIME_PERMISSIONS",pid,caller)? {
            return Err(Exception::security("reset-permissions requires REVOKE_RUNTIME_PERMISSIONS"));
        }
        if !matches!(caller,0|1000)&&!self.check_permission("android.permission.INTERACT_ACROSS_USERS_FULL",pid,caller)? {
            return Err(Exception::security("reset-permissions requires INTERACT_ACROSS_USERS_FULL"));
        }
        let reply=self.shell_policy_call(policy_api::RESET_RUNTIME_PERMISSIONS,|request|{request.write_i32(pid);request.write_i32(caller);})?;
        let mut reader=reply.reader();reader.read_exception().map_err(|status|failure(status.to_string()))??;
        if reader.remaining()!=0{return Err(failure("Permission reset reply trailing data"));}
        let bridge=self.package_bootstrap()?;let capture=self.capture_package_queries()?;
        self.refresh_package_permission_queries(&bridge,&capture)
    }
    pub fn shell_clear_data(self:&Arc<Self>,uid:u32,pid:i32,package:&str,user:i32,cache_only:bool)->Result<bool> {
        let caller=self.authenticate_package_shell(uid,pid)?;
        let observer=Arc::new(ClearObserver {state:Mutex::new((true,None)),wake:Condvar::new()});
        let binder=self.process.add_service(observer.clone());
        let result=(|| -> Result<()> {
            if cache_only {
                let capture=self.capture_package_queries()?;let resolver=crate::package::resolve::Resolver::default();
                let resolution=resolver.resolution(capture.state()).map_err(|error|failure(format!("Shell cache resolution: {error:?}")))?;
                let query=crate::package::query::Query {state:capture.state(),filter:&resolution.apps_filter,calling_uid:caller};
                let mut request=Parcel::new();package::DeleteApplicationCacheFilesAsUser {
                    package_name:Some(package.into()),user_id:user,observer:Some(binder),
                }.write(&mut request);
                let reply=self.package_maintenance_owner()?.answer(&mut Call {code:package::DELETE_APPLICATION_CACHE_FILES_AS_USER,
                    flags:0,sender_pid:pid,sender_euid:uid,data:Reader::new(request.data(),request.objects())},&query)
                    .ok_or_else(||failure("Native cache command owner unavailable"))?
                    .map_err(|status|failure(format!("Native cache command: {status}")))?;
                let mut reader=Reader::new(reply.data(),reply.objects());reader.read_exception().map_err(|status|failure(status.to_string()))??;
                if reader.remaining()!=0{return Err(failure("Cache clearing reply trailing data"));}
            }else {
                let mut request=Parcel::new();request.write_interface_token(am::DESCRIPTOR);
                request.write_string16(Some(package));request.write_bool(false);
                request.write_binder(Some(binder));request.write_i32(user);
                let service=self.service("activity")?;
                let reply=match service {
                    ServiceOwner::Remote(owner)=>self.process.transact_preserving_inbound(owner.handle,am::CLEAR_APPLICATION_USER_DATA,&request)
                        .map_err(|status|unreachable_service("activity",status))?,
                    ServiceOwner::Local(owner)=>owner.transact(am::CLEAR_APPLICATION_USER_DATA,&request,false)
                        .map_err(|status|unreachable_service("activity",status))?,
                };
                let mut reader=reply.reader();reader.read_exception().map_err(|status|failure(status.to_string()))??;
                reader.read_bool().map_err(|status|failure(status.to_string()))?;
                if reader.remaining()!=0{return Err(failure("AM clearing reply trailing data"));}
            }
            Ok(())
        })();
        if let Err(error)=result {observer.retire();return Err(error);}
        Ok(observer.wait())
    }
    pub fn shell_requested_runtime_permissions(self:&Arc<Self>,uid:u32,pid:i32,package:Option<&str>,user:i32)->Result<Vec<(String,Vec<String>)>> {
        let caller=self.authenticate_package_shell(uid,pid)?;
        let bridge=self.package_bootstrap()?;self.check_package_bootstrap(&bridge)?;
        let capture=self.capture_package_queries()?;
        let resolution=capture.resolution().map_err(|error|failure(format!("Shell permission query resolution: {error:?}")))?;
        let query=crate::package::query::Query {state:capture.state(),filter:&resolution.apps_filter,calling_uid:caller};
        let flags=crate::package::info::flags::GET_PERMISSIONS;
        let infos=if let Some(name)=package {
            vec![query.package_info_internal(name,-1,flags,user,caller).map_err(crate::package::installer::policy::unknown)??
                .ok_or_else(||Exception::illegal_argument("Package not found"))?]
        }else {query.installed_packages(flags,user).map_err(crate::package::installer::policy::unknown)??};
        let mut targets=Vec::new();
        for info in infos {
            let name=info.package_name.ok_or_else(||failure("Shell package info has no identity"))?;
            let requested=info.requested_permissions.unwrap_or_default();
            let reply=self.shell_policy_call(policy_api::RUNTIME_PERMISSIONS,|request|{
                request.write_i32(requested.len() as i32);for name in &requested{request.write_string16(Some(name));}
            })?;
            let mut reader=reply.reader();reader.read_exception().map_err(|status|failure(status.to_string()))??;
            let permissions=aim_service_aidl::read_string_list(&mut reader).map_err(|status|failure(status.to_string()))?
                .ok_or_else(||failure("Original runtime permission names unavailable"))?
                .into_iter().map(|name|name.ok_or_else(||failure("Original runtime permission name is null"))).collect::<Result<Vec<_>>>()?;
            if reader.remaining()!=0{return Err(failure("Runtime permission names trailing data"));}
            targets.push((name,permissions));
        }
        self.check_package_bootstrap(&bridge)?;
        Ok(targets)
    }
}
