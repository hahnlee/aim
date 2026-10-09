//! Native controller for process-owned POSIX locks (#1189).
//! Only native process admission registers owners; Mach request payloads cannot.
use crate::{posix_control::{Client,Endpoint,Frame,Operation,Owner,Request},process_namespace::ProcessIdentity};
use std::{collections::BTreeMap,io,path::PathBuf,process::{Child,Command,Stdio},sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}},thread,time::{Duration,Instant}};
fn error(code:i32)->io::Error{io::Error::from_raw_os_error(code)}
type Key=(i32,u64,u64);
#[derive(Clone,Copy,Debug)]
pub struct Credentials{pub uid:u32,pub effective_uid:u32,pub effective_capabilities:u64}
/// Read the kernel-published identity for this exact registered incarnation.
pub fn read_credentials(table:&std::path::Path,actor:ProcessIdentity)->io::Result<Credentials>{
 use std::{io::Read,os::{fd::AsRawFd,unix::fs::OpenOptionsExt}};
 let namespace=crate::process_namespace::mount_namespace_of(table,actor)?;
 let mut record=crate::private_fd::PrivateFile::allocate(||std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(table.join(actor.host_pid.to_string())))?;
 let mut stat:libc::stat=unsafe{std::mem::zeroed()};if unsafe{libc::fstat(record.as_raw_fd(),&mut stat)}<0{return Err(io::Error::last_os_error());}if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(error(libc::EPROTO));}
 loop{if unsafe{libc::flock(record.as_raw_fd(),libc::LOCK_SH)}==0{break;}let failure=io::Error::last_os_error();if failure.kind()!=io::ErrorKind::Interrupted{return Err(failure);}}
 let mut text=String::new();record.by_ref().take(65537).read_to_string(&mut text)?;if text.len()>65536{return Err(error(libc::EPROTO));}
 let(mut uid,mut euid,mut capabilities)=(None,None,None);
 for line in text.lines(){let Some((name,value))=line.split_once('\t')else{continue;};match name{
  "uid"=>{if uid.is_some(){return Err(error(libc::EPROTO));}uid=Some(value.parse::<u32>().map_err(|_|error(libc::EPROTO))?);},
  "euid"=>{if euid.is_some(){return Err(error(libc::EPROTO));}euid=Some(value.parse::<u32>().map_err(|_|error(libc::EPROTO))?);},
  "cap_effective"=>{if capabilities.is_some(){return Err(error(libc::EPROTO));}capabilities=Some(match value.strip_prefix("0x"){Some(value)=>u64::from_str_radix(value,16),None=>value.parse()}.map_err(|_|error(libc::EPROTO))?);},_=>{}
 }}
 let uid=uid.ok_or_else(||error(libc::EPROTO))?;let effective_capabilities=capabilities.ok_or_else(||error(libc::EPROTO))?;
 if !actor.is_live()||crate::process_namespace::mount_namespace_of(table,actor)?!=namespace{return Err(error(libc::ESRCH));}
 // The v1 identity writer omits euid exactly when it equals uid.
 Ok(Credentials{uid,effective_uid:euid.unwrap_or(uid),effective_capabilities})
}
fn key(p:ProcessIdentity)->Key{(p.host_pid,p.start_seconds,p.start_microseconds)}
static NEXT:AtomicU64=AtomicU64::new(1);
#[derive(Clone)]
pub struct Config {pub endpoint:String,pub holder:PathBuf,pub startup_timeout:Duration}
#[derive(Clone,Debug)]
pub struct OwnerConfig {pub endpoint:String,pub process:ProcessIdentity}
impl OwnerConfig {
 /// Native init publishes this locator. Its authority is the separately
 /// retained init identity and audited Mach peer, not filesystem ownership.
 pub fn write(&self,path:&std::path::Path)->io::Result<()> {
  use std::{io::Write,os::unix::fs::OpenOptionsExt};
  if !self.process.is_live()||self.endpoint.is_empty()||self.endpoint.contains(['\t','\n','\0']){return Err(error(libc::EINVAL));}
  let text=format!("AIMPOSIX1\t{}\t{}\t{}\t{}\n",self.process.host_pid,self.process.start_seconds,self.process.start_microseconds,self.endpoint);
  let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)?;
  file.write_all(text.as_bytes())?;file.sync_all()
 }
 pub fn read(path:&std::path::Path,init:ProcessIdentity)->io::Result<Self>{
  use std::{io::Read,os::unix::fs::OpenOptionsExt};
  let mut file=std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)?;
  if !file.metadata()?.is_file(){return Err(error(libc::EINVAL));}
  let mut text=String::new();file.by_ref().take(1025).read_to_string(&mut text)?;
  if text.len()>1024||!text.ends_with('\n'){return Err(error(libc::EPROTO));}
  let fields=text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
  if fields.len()!=5||fields[0]!="AIMPOSIX1"||fields[4].is_empty(){return Err(error(libc::EPROTO));}
  let process=ProcessIdentity{host_pid:fields[1].parse().map_err(|_|error(libc::EPROTO))?,start_seconds:fields[2].parse().map_err(|_|error(libc::EPROTO))?,start_microseconds:fields[3].parse().map_err(|_|error(libc::EPROTO))?};
  if process!=init||!init.is_live(){return Err(error(libc::EPERM));}
  Ok(Self{endpoint:fields[4].into(),process})
 }
}
struct Holder {owner:Owner,process:ProcessIdentity,client:Client,child:Mutex<Option<Child>>,requests:Mutex<BTreeMap<u64,(Operation,u64)>>,next_request:AtomicU64,dispatch:Mutex<()>,rebinding:AtomicBool}
impl Holder {
 fn spawn(config:&Config,owner:Owner,controller:ProcessIdentity)->io::Result<Arc<Self>>{
  let name=format!("com.aim.posix-holder.{}.{}.{}",controller.host_pid,controller.start_microseconds,NEXT.fetch_add(1,Ordering::Relaxed));
  let mut child=Command::new(&config.holder).args(["--service-name",&name,"--controller-pid",&controller.host_pid.to_string(),"--controller-seconds",&controller.start_seconds.to_string(),"--controller-microseconds",&controller.start_microseconds.to_string()]).stdin(Stdio::null()).spawn()?;
  let result=(||{
   let process=ProcessIdentity::running(child.id()as i32)?;let deadline=Instant::now()+config.startup_timeout;
   let client=loop{match Client::lookup(&name){Ok(client)=>break client,Err(failure)if failure.raw_os_error()==Some(libc::ENOENT)=>{if child.try_wait()?.is_some(){return Err(error(libc::ECHILD));}if Instant::now()>=deadline{return Err(error(libc::ETIMEDOUT));}thread::sleep(Duration::from_millis(5));},Err(failure)=>return Err(failure)}};
   if !owner.process.is_live(){return Err(error(libc::ESRCH));}Ok((process,client))
  })();
  match result{Ok((process,client))=>Ok(Arc::new(Self{owner,process,client,child:Mutex::new(Some(child)),requests:Mutex::new(BTreeMap::new()),next_request:AtomicU64::new(1),dispatch:Mutex::new(()),rebinding:AtomicBool::new(false)})),Err(failure)=>{if child.try_wait()?.is_none(){child.kill()?;}child.wait()?;Err(failure)}}
 }
 fn remap(&self,mut frame:Frame)->io::Result<Frame>{
  if self.rebinding.load(Ordering::Acquire){return Err(error(libc::EAGAIN));}
  let mut requests=self.requests.lock().unwrap();
  if requests.contains_key(&frame.request){return Err(error(libc::EALREADY));}
  let native=self.next_request.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|next|next.checked_add(1)).map_err(|_|error(libc::EOVERFLOW))?;
  if frame.operation==Operation::Cancel {
   let (operation,waiting)=requests.get(&frame.ticket).ok_or_else(||error(libc::ENOENT))?;
   if *operation!=Operation::Wait{return Err(error(libc::EINVAL));}frame.ticket=*waiting;
  }
  requests.insert(frame.request,(frame.operation,native));frame.request=native;frame.host_pid=0;frame.guest_pid=0;Ok(frame)
 }
 fn finished(&self,original:u64,native:u64){let mut requests=self.requests.lock().unwrap();if requests.get(&original).is_some_and(|(_,request)|*request==native){requests.remove(&original);}}
 fn rebind(&self)->io::Result<()> {
  let waits={let _dispatch=self.dispatch.lock().unwrap();
   if self.rebinding.swap(true,Ordering::AcqRel){return Err(error(libc::EBUSY));}
   self.requests.lock().unwrap().values().filter(|(operation,_)|*operation==Operation::Wait).map(|(_,request)|*request).collect::<Vec<_>>()};
  let result=(||{
   for waiting in waits {let native=self.next_request.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|next|next.checked_add(1)).map_err(|_|error(libc::EOVERFLOW))?;let mut cancel=Frame::new(Operation::Cancel,native);cancel.ticket=waiting;let response=self.client.call_right(cancel,None)?;if response.errno!=0{return Err(error(response.errno));}}
   self.requests.lock().unwrap().clear();Ok(())
  })();
  self.rebinding.store(false,Ordering::Release);result
 }
 fn stop(&self)->io::Result<()> {
  let mut child=self.child.lock().unwrap();let Some(mut owned)=child.take()else{return Ok(())};
  let result=(||{
   if let Some(status)=owned.try_wait()?{return if status.success(){Ok(())}else{Err(error(libc::ECHILD))};}
   let reply=self.client.call_right(Frame::new(Operation::Exit,u64::MAX),None)?;
   if reply.errno!=0{return Err(error(reply.errno));}
   let deadline=Instant::now()+Duration::from_secs(2);
   loop{if let Some(status)=owned.try_wait()?{return if status.success(){Ok(())}else{Err(error(libc::ECHILD))};}if Instant::now()>=deadline{return Err(error(libc::ETIMEDOUT));}thread::sleep(Duration::from_millis(5));}
  })();
  if result.is_err()&&owned.try_wait()?.is_none(){owned.kill()?;owned.wait()?;}
  result
 }
}
impl Drop for Holder {fn drop(&mut self){if let Err(failure)=self.stop(){eprintln!("POSIX holder cleanup: {failure}");}}}
struct Forwarded {frame:Frame,native_request:u64,actor:ProcessIdentity,reply:crate::posix_control::Reply,holder:Arc<Holder>,pending:crate::posix_control::Pending}
#[derive(Clone)]
struct Namespace {table:PathBuf,init:crate::process_namespace::InitRegistration}
struct State {config:Config,controller:ProcessIdentity,namespace:Mutex<Option<Namespace>>,owners:Mutex<BTreeMap<Key,Arc<Holder>>>,failures:Mutex<Vec<String>>}
impl State {
 fn record(&self,failure:io::Error){self.failures.lock().unwrap().push(failure.to_string());}
 fn retire_dead(&self)->io::Result<()> {
  let dead={let mut owners=self.owners.lock().unwrap();let keys=owners.iter().filter(|(_,holder)|!holder.owner.process.is_live()).map(|(key,_)|*key).collect::<Vec<_>>();keys.into_iter().filter_map(|key|owners.remove(&key)).collect::<Vec<_>>()};
  for holder in dead{holder.stop()?;}
  if let Some(namespace)=self.namespace.lock().unwrap().clone(){crate::process_namespace::InitNamespaceEntry::retire_dead(&namespace.table,self.controller)?;}
  Ok(())
 }
 fn retire_all(&self)->io::Result<()> {
  let owners=std::mem::take(&mut *self.owners.lock().unwrap());let mut failure=None;
  for holder in owners.into_values(){if let Err(error)=holder.stop(){failure=Some(error);}}
  match failure{Some(error)=>Err(error),None=>Ok(())}
 }
 fn admit_fork(&self,actor:ProcessIdentity)->io::Result<Arc<Holder>> {
  let namespace=self.namespace.lock().unwrap().clone().ok_or_else(||error(libc::EPERM))?;
  let mut info:libc::proc_bsdinfo=unsafe{std::mem::zeroed()};let size=std::mem::size_of_val(&info)as i32;
  if unsafe{libc::proc_pidinfo(actor.host_pid,libc::PROC_PIDTBSDINFO,1,(&mut info as *mut libc::proc_bsdinfo).cast(),size)}!=size{return Err(error(libc::ESRCH));}
  let parent=self.owners.lock().unwrap().values().find(|holder|holder.owner.process.host_pid==info.pbi_ppid as i32&&holder.owner.process.is_live()).cloned().ok_or_else(||error(libc::EPERM))?;
  let init=crate::process_namespace::InitRegistration::read(&namespace.table)?;
  let parent_namespace=crate::process_namespace::mount_namespace_of(&namespace.table,parent.owner.process)?;
  let child_namespace=crate::process_namespace::mount_namespace_of(&namespace.table,actor).map_err(|failure|if failure.kind()==io::ErrorKind::NotFound{error(libc::EPERM)}else{failure})?;
  if init.process!=namespace.init.process||init.mount_namespace!=namespace.init.mount_namespace
   ||child_namespace!=parent_namespace{return Err(error(libc::EPERM));}
  if !actor.is_live()||!parent.owner.process.is_live(){return Err(error(libc::ESRCH));}
  let owner=Owner{process:actor,guest_pid:if actor==namespace.init.process{1}else{actor.host_pid}};
  let holder=Holder::spawn(&self.config,owner,self.controller)?;
  if !actor.is_live()||!parent.owner.process.is_live(){holder.stop()?;return Err(error(libc::ESRCH));}
  if crate::process_namespace::mount_namespace_of(&namespace.table,actor)?!=child_namespace
   ||crate::process_namespace::mount_namespace_of(&namespace.table,parent.owner.process)?!=parent_namespace{holder.stop()?;return Err(error(libc::EPERM));}
  let mut owners=self.owners.lock().unwrap();
  if let Some(existing)=owners.get(&key(actor)).cloned(){drop(owners);holder.stop()?;return Ok(existing);}
  owners.insert(key(actor),holder.clone());Ok(holder)
 }
 fn external_namespace(&self,actor:ProcessIdentity)->io::Result<String> {
  let namespace=self.namespace.lock().unwrap().clone().ok_or_else(||error(libc::EPERM))?;
  let init=crate::process_namespace::InitRegistration::read(&namespace.table)?;
  if init.process!=self.controller||init.process!=namespace.init.process||init.mount_namespace!=namespace.init.mount_namespace{return Err(error(libc::EPERM));}
  let entry=crate::process_namespace::InitNamespaceEntry::read(&namespace.table,actor,init.process)?;
  let current=entry.namespace;
  if crate::process_namespace::mount_namespace_of(&namespace.table,actor)?!=current{return Err(error(libc::EPERM));}
  crate::mount_namespace::Namespace::open(namespace.table.parent().and_then(std::path::Path::parent).ok_or_else(||error(libc::EPROTO))?,&current)?.read()?;
  // Namespace entry is a privileged operation; the native owner reads the
  // retained guest identity, never UID or guest PID claims in a Mach frame.
  let credentials=read_credentials(&namespace.table,actor)?;
  if credentials.effective_capabilities&((1<<21)|(1<<18))!=((1<<21)|(1<<18)){return Err(error(libc::EPERM));}
  if !actor.is_live(){return Err(error(libc::ESRCH));}Ok(current)
 }
 fn admit_external(&self,actor:ProcessIdentity)->io::Result<Arc<Holder>> {
  let namespace=self.external_namespace(actor)?;
  let init=self.namespace.lock().unwrap().clone().ok_or_else(||error(libc::EPERM))?.init;
  let guest_pid=if actor==init.process{1}else{actor.host_pid};
  let holder=Holder::spawn(&self.config,Owner{process:actor,guest_pid},self.controller)?;
  let validation=self.external_namespace(actor);
  if !matches!(validation,Ok(ref current)if *current==namespace){holder.stop()?;return Err(validation.err().unwrap_or_else(||error(libc::EPERM)));}
  let mut owners=self.owners.lock().unwrap();
  if let Some(existing)=owners.get(&key(actor)).cloned(){drop(owners);holder.stop()?;return Ok(existing);}
  // This native-created Holder is the revocable process capability. Its key
  // includes the audited actor birth; retire_dead destroys its final lock owner.
  owners.insert(key(actor),holder.clone());Ok(holder)
 }
 fn begin(&self,request:Request)->io::Result<Option<Forwarded>> {
  self.retire_dead()?;
  let Request{frame,proof,actor,reply}=request;
  let holder=self.owners.lock().unwrap().get(&key(actor)).cloned();
  let holder=if holder.is_none()&&matches!(frame.operation,Operation::Attach|Operation::ExternalAttach){match if frame.operation==Operation::ExternalAttach{self.admit_external(actor)}else{self.admit_fork(actor)}{Ok(holder)=>Some(holder),Err(failure)=>{let mut response=frame;response.host_pid=0;response.guest_pid=0;response.errno=failure.raw_os_error().unwrap_or(libc::EPERM);reply.send(response)?;return Ok(None)}}}else{holder};
  if matches!(frame.operation,Operation::Attach|Operation::ExternalAttach) {
   let mut response=frame;response.host_pid=0;response.guest_pid=0;
   response.errno=match holder.as_ref(){Some(holder)if holder.owner.process==actor&&actor.is_live()&&holder.process.is_live()=>{match holder.rebind(){Ok(())=>{response.guest_pid=holder.owner.guest_pid;0},Err(failure)=>failure.raw_os_error().unwrap_or(libc::EIO)}},_=>libc::EPERM};
   reply.send(response)?;return Ok(None);
  }
  let prepared=(||{
   let holder=holder.ok_or_else(||error(libc::EPERM))?;
   if !actor.is_live()||holder.owner.process!=actor||!holder.process.is_live(){return Err(error(libc::ESRCH));}
   if frame.operation==Operation::Exit{return Err(error(libc::EPERM));}
   let client=holder.client.try_clone()?;
   // Forward in endpoint receive order, before starting a reply waiter.
   // Cancel must not overtake the corresponding Wait reservation.
   let dispatch=holder.dispatch.lock().unwrap();
   let native=holder.remap(frame)?;
   let pending=match client.begin_right(native,proof.as_ref()){Ok(value)=>value,Err(failure)=>{holder.finished(frame.request,native.request);return Err(failure)}};
   drop(dispatch);
   Ok((holder,pending,native.request))
  })();
  match prepared {
   Ok((holder,pending,native_request))=>Ok(Some(Forwarded{frame,native_request,actor,reply,holder,pending})),
   Err(failure)=>{let mut response=frame;response.errno=failure.raw_os_error().unwrap_or(libc::EIO);reply.send(response)?;Ok(None)}
  }
 }
 fn handle(&self,forwarded:Forwarded)->io::Result<()> {
  let Forwarded{frame,native_request,actor,reply,holder,pending}=forwarded;let mut response=frame;
  let result=(||{
   response=loop{match pending.wait(Duration::from_millis(100)){Ok(value)=>break value,Err(failure)if matches!(failure.raw_os_error(),Some(libc::ETIMEDOUT)|Some(libc::EINTR))=>{if !actor.is_live()||!holder.process.is_live(){return Err(error(libc::ESRCH));}},Err(failure)=>return Err(failure)}};
   if frame.operation==Operation::Get&&response.errno==0&&response.range.kind!=2 {
    // The controller's exact live holder registry is authoritative. Host
    // processes outside this guest namespace have no guest PID.
    let owners=self.owners.lock().unwrap();
    response.guest_pid=owners.values().find(|entry|entry.process.host_pid==response.host_pid&&entry.process.is_live()&&entry.owner.process.is_live()).map_or(0,|entry|entry.owner.guest_pid);
    response.host_pid=0;
   }
   Ok(())
  })();
  if let Err(failure)=result{response.errno=failure.raw_os_error().unwrap_or(libc::EIO);}
  holder.finished(frame.request,native_request);response.request=frame.request;
  reply.send(response)
 }
}
pub struct Controller {config:Config,process:ProcessIdentity,state:Arc<State>,stop:Arc<AtomicBool>,thread:Option<thread::JoinHandle<io::Result<()>>>}
impl Controller {
 pub fn start(config:Config)->io::Result<Self>{
  if config.startup_timeout.is_zero()||!config.holder.is_absolute()||!config.holder.is_file(){return Err(error(libc::EINVAL));}
  let endpoint=Endpoint::register(&config.endpoint)?;let process=ProcessIdentity::running(std::process::id()as i32)?;
  let state=Arc::new(State{config:config.clone(),controller:process,namespace:Mutex::new(None),owners:Mutex::new(BTreeMap::new()),failures:Mutex::new(Vec::new())});let stop=Arc::new(AtomicBool::new(false));
  let serving=state.clone();let stopped=stop.clone();
  let thread=thread::Builder::new().name("posix-controller".into()).spawn(move||{
   let mut requests:Vec<thread::JoinHandle<io::Result<()>>>=Vec::new();
   let result=(||{
    while !stopped.load(Ordering::Acquire){
     serving.retire_dead()?;
     for index in (0..requests.len()).rev(){if requests[index].is_finished(){match requests.swap_remove(index).join(){Ok(Ok(()))=>{},Ok(Err(failure))=>serving.record(failure),Err(_)=>return Err(error(libc::EIO))}}}
     match endpoint.receive(Duration::from_millis(50)){
      Ok(request)=>{if requests.len()>=256{let mut response=request.frame;response.errno=libc::EAGAIN;request.reply.send(response)?;}else{match serving.begin(request){Ok(Some(forwarded))=>{let state=serving.clone();requests.push(thread::spawn(move||state.handle(forwarded)));},Ok(None)=>{},Err(failure)=>serving.record(failure)}}},
      Err(failure)if matches!(failure.raw_os_error(),Some(libc::ETIMEDOUT)|Some(libc::EINTR))=>{},Err(failure)if matches!(failure.raw_os_error(),Some(libc::ESRCH)|Some(libc::EPERM)|Some(libc::EPROTO))=>serving.record(failure),Err(failure)=>return Err(failure)
     }
    }Ok(())
   })();
   let cleanup=serving.retire_all();
   for request in requests{match request.join(){Ok(Ok(()))=>{},Ok(Err(failure))=>serving.record(failure),Err(_)=>serving.record(error(libc::EIO))}}
   result.and(cleanup)
  })?;
  Ok(Self{config,process,state,stop,thread:Some(thread)})
 }
 /// Bind the native namespace receipt directory, already verified by boot
 /// to be outside all guest-visible mounts.
 pub fn bind_namespace(&self,table:&std::path::Path,init:&crate::process_namespace::InitRegistration)->io::Result<()> {
  let table=std::fs::canonicalize(table)?;let current=crate::process_namespace::InitRegistration::read(&table)?;
  if current.process!=init.process||current.mount_namespace!=init.mount_namespace||init.process!=self.process{return Err(error(libc::EPERM));}
  let mut namespace=self.state.namespace.lock().unwrap();
  if let Some(prior)=namespace.as_ref(){if prior.table==table&&prior.init.process==init.process&&prior.init.mount_namespace==init.mount_namespace{return Ok(());}return Err(error(libc::EBUSY));}
  *namespace=Some(Namespace{table,init:init.clone()});Ok(())
 }
 /// Native admission passes the actual launched child/namespace receipt.
 /// No Mach frame, Darwin UID, or guest-supplied PID can create this entry.
 pub fn register_guest(&self,owner:Owner)->io::Result<ProcessIdentity>{
  if self.stop.load(Ordering::Acquire)||!self.process.is_live()||!owner.process.is_live()||owner.guest_pid<=0{return Err(error(libc::ESRCH));}
  self.state.retire_dead()?;
  let mut owners=self.state.owners.lock().unwrap();
  if let Some(holder)=owners.get(&key(owner.process)){if holder.owner!=owner{return Err(error(libc::EINVAL));}if !holder.process.is_live(){return Err(error(libc::ECHILD));}return Ok(holder.process);}
  if owners.values().any(|entry|entry.owner.guest_pid==owner.guest_pid&&entry.owner.process.is_live()){return Err(error(libc::EEXIST));}
  let holder=Holder::spawn(&self.config,owner,self.process)?;let process=holder.process;owners.insert(key(owner.process),holder);Ok(process)
 }
 /// Native exec handoff retires old blocking requests, while keeping the
 /// same process's file-backed locks and cached holder descriptors.
 pub fn rebind_guest(&self,owner:Owner)->io::Result<()> {
  let holder=self.state.owners.lock().unwrap().get(&key(owner.process)).cloned().ok_or_else(||error(libc::ESRCH))?;
  if holder.owner!=owner||!owner.process.is_live(){return Err(error(libc::ESRCH));}holder.rebind()
 }
 /// Native wait/reap retains the birth receipt even after the PID is gone.
 pub fn guest_exited(&self,owner:Owner)->io::Result<()> {
  if owner.process.is_live(){return Err(error(libc::EBUSY));}
  let holder={let mut owners=self.state.owners.lock().unwrap();
   if let Some(holder)=owners.get(&key(owner.process)){if holder.owner!=owner{return Err(error(libc::EPERM));}}
   owners.remove(&key(owner.process))};
  if let Some(holder)=holder{holder.stop()?;}
  if let Some(namespace)=self.state.namespace.lock().unwrap().clone(){crate::process_namespace::InitNamespaceEntry::retire_dead(&namespace.table,self.process)?;}
  Ok(())
 }
 pub fn owner_config(&self)->io::Result<OwnerConfig>{if self.stop.load(Ordering::Acquire)||!self.process.is_live(){return Err(error(libc::ESRCH));}Ok(OwnerConfig{endpoint:self.config.endpoint.clone(),process:self.process})}
 pub fn failures(&self)->Vec<String>{self.state.failures.lock().unwrap().clone()}
 pub fn shutdown(mut self)->io::Result<()>{self.join()}
 fn join(&mut self)->io::Result<()>{self.stop.store(true,Ordering::Release);match self.thread.take(){Some(thread)=>thread.join().map_err(|_|error(libc::EIO))?,None=>Ok(())}}
}
impl Drop for Controller {fn drop(&mut self){if let Err(failure)=self.join(){eprintln!("POSIX controller shutdown: {failure}");}}}
