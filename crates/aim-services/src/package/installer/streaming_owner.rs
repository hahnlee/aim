//! Concrete streaming DataLoaderManager state and owned stage FD writes.
use super::{Session,Record,storage,native::{StreamPreparation,Publisher},codec::Object};
use aim_binder_host::{local::{Strong,LocalProcess,Service,Call,Reply},parcel::{Exception,Parcel,Reader,Binder,EX_ILLEGAL_STATE,BAD_VALUE}};
use aim_service_aidl::{ReadParcelable,WriteParcelable,dev_aim_server_iinstallerpreparationbridge as api};
use std::{collections::BTreeMap,sync::{Arc,Mutex,Weak},io::{Read,Write},time::{Duration,Instant}};
pub type Resume=Arc<dyn Fn(i32)->Result<(),Exception>+Send+Sync>;
pub type Failure=Arc<dyn Fn(i32,i32,String)->Result<(),Exception>+Send+Sync>;
pub type Pending=Arc<dyn Fn(i32,String)->Result<(),Exception>+Send+Sync>;
pub type IncrementalInputs=Arc<dyn Fn(&Session,&Record)->Result<(Option<String>,Vec<u8>),Exception>+Send+Sync>;
#[derive(Clone)]
struct State{session:Session,record:Record,started:bool,finished:bool,failure:Option<String>,status_node:Option<Binder>,connector_node:Option<Binder>}
pub struct Owner{bridge:Strong,process:Arc<LocalProcess>,disk:Arc<Mutex<storage::Store>>,publisher:Publisher,resume:Resume,failure:Failure,pending:Pending,incremental:IncrementalInputs,states:Mutex<BTreeMap<i32,State>>,errors:Mutex<Vec<String>>,stopped:std::sync::atomic::AtomicBool}
fn transport(code:i32)->Exception{Exception::new(EX_ILLEGAL_STATE,format!("stream preparation transport: {code}"))}
impl Owner{
    pub fn new(bridge:Strong,process:Arc<LocalProcess>,disk:Arc<Mutex<storage::Store>>,publisher:Publisher,resume:Resume,failure:Failure,pending:Pending,incremental:IncrementalInputs)->Arc<Self>{Arc::new(Self{bridge,process,disk,publisher,resume,failure,pending,incremental,states:Mutex::new(BTreeMap::new()),errors:Mutex::new(vec![]),stopped:std::sync::atomic::AtomicBool::new(false)})}
    pub fn preparation(self:&Arc<Self>)->StreamPreparation{let owner=self.clone();Arc::new(move|session,record,path|owner.prepare(session,record,path))}
    pub fn production(bridge:Strong,process:Arc<LocalProcess>,disk:Arc<Mutex<storage::Store>>,publisher:Publisher,resume:Resume,failure:Failure,pending:Pending)->Arc<Self>{
        let owner=Arc::new_cyclic(|weak:&Weak<Self>|{
            let weak=weak.clone();
            Self{bridge,process,disk,publisher,resume,failure,pending,incremental:Arc::new(move|session,record|{
                let owner=weak.upgrade().ok_or_else(||transport(aim_binder_host::parcel::DEAD_OBJECT))?;
                let mut request=Parcel::new();request.write_interface_token(api::DESCRIPTOR);request.write_string16(record.params.app_package_name.as_deref());request.write_i32(session.user as i32);
                let reply=owner.bridge.transact(api::GET_INCREMENTAL_INPUTS,&request,false).map_err(transport)?;
                let mut reader=reply.reader();reader.read_exception().map_err(transport)??;
                let size=reader.read_i32().map_err(transport)?;if size<0{return Err(transport(BAD_VALUE));}let start=reader.position();reader.skip((size as usize+3)&!3).map_err(transport)?;
                let bytes=reader.since(start).0[..size as usize].to_vec();let mut record=Reader::new(&bytes,&[]);
                let inherited=record.read_string16().map_err(transport)?;let size=record.read_i32().map_err(transport)?;if size<0{return Err(transport(BAD_VALUE));}let start=record.position();record.skip((size as usize+3)&!3).map_err(transport)?;
                let timeouts=record.since(start).0[..size as usize].to_vec();if record.remaining()!=0{return Err(transport(BAD_VALUE));}Ok((inherited,timeouts))
            }),states:Mutex::new(BTreeMap::new()),errors:Mutex::new(vec![]),stopped:std::sync::atomic::AtomicBool::new(false)}
        });owner
    }
    pub fn prepare(self:&Arc<Self>,session:&Session,record:&Record,path:&str)->Result<bool,Exception>{
        if self.stopped.load(std::sync::atomic::Ordering::Acquire){return Err(Exception::new(EX_ILLEGAL_STATE,"stream owner stopped"));}
        let Some(params)=record.params.data_loader_params.as_ref() else{return Ok(true);};
        {
            let states=self.states.lock().unwrap();
            if let Some(state)=states.get(&session.id){
                if let Some(error)=&state.failure{return Err(Exception::new(EX_ILLEGAL_STATE,error.clone()));}
                if state.finished{return Ok(true);}
                if state.started{return Ok(false);}
            }
        }
        let connector=(self.publisher)(Arc::new(Connector{owner:Arc::downgrade(self),id:session.id}))?;
        let callback=(self.publisher)(Arc::new(Status{owner:Arc::downgrade(self),id:session.id}))?;
        self.states.lock().unwrap().insert(session.id,State{session:session.clone(),record:record.clone(),started:true,finished:false,failure:None,status_node:Some(callback),connector_node:Some(connector)});
        let mut params_reader=Reader::new(&params.bytes,&params.objects);
        params_reader.read_string16().map_err(transport)?;let start=params_reader.position();
        let size=params_reader.read_i32().map_err(transport)?;
        if size<8{return Err(transport(BAD_VALUE));}
        let incremental=params_reader.read_i32().map_err(transport)?==2;
        let mut request=Parcel::new();request.write_interface_token(api::DESCRIPTOR);request.write_i32(session.id);
        let incremental_inputs=if incremental{Some((self.incremental)(session,record)?)}else{None};
        if let Some((inherited,_))=&incremental_inputs{request.write_string16(Some(path));request.write_string16(inherited.as_deref());}
        aim_service_aidl::write_byte_array(&mut request,Some(&params.bytes));
        let added=session.installation_files.iter().filter(|file|file.name.as_deref().is_some_and(|name|!name.ends_with(".removed"))).collect::<Vec<_>>();
        request.write_i32(added.len() as i32);
        for file in added{
            request.write_i32(1);let start=request.position();request.write_i32(0);
            request.write_i32(file.location);request.write_string16(file.name.as_deref());request.write_i64(file.length);
            aim_service_aidl::write_byte_array(&mut request,file.metadata.as_deref());aim_service_aidl::write_byte_array(&mut request,file.signature.as_deref());
            request.set_i32_at(start,(request.position()-start) as i32);
        }
        let removed=session.installation_files.iter().filter_map(|file|file.name.as_deref().and_then(|name|name.strip_suffix(".removed")).map(str::to_owned)).collect::<Vec<_>>();
        if let Some((_,timeouts))=&incremental_inputs{aim_service_aidl::write_byte_array(&mut request,Some(timeouts));request.write_binder(Some(callback));}
        else{request.write_i32(removed.len() as i32);for name in removed{request.write_string16(Some(&name));}request.write_binder(Some(connector));request.write_binder(Some(callback));}
        let reply=self.bridge.transact(if incremental{api::PREPARE_INCREMENTAL}else{api::PREPARE_STREAMING},&request,false).map_err(transport)?;
        let mut reader=reply.reader();reader.read_exception().map_err(transport)??;
        if !reader.read_bool().map_err(transport)? && !incremental{
            self.states.lock().unwrap().remove(&session.id);
            return Err(Exception::new(EX_ILLEGAL_STATE,"Failed to initialize data loader"));
        }
        if reader.remaining()!=0{return Err(transport(BAD_VALUE));}
        Ok(false)
    }
    fn status(&self,id:i32,status:i32)->Result<(),Exception>{
        if self.stopped.load(std::sync::atomic::Ordering::Acquire){return Ok(());}
        let root={
            let mut states=self.states.lock().unwrap();let Some(state)=states.get_mut(&id) else{return Ok(());};
            if state.finished||state.session.destroyed{return Ok(());}
            match status{
                6=>state.finished=true,
                7|9=>{state.finished=true;state.failure=Some(if status==7{"Failed to prepare image."}else{"DataLoader reported unrecoverable failure."}.into());},
                8|0=>state.started=false,
                _=>return Ok(()),
            }
            if state.session.parent==-1{id}else{state.session.parent}
        };
        match status{6=>(self.resume)(root),7|9=>(self.failure)(root,-21,if status==7{"Failed to prepare image."}else{"DataLoader reported unrecoverable failure."}.into()),8|0=>(self.pending)(root,"DataLoader unavailable".into()),_=>Ok(())}
    }
    pub fn destroy(&self,id:i32)->Result<(),Exception>{
        let removed=self.states.lock().unwrap().remove(&id);if removed.is_none(){return Ok(());}
        let mut request=Parcel::new();request.write_interface_token(api::DESCRIPTOR);request.write_i32(id);
        let reply=self.bridge.transact(api::DESTROY_STREAMING,&request,false).map_err(transport)?;
        reply.reader().read_exception().map_err(transport)??;Ok(())
    }
    fn write(&self,id:i32,name:&str,offset:i64,length:i64,fd:u32)->Result<(),Exception>{
        if self.stopped.load(std::sync::atomic::Ordering::Acquire){return Err(Exception::new(EX_ILLEGAL_STATE,"stream owner stopped"));}
        let state=self.states.lock().unwrap().get(&id).cloned().ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"stream session unavailable"))?;
        if state.session.destroyed||state.finished{return Err(Exception::new(EX_ILLEGAL_STATE,"stream session no longer writable"));}
        if !state.session.installation_files.iter().any(|file|file.name.as_deref()==Some(name)&&!name.ends_with(".removed")){return Err(Exception::security("File name is not in the list of added files."));}
        let descriptor=self.process.file(fd).ok_or_else(||transport(BAD_VALUE))?;
        let incoming=aim_binder_host::server::file_fd(&descriptor).ok_or_else(||transport(BAD_VALUE))?;
        let mut input=std::fs::File::from(incoming);
        let mut output=self.disk.lock().unwrap().write_target(&state.session,&state.record,name,offset).map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.message))?;
        let mut left=length;let mut buffer=vec![0;131072];
        while left>0{if self.stopped.load(std::sync::atomic::Ordering::Acquire){return Err(Exception::new(EX_ILLEGAL_STATE,"stream owner stopped during write"));}let count=input.read(&mut buffer[..(left as usize).min(131072)]).map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.to_string()))?;if count==0{break;}output.write_all(&buffer[..count]).map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.to_string()))?;left-=count as i64;}
        output.sync_all().map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.to_string()))
    }
    pub fn stop(&self){self.stopped.store(true,std::sync::atomic::Ordering::Release);}
    pub fn close(&self)->Result<(),Exception>{
        self.stop();let ids=self.states.lock().unwrap().keys().copied().collect::<Vec<_>>();
        let mut first=None;for id in ids{if let Err(error)=self.destroy(id){self.errors.lock().unwrap().push(error.message.clone());if first.is_none(){first=Some(error);}}}
        if let Some(error)=first{Err(error)}else{Ok(())}
    }
    pub fn errors(&self)->Vec<String>{self.errors.lock().unwrap().clone()}
}
struct Connector{owner:Weak<Owner>,id:i32}
impl Service for Connector{
    fn descriptor(&self)->&str{"android.content.pm.IPackageInstallerSessionFileSystemConnector"}
    fn accepts_fds(&self)->bool{true}
    fn transact(&self,call:&mut Call<'_>)->Reply{
        use aim_service_aidl::android_content_pm_ipackageinstallersessionfilesystemconnector as aidl;
        if call.code!=aidl::WRITE_DATA{return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);}
        call.data.enforce_interface(aidl::DESCRIPTOR)?;
        let name=call.data.read_string16()?.ok_or(BAD_VALUE)?;let offset=call.data.read_i64()?;let length=call.data.read_i64()?;
        if call.data.read_i32()?==0{return Err(BAD_VALUE);}
        let has_comm=call.data.read_i32()?;let fd=call.data.read_fd()?;if has_comm!=0{call.data.read_fd()?;}
        if call.data.remaining()!=0{return Err(BAD_VALUE);}
        let owner=self.owner.upgrade().ok_or(aim_binder_host::parcel::DEAD_OBJECT)?;
        let mut reply=Parcel::new();match owner.write(self.id,&name,offset,length,fd){Ok(())=>reply.write_no_exception(),Err(error)=>reply.write_exception(&error)}Ok(reply)
    }
}
struct BundleStatus(i32);
impl ReadParcelable for BundleStatus{
    fn read_from(reader:&mut Reader<'_>)->Result<Self,i32>{
        let size=reader.read_i32()?;if size<0||reader.read_i32()?!=0x4c444e42{return Err(BAD_VALUE);}
        let end=reader.position()+size as usize;let count=reader.read_i32()?;let mut status=None;
        for _ in 0..count{let key=reader.read_string16()?;match reader.read_i32()?{1=>{let value=reader.read_i32()?;if key.as_deref()==Some("status"){status=Some(value);}},0=>{reader.read_string16()?;},_=>return Err(BAD_VALUE)}}
        if reader.position()!=end{return Err(BAD_VALUE);}Ok(Self(status.ok_or(BAD_VALUE)?))
    }
}
struct Status{owner:Weak<Owner>,id:i32}
impl Service for Status{
    fn descriptor(&self)->&str{aim_service_aidl::android_os_iremotecallback::DESCRIPTOR}
    fn transact(&self,call:&mut Call<'_>)->Reply{
        use aim_service_aidl::android_os_iremotecallback as aidl;
        if call.code!=aidl::SEND_RESULT{return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);}
        if call.sender_euid!=1000{return Err(aim_binder_host::parcel::PERMISSION_DENIED);}
        let args=aidl::SendResult::<BundleStatus>::read(&mut call.data)?;
        if let(Some(owner),Some(status))=(self.owner.upgrade(),args.data){if let Err(error)=owner.status(self.id,status.0){owner.errors.lock().unwrap().push(error.message);}}
        Ok(Parcel::new())
    }
}
