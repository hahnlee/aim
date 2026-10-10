//! Original InstallRequest external-profile warnings and child-session lifetime.
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable, read_string_list, write_string_list};
use std::{collections::{BTreeMap, BTreeSet}, sync::Mutex};

#[derive(Debug, PartialEq)]
pub(crate) enum DexoptResult {
    Completed {final_status:i32,external_profile_errors:Vec<String>},
    ObservedException {class:String,message:Option<String>},
}
impl ReadParcelable for DexoptResult {
    fn read_from(reader:&mut Reader<'_>)->Result<Self,i32> {
        let start=reader.position();let size=reader.read_i32()?;
        if size<24{return Err(BAD_VALUE)}
        let end=start.checked_add(size as usize).ok_or(BAD_VALUE)?;
        if end>reader.position()+reader.remaining(){return Err(BAD_VALUE)}
        let status=reader.read_i32()?;
        let errors=read_string_list(reader)?;
        let present=reader.read_i32()?;
        let class=reader.read_string16()?;let message=reader.read_string16()?;
        if reader.position()>end{return Err(BAD_VALUE)}
        reader.set_position(end);
        match (present,class,message,errors) {
            (1,None,None,Some(errors)) if matches!(status,10|20|30|40)=>Ok(Self::Completed {
                final_status:status,external_profile_errors:errors.into_iter().map(|value|value.ok_or(BAD_VALUE)).collect::<Result<Vec<_>,_>>()?,
            }),
            (0,Some(class),message,None) if status==0&&!class.is_empty()=>Ok(Self::ObservedException{class,message}),
            _=>Err(BAD_VALUE),
        }
    }
}
impl WriteParcelable for DexoptResult {
    fn write_to(&self,parcel:&mut Parcel) {
        let start=parcel.position();parcel.write_i32(0);
        match self {
            Self::Completed{final_status,external_profile_errors}=>{
                parcel.write_i32(*final_status);
                let values=external_profile_errors.iter().cloned().map(Some).collect::<Vec<_>>();
                write_string_list(parcel,Some(&values));parcel.write_bool(true);parcel.write_string16(None);parcel.write_string16(None);
            }
            Self::ObservedException{class,message}=>{
                // Java's unused primitive default is not a DexoptResult status.
                parcel.write_i32(0);write_string_list(parcel,None);parcel.write_bool(false);
                parcel.write_string16(Some(class));parcel.write_string16(message.as_deref());
            }
        }
        parcel.set_i32_at(start,(parcel.position()-start) as i32);
    }
}
fn error(message:&str)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Ticket { root:i32, token:u64, session:i32 }
struct Pending { token:u64, ordered:Vec<i32>, generation:Option<u64>, warnings:BTreeMap<i32,Vec<String>> }
#[derive(Default)]
struct State { next:u64, pending:BTreeMap<i32,Pending> }
#[derive(Default)]
pub(crate) struct Owner(Mutex<State>);
pub(crate) struct Attempt<'a> { owner:&'a Owner, root:i32, token:u64 }
impl Owner {
    pub fn begin(&self,root:i32,ordered:Vec<i32>)->Result<Attempt<'_>,Exception> {
        if ordered.first()!=Some(&root)||ordered.iter().copied().collect::<BTreeSet<_>>().len()!=ordered.len(){return Err(error("install warning session order invalid"))}
        let mut state=self.0.lock().unwrap();
        if state.pending.values().any(|pending|pending.ordered.iter().any(|id|ordered.contains(id))){return Err(error("install warning session attempt already active"))}
        state.next=state.next.checked_add(1).ok_or_else(||error("install warning attempt token exhausted"))?;
        let token=state.next;
        state.pending.insert(root,Pending{token,ordered,generation:None,warnings:BTreeMap::new()});
        Ok(Attempt{owner:self,root,token})
    }
    pub fn ticket(&self,session:i32)->Result<Ticket,Exception> {
        self.0.lock().unwrap().pending.iter().find(|(_,pending)|pending.ordered.contains(&session))
            .map(|(&root,pending)|Ticket{root,token:pending.token,session}).ok_or_else(||error("install warning attempt unavailable"))
    }
    pub fn record(&self,ticket:Ticket,generation:u64,adb:bool,errors:&[String])->Result<(),Exception> {
        self.record_outcome(ticket,generation,adb,Some(errors))
    }
    pub fn record_without_result(&self,ticket:Ticket,generation:u64)->Result<(),Exception> {
        self.record_outcome(ticket,generation,false,None)
    }
    fn record_outcome(&self,ticket:Ticket,generation:u64,adb:bool,errors:Option<&[String]>)->Result<(),Exception> {
        let mut state=self.0.lock().unwrap();
        let pending=state.pending.get_mut(&ticket.root).filter(|pending|pending.token==ticket.token&&pending.ordered.contains(&ticket.session))
            .ok_or_else(||error("install warning attempt retired or replaced"))?;
        if generation==0||pending.generation.is_some_and(|previous|previous!=generation){return Err(error("install warning publication generation differs"))}
        if pending.warnings.contains_key(&ticket.session){return Err(error("install warning child result already recorded"))}
        pending.generation=Some(generation);
        let mut unique=Vec::new();
        if let Some(errors)=errors {for message in errors {if !unique.contains(message){unique.push(message.clone());}}}
        let warnings=if adb&&!unique.is_empty(){vec![format!("Error occurred during dexopt when processing external profiles:\n  {}",unique.join("\n  "))]}else{Vec::new()};
        pending.warnings.insert(ticket.session,warnings);Ok(())
    }
    pub fn retire(&self,ids:&[i32]) {
        self.0.lock().unwrap().pending.retain(|_,pending|!pending.ordered.iter().any(|id|ids.contains(id)));
    }
}
impl Attempt<'_> {
    pub fn take(&self)->Result<Vec<String>,Exception> {
        let mut state=self.owner.0.lock().unwrap();
        if !state.pending.get(&self.root).is_some_and(|pending|pending.token==self.token){return Err(error("install warning attempt retired before delivery"))}
        let mut pending=state.pending.remove(&self.root).unwrap();
        let required=if pending.ordered.len()>1{&pending.ordered[1..]}else{&pending.ordered[..]};
        if required.iter().any(|id|!pending.warnings.contains_key(id)){return Err(error("install warning child ART result unavailable"))}
        let mut warnings=Vec::new();
        for id in pending.ordered {if let Some(child)=pending.warnings.remove(&id){warnings.extend(child);}}
        Ok(warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_order_and_actual_error_dedup_preserve_atomic_success_warning_shape() {
        let owner=Owner::default();let attempt=owner.begin(1,vec![1,20,10]).unwrap();
        let child=owner.ticket(10).unwrap();owner.record(child,9,true,&["same error".into(),"same error".into()]).unwrap();
        owner.record(owner.ticket(20).unwrap(),9,true,&["same error".into(),"other error".into()]).unwrap();
        let result=attempt.take().unwrap();assert_eq!(result.len(),2);
        assert_eq!(result[0],"Error occurred during dexopt when processing external profiles:\n  same error\n  other error");
        assert_eq!(result[1],"Error occurred during dexopt when processing external profiles:\n  same error");
        assert!(owner.record(child,9,true,&["late error".into()]).is_err());
        let next=owner.begin(1,vec![1]).unwrap();owner.record(owner.ticket(1).unwrap(),10,false,&["actual non-adb profile error".into()]).unwrap();
        assert!(next.take().unwrap().is_empty());
    }
    #[test]
    fn error_and_callback_failure_retire_tokens_without_reused_session_bleed() {
        let owner=Owner::default();
        let stale={let _attempt=owner.begin(1,vec![1]).unwrap();let ticket=owner.ticket(1).unwrap();owner.record(ticket,7,true,&["first attempt".into()]).unwrap();ticket};
        let attempt=owner.begin(1,vec![1]).unwrap();let ticket=owner.ticket(1).unwrap();
        assert!(owner.record(stale,7,true,&["stale".into()]).is_err());
        owner.record(ticket,8,true,&["new result".into()]).unwrap();
        let warnings=attempt.take().unwrap();assert_eq!(warnings.len(),1);assert!(warnings[0].contains("new result"));
        let callback:Result<(),Exception>=Err(error("receiver failed"));assert!(callback.is_err());drop(attempt);
        assert!(owner.ticket(1).is_err());
        let empty=owner.begin(1,vec![1]).unwrap();assert!(empty.take().is_err());assert!(owner.ticket(1).is_err());
    }
    #[test]
    fn generation_drift_duplicate_result_and_abandon_are_observable() {
        let owner=Owner::default();let attempt=owner.begin(1,vec![1,2,3]).unwrap();
        let first=owner.ticket(2).unwrap();owner.record(first,8,true,&[]).unwrap();
        assert!(owner.record(first,8,true,&[]).is_err());
        let second=owner.ticket(3).unwrap();assert!(owner.record(second,9,true,&["foreign generation".into()]).is_err());
        owner.retire(&[3]);assert!(attempt.take().is_err());assert!(owner.record(second,8,true,&[]).is_err());
        assert!(owner.begin(1,vec![1,1]).is_err());
        let next=owner.begin(1,vec![1]).unwrap();drop(attempt);
        owner.record(owner.ticket(1).unwrap(),10,true,&[]).unwrap();assert!(next.take().unwrap().is_empty());
    }
    #[test]
    fn typed_result_round_trip_rejects_truncation_and_unmodelled_status() {
        for status in [10,20,30,40] {
            let value=DexoptResult::Completed{final_status:status,external_profile_errors:vec!["actual fixture error".into()]};
            let mut parcel=Parcel::new();value.write_to(&mut parcel);
            assert_eq!(DexoptResult::read_from(&mut parcel.reader()).unwrap(),value);
            for size in 0..parcel.data().len(){assert!(DexoptResult::read_from(&mut Reader::new(&parcel.data()[..size],&[])).is_err());}
            parcel.set_i32_at(4,0);assert!(DexoptResult::read_from(&mut parcel.reader()).is_err());
        }
    }
    #[test]
    fn observed_throwable_has_no_status_or_profile_errors_and_completes_without_fabricated_warning() {
        for message in [None,Some("actual exception fixture".into())] {
            let diagnostic=DexoptResult::ObservedException{class:"java.lang.IllegalStateException".into(),message};
            let mut parcel=Parcel::new();diagnostic.write_to(&mut parcel);
            assert_eq!(DexoptResult::read_from(&mut parcel.reader()).unwrap(),diagnostic);
            for size in 0..parcel.data().len(){assert!(DexoptResult::read_from(&mut Reader::new(&parcel.data()[..size],&[])).is_err());}
            let owner=Owner::default();let attempt=owner.begin(1,vec![1]).unwrap();
            owner.record_without_result(owner.ticket(1).unwrap(),9).unwrap();assert!(attempt.take().unwrap().is_empty());
            parcel.set_i32_at(4,30);assert!(DexoptResult::read_from(&mut parcel.reader()).is_err());
            parcel.set_i32_at(4,0);parcel.set_i32_at(12,1);assert!(DexoptResult::read_from(&mut parcel.reader()).is_err());
        }
    }
    #[test]
    fn real_binder_result_and_session_warning_transport_preserve_status_and_owner_errors() {
        use aim_binder_driver::{Credentials,Device,Driver,Errno,File,GuestProcess,errno,uapi::*};
        use aim_binder_host::local::{Call,LocalProcess,Reply,Service};
        use aim_binder_host::parcel::{Binder,UNKNOWN_TRANSACTION};
        use aim_service_aidl::{dev_aim_server_iinstallercompletionbridge as art,dev_aim_server_iinstallerexternalbridge as external};
        use std::sync::{Arc,atomic::{AtomicI32,Ordering}};
        struct NoMemory;
        impl GuestProcess for NoMemory {
            fn copy_from_user(&mut self,_:u64,_:&mut[u8])->Result<(),Errno>{Err(errno::EFAULT)}
            fn copy_to_user(&mut self,_:u64,_:&[u8])->Result<(),Errno>{Err(errno::EFAULT)}
            fn get_file(&mut self,_:u32)->Result<File,Errno>{Err(errno::EBADF)}
            fn install_file(&mut self,_:File)->Result<u32,Errno>{Err(errno::EBADF)}
            fn close_fd(&mut self,_:u32){panic!("unexpected transport file")}
        }
        struct Processes{driver:Arc<Driver>,server:Arc<LocalProcess>,client:Arc<LocalProcess>}
        impl Drop for Processes{fn drop(&mut self){self.driver.release(self.client.proc_handle());self.driver.release(self.server.proc_handle());}}
        fn processes(service:Arc<dyn Service>,pid:i32)->Processes{
            let driver=Driver::new();let open=|id|LocalProcess::open(&driver,Device::Binder,Credentials{pid:id,euid:1000,security_context:None});
            let server=open(pid);let client=open(pid+1);let Binder::Local(ptr)=server.add_service(service)else{unreachable!()};
            let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:0,binder:ptr,cookie:ptr}.encode();
            driver.ioctl(server.proc_handle(),pid+2,BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut NoMemory).unwrap();server.start();client.start();
            Processes{driver,server,client}
        }
        struct Art(Arc<AtomicI32>);
        impl Service for Art {
            fn descriptor(&self)->&str{art::DESCRIPTOR}
            fn transact(&self,call:&mut Call<'_>)->Reply{
                assert_eq!(call.sender_euid,1000);if call.code!=art::DEXOPT_INSTALLED_RESULT{return Err(UNKNOWN_TRANSACTION)}
                let args=art::DexoptInstalledResult::read(&mut call.data)?;
                assert_eq!(args.package_name.as_deref(),Some("transport-fixture"));assert_eq!(args.install_flags,0x20);assert_eq!(call.data.remaining(),0);
                let mut reply=Parcel::new();
                match self.0.load(Ordering::Acquire) {
                    1=>reply.write_exception(&error("actual owner fixture failure")),
                    2=>art::write_dexopt_installed_result_reply(&mut reply,Some(&DexoptResult::ObservedException{class:"java.lang.AssertionError".into(),message:None})),
                    _=>art::write_dexopt_installed_result_reply(&mut reply,Some(&DexoptResult::Completed{final_status:30,external_profile_errors:vec!["profile checksum mismatch fixture".into()]})),
                }
                Ok(reply)
            }
        }
        let mode=Arc::new(AtomicI32::new(0));let processes_art=processes(Arc::new(Art(mode.clone())),99801);let endpoint=processes_art.client.strong(0);
        let request=art::DexoptInstalledResult{package_name:Some("transport-fixture".into()),install_scenario:0,install_reason:0,install_flags:0x20,compiler_filter:None,debuggable:false,instant_app:false,apex:false,rollback_from_platform:false};
        let mut data=Parcel::new();request.write(&mut data);
        let reply=endpoint.transact(art::DEXOPT_INSTALLED_RESULT,&data,false).unwrap();
        let result=art::read_dexopt_installed_result_reply::<DexoptResult>(&mut reply.reader()).unwrap().unwrap().unwrap();
        let DexoptResult::Completed{final_status,external_profile_errors}=result else {panic!("actual result required")};assert_eq!(final_status,30);
        let owner=Owner::default();let attempt=owner.begin(7,vec![7]).unwrap();owner.record(owner.ticket(7).unwrap(),10,true,&external_profile_errors).unwrap();
        let warnings=attempt.take().unwrap();assert_eq!(warnings.len(),1);
        mode.store(1,Ordering::Release);let reply=endpoint.transact(art::DEXOPT_INSTALLED_RESULT,&data,false).unwrap();
        assert_eq!(art::read_dexopt_installed_result_reply::<DexoptResult>(&mut reply.reader()).unwrap().unwrap_err().message,"actual owner fixture failure");
        mode.store(2,Ordering::Release);let reply=endpoint.transact(art::DEXOPT_INSTALLED_RESULT,&data,false).unwrap();
        assert_eq!(art::read_dexopt_installed_result_reply::<DexoptResult>(&mut reply.reader()).unwrap().unwrap().unwrap(),DexoptResult::ObservedException{class:"java.lang.AssertionError".into(),message:None});
        drop(endpoint);drop(processes_art);
        struct StatusSink(Arc<Mutex<Vec<String>>>);
        impl Service for StatusSink {
            fn descriptor(&self)->&str{external::DESCRIPTOR}
            fn transact(&self,call:&mut Call<'_>)->Reply{
                if call.code!=external::SEND_SESSION_STATUS_WITH_WARNINGS{return Err(UNKNOWN_TRANSACTION)}
                let args=external::SendSessionStatusWithWarnings::<super::super::preapproval::IntentSender>::read(&mut call.data)?;
                assert_eq!(args.session_id,7);assert_eq!(args.legacy_status,1);assert!(!args.preapproval);assert_eq!(call.data.remaining(),0);
                *self.0.lock().unwrap()=args.warnings.ok_or(BAD_VALUE)?.into_iter().map(|value|value.ok_or(BAD_VALUE)).collect::<Result<Vec<_>,_>>()?;
                let mut reply=Parcel::new();external::write_send_session_status_with_warnings_reply(&mut reply);Ok(reply)
            }
        }
        struct Receiver;
        impl Service for Receiver {fn descriptor(&self)->&str{"transport.receiver"}fn transact(&self,_:&mut Call<'_>)->Reply{Err(UNKNOWN_TRANSACTION)}}
        let received=Arc::new(Mutex::new(Vec::new()));let process_status=processes(Arc::new(StatusSink(received.clone())),99901);
        let receiver=process_status.client.add_service(Arc::new(Receiver));
        let endpoint=super::super::preapproval::BridgeOwner::new(process_status.client.strong(0));
        endpoint.deliver_with_warnings(&super::super::preapproval::Status{receiver:super::super::preapproval::IntentSender{target:receiver},session_id:7,package:Some("transport-fixture".into()),legacy_status:1,message:None,preapproval:false,pending_installer:None},&warnings).unwrap();
        assert_eq!(*received.lock().unwrap(),warnings);drop(endpoint);drop(process_status);
    }
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        let mut state=self.owner.0.lock().unwrap();
        if state.pending.get(&self.root).is_some_and(|pending|pending.token==self.token){state.pending.remove(&self.root);}
    }
}
