//! Original package shell operations invoke the same native AIDL bodies with
//! the authenticated inbound caller. Never forward a local capability remotely.
use aim_binder_host::{local::{Call,LocalProcess,Service},parcel::{Binder,Parcel,Reader,Exception,EX_ILLEGAL_STATE}};
use std::{sync::Arc,marker::PhantomData,rc::Rc};

pub struct Context<'a> {
    pub uid:u32,
    pub pid:i32,
    pub command:crate::shell::ShellCommand,
    process:Arc<LocalProcess>,
    public:&'a dyn Service,
    system:std::sync::Weak<crate::system::System>,
    receivers:std::sync::Mutex<std::collections::BTreeMap<u64,Arc<Receiver>>>,
    _synchronous:PhantomData<Rc<()>>,
}
impl<'a> Context<'a> {
    pub fn new(system:&Arc<crate::system::System>,public:&'a dyn Service,call:&mut Call<'_>)->Result<Self,i32>{
        let process=system.binder_process();
        let command=crate::shell::ShellCommand::read(&process,&mut call.data)?;
        if call.data.remaining()!=0{return Err(aim_binder_host::parcel::BAD_VALUE);}
        Ok(Self{uid:call.sender_euid,pid:call.sender_pid,command,process,public,system:Arc::downgrade(system),receivers:Default::default(),_synchronous:PhantomData})
    }
    pub fn system(&self)->Result<Arc<crate::system::System>,Exception>{
        self.system.upgrade().ok_or_else(||failure("shell native system stopped"))
    }
    pub fn capture(&self)->Result<Arc<super::scan_snapshot::query_state::Capture>,Exception>{self.system()?.capture_package_queries()}
    pub fn process(&self)->&Arc<LocalProcess>{&self.process}
    pub fn input_clone(&mut self)->Result<std::fs::File,Exception>{
        self.command.input().ok_or_else(||Exception::illegal_argument("shell input descriptor missing"))?
            .try_clone().map_err(|error|failure(format!("shell input descriptor: {error}")))
    }
    pub fn publish_receiver(&self,service:Arc<dyn Service>)->Result<Binder,Exception>{
        let identity=self.process.authenticated_inbound_identity().ok_or_else(||failure("shell receiver outside authenticated inbound call"))?;
        if identity!=(self.pid,self.uid){return Err(failure("shell receiver caller differs"));}
        let receiver=Arc::new(Receiver{inner:std::sync::Mutex::new(Some(service))});
        let binder=self.process.add_service(receiver.clone());
        if let Binder::Local(ptr)=binder{self.receivers.lock().unwrap().insert(ptr,receiver);}
        Ok(binder)
    }
    pub fn retire_receiver(&self,binder:Binder){if let Binder::Local(ptr)=binder{if let Some(receiver)=self.receivers.lock().unwrap().remove(&ptr){receiver.inner.lock().unwrap().take();}}}
    pub fn install_input(&mut self,path:Option<&str>)->Result<std::fs::File,Exception>{self.command.open_input(&self.process,path)}
    pub fn translate_user(&self,user:i32,all_default:i32,operation:&str)->Result<i32,Exception>{self.system()?.shell_handle_incoming_user(self.pid,self.uid,user,all_default,operation)}
    pub fn user_exists(&self,user:i32)->Result<bool,Exception>{Ok(self.capture()?.state().users.contains_key(&user))}
    pub fn boot_completed(&self)->Result<bool,Exception>{self.system()?.shell_boot_completed()}
    pub fn invoke_permission(&self,code:u32,request:Parcel)->Result<Parcel,Exception>{self.system()?.shell_permission_call(self.uid,self.pid,code,request)}
    pub fn reset_runtime_permissions(&self)->Result<(),Exception>{self.system()?.shell_reset_runtime_permissions(self.uid,self.pid)}
    pub fn clear_data(&self,package:&str,user:i32,cache_only:bool)->Result<bool,Exception>{self.system()?.shell_clear_data(self.uid,self.pid,package,user,cache_only)}
    pub fn requested_runtime_permissions(&self,package:Option<&str>,user:i32)->Result<Vec<(String,Vec<String>)>,Exception>{self.system()?.shell_requested_runtime_permissions(self.uid,self.pid,package,user)}
    pub fn read_leaf(&self,code:u32,request:Parcel)->Result<Parcel,Exception>{self.system()?.shell_read_leaf(self.uid,self.pid,code,request)}
    pub fn uninstall_package_flags(&self,name:&str,user:i32)->Result<Option<(bool,bool)>,Exception>{
        let capture=self.capture()?;let Some(package)=capture.state().packages.get(name)else{return Ok(None)};
        if !package.users.get(&user).is_some_and(|state|state.installed){return Ok(None);}
        Ok(Some((package.is.system,package.pkg.as_ref().is_some_and(|code|code.is2(super::pkg::booleans2::APEX)))))
    }
    pub fn uninstall_apex(&self,name:&str,version:i64,user:i32,receiver:super::installer::preapproval::IntentSender,flags:i32)->Result<(),Exception>{self.system()?.shell_uninstall_apex(self.uid,self.pid,name,version,user,receiver,flags)}
    pub fn install_size(&mut self,path:&str,abi:Option<&str>)->Result<i64,Exception>{let file=self.install_input(Some(path))?;self.system()?.shell_install_size(self.uid,self.pid,file,path,abi)}
    pub fn validate_install_abi(&self,abi:&str)->Result<(),Exception>{self.system()?.shell_validate_install_abi(self.uid,self.pid,abi)}
    pub fn validate_install_compiler_filter(&self,filter:&str)->Result<(),Exception>{self.system()?.shell_validate_install_compiler_filter(self.uid,self.pid,filter)}
    pub fn sdk_dependency_installer_enabled(&self)->Result<bool,Exception>{self.system()?.shell_dependency_installer_enabled(self.uid,self.pid)}
    pub fn invoke_public(&self,code:u32,request:Parcel)->Result<Parcel,Exception>{
        let identity=self.process.authenticated_inbound_identity().ok_or_else(||failure("public shell invocation outside authenticated inbound call"))?;
        if identity!=(self.pid,self.uid){return Err(failure("public shell caller differs from authenticated inbound identity"));}
        if code==crate::shell::SHELL_COMMAND_TRANSACTION{return Err(failure("recursive package shell invocation"));}
        if !request.files().is_empty(){return Err(failure("public shell invocation requires transferred input descriptors"));}
        self.public.transact(&mut Call{code,flags:0,sender_pid:self.pid,sender_euid:self.uid,
            data:Reader::new(request.data(),request.objects())}).map_err(transport)
    }
    pub fn invoke(&self,target:Binder,code:u32,request:Parcel)->Result<Parcel,Exception>{
        let Binder::Local(ptr)=target else{return Err(failure("shell invocation cannot elevate a remote service"));};
        let identity=self.process.authenticated_inbound_identity().ok_or_else(||failure("shell invocation outside authenticated inbound call"))?;
        if identity!=(self.pid,self.uid){return Err(failure("shell caller differs from authenticated inbound identity"));}
        self.process.local_service(ptr).ok_or_else(||failure("shell local capability retired"))?
            .transact(code,&request,false).map(|reply|reply.into_parcel()).map_err(transport)
    }
    pub fn finish(self,result:i32){for receiver in self.receivers.lock().unwrap().values(){receiver.inner.lock().unwrap().take();}self.command.finish(result)}
}
struct Receiver{inner:std::sync::Mutex<Option<Arc<dyn Service>>>}
impl Service for Receiver{
    fn descriptor(&self)->&str{"android.content.IIntentSender"}
    fn accepts_fds(&self)->bool{true}
    fn transact(&self,call:&mut Call<'_>)->aim_binder_host::local::Reply{
        self.inner.lock().unwrap().clone().ok_or(aim_binder_host::parcel::DEAD_OBJECT)?.transact(call)
    }
}
fn failure(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
fn transport(status:i32)->Exception{failure(format!("native shell AIDL transport: {status}"))}

pub fn dispatch(mut context:Context<'_>){
    if !context.command.has_output(){return;}
    let result=run(&mut context);
    let status=match result {
        Ok(Some(status))=>status,
        Ok(None)=>{
            let command=context.command.command().unwrap_or_default().to_owned();
            context.command.eprintln(&format!("Unknown command: {command}"));-1
        }
        Err(error)=>{
            let class=match error.code{
                aim_binder_host::parcel::EX_SECURITY=>"java.lang.SecurityException",
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT=>"java.lang.IllegalArgumentException",
                _=>"java.lang.IllegalStateException",
            };
            context.command.exception(&format!("{class}: {}",error.message));-1
        }
    };
    context.finish(status);
}
fn run(context:&mut Context<'_>)->Result<Option<i32>,Exception>{
    if let Some(status)=super::shell_read::run(context)?{return Ok(Some(status));}
    if let Some(status)=super::shell_install::run(context)?{return Ok(Some(status));}
    super::shell_mutation::run(context)
}
