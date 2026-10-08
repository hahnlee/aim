//! Native controller for process-owned POSIX locks (#1189).
//! Only native process admission registers owners; Mach request payloads cannot.
use crate::{posix_control::{Client,Endpoint,Frame,Operation,Owner,Request},process_namespace::ProcessIdentity};
use std::{collections::BTreeMap,io,path::PathBuf,process::{Child,Command,Stdio},sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}},thread,time::{Duration,Instant}};
fn error(code:i32)->io::Error{io::Error::from_raw_os_error(code)}
type Key=(i32,u64,u64);
fn key(p:ProcessIdentity)->Key{(p.host_pid,p.start_seconds,p.start_microseconds)}
static NEXT:AtomicU64=AtomicU64::new(1);
pub struct Config {pub endpoint:String,pub holder:PathBuf,pub startup_timeout:Duration}
#[derive(Clone,Debug)]
pub struct OwnerConfig {pub endpoint:String,pub process:ProcessIdentity}
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
struct State {owners:Mutex<BTreeMap<Key,Arc<Holder>>>,failures:Mutex<Vec<String>>}
impl State {
 fn record(&self,failure:io::Error){self.failures.lock().unwrap().push(failure.to_string());}
 fn retire_dead(&self)->io::Result<()> {
  let dead={let mut owners=self.owners.lock().unwrap();let keys=owners.iter().filter(|(_,holder)|!holder.owner.process.is_live()).map(|(key,_)|*key).collect::<Vec<_>>();keys.into_iter().filter_map(|key|owners.remove(&key)).collect::<Vec<_>>()};
  for holder in dead{holder.stop()?;}Ok(())
 }
 fn retire_all(&self)->io::Result<()> {
  let owners=std::mem::take(&mut *self.owners.lock().unwrap());let mut failure=None;
  for holder in owners.into_values(){if let Err(error)=holder.stop(){failure=Some(error);}}
  match failure{Some(error)=>Err(error),None=>Ok(())}
 }
 fn begin(&self,request:Request)->io::Result<Option<Forwarded>> {
  self.retire_dead()?;
  let Request{frame,proof,actor,reply}=request;
  let holder=self.owners.lock().unwrap().get(&key(actor)).cloned();
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
  let state=Arc::new(State{owners:Mutex::new(BTreeMap::new()),failures:Mutex::new(Vec::new())});let stop=Arc::new(AtomicBool::new(false));
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
 pub fn owner_config(&self)->io::Result<OwnerConfig>{if self.stop.load(Ordering::Acquire)||!self.process.is_live(){return Err(error(libc::ESRCH));}Ok(OwnerConfig{endpoint:self.config.endpoint.clone(),process:self.process})}
 pub fn failures(&self)->Vec<String>{self.state.failures.lock().unwrap().clone()}
 pub fn shutdown(mut self)->io::Result<()>{self.join()}
 fn join(&mut self)->io::Result<()>{self.stop.store(true,Ordering::Release);match self.thread.take(){Some(thread)=>thread.join().map_err(|_|error(libc::EIO))?,None=>Ok(())}}
}
impl Drop for Controller {fn drop(&mut self){if let Err(failure)=self.join(){eprintln!("POSIX controller shutdown: {failure}");}}}
