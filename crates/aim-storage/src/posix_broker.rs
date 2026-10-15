//! Native controller for process-owned POSIX locks (#1189).
//! Only native process admission registers owners; Mach request payloads cannot.
use crate::{posix_control::{Client,Endpoint,Frame,Operation,Owner,Request},process_namespace::ProcessIdentity};
use std::{collections::BTreeMap,io,path::PathBuf,process::{Child,Command},sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}},thread,time::{Duration,Instant}};
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
fn admission_diagnostic(arguments:std::fmt::Arguments<'_>) {
 use std::io::Write;
 // Diagnostics must not replace the original owner error if stderr is closed.
 let _=writeln!(io::stderr().lock(),"{arguments}");
}
fn admission_failure(stage:&str,actor:ProcessIdentity,failure:io::Error)->io::Error {
 admission_diagnostic(format_args!("[posix-owner] stage={stage} controller_pid={} actor={actor:?} host_errno={:?} error={failure}",std::process::id(),failure.raw_os_error()));
 failure
}
fn spawn_failure(config:&Config,owner:Owner,controller:ProcessIdentity,failure:io::Error)->io::Error {
 // Snapshot only: closed stdio is legal and these numbers are not ownership proofs.
 // The holder installs null stdin after exec and inherits stdout/stderr. Never reopen or retry here.
 for fd in 0..3 {
  let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};
  let flags_errno=if flags<0{io::Error::last_os_error().raw_os_error()}else{None};
  let access=unsafe{libc::fcntl(fd,libc::F_GETFL)};
  let access_errno=if access<0{io::Error::last_os_error().raw_os_error()}else{None};
  admission_diagnostic(format_args!("[posix-owner] stage=holder-command-spawn-stdio controller={controller:?} actor={:?} fd={fd} getfd={flags} getfd_errno={flags_errno:?} getfl={access} getfl_errno={access_errno:?} stdin=inherit stdout=inherit stderr=inherit",owner.process));
 }
 admission_diagnostic(format_args!("[posix-owner] stage=holder-command-spawn-context controller={controller:?} actor={:?} executable={}",owner.process,config.holder.display()));
 admission_failure("holder-command-spawn",owner.process,failure)
}

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
/// A guest process's lock owner. Its holder process starts with the first
/// request that needs it: most processes never take a POSIX lock, and on
/// Linux a process that holds none costs the kernel nothing.
struct Holder {owner:Owner,config:Config,controller:ProcessIdentity,native:Mutex<Option<Arc<Native>>>,registered:AtomicBool,requests:Mutex<BTreeMap<u64,(Operation,u64)>>,next_request:AtomicU64,dispatch:Mutex<()>,rebinding:AtomicBool}
struct Native {process:ProcessIdentity,client:Client,child:Mutex<Option<Child>>}
impl Native {
 fn spawn(config:&Config,owner:Owner,controller:ProcessIdentity)->io::Result<Self>{
  let name=format!("com.aim.posix-holder.{}.{}.{}",controller.host_pid,controller.start_microseconds,NEXT.fetch_add(1,Ordering::Relaxed));
  let mut child=Command::new(&config.holder).args(["--service-name",&name,"--controller-pid",&controller.host_pid.to_string(),"--controller-seconds",&controller.start_seconds.to_string(),"--controller-microseconds",&controller.start_microseconds.to_string()]).spawn().map_err(|failure|spawn_failure(config,owner,controller,failure))?;
  let result=(||{
   let process=ProcessIdentity::running(child.id()as i32).map_err(|failure|admission_failure("holder-running-pid",owner.process,failure))?;let deadline=Instant::now()+config.startup_timeout;
   let client=loop{match Client::lookup(&name){Ok(client)=>break client,Err(failure)if failure.raw_os_error()==Some(libc::ENOENT)=>{if child.try_wait().map_err(|failure|admission_failure("holder-try-wait",owner.process,failure))?.is_some(){return Err(admission_failure("holder-exited-before-lookup",owner.process,error(libc::ECHILD)));}if Instant::now()>=deadline{return Err(admission_failure("holder-lookup-timeout",owner.process,error(libc::ETIMEDOUT)));}thread::sleep(Duration::from_millis(5));},Err(failure)=>return Err(admission_failure("holder-client-lookup",owner.process,failure))}};
   if !owner.process.is_live(){return Err(admission_failure("holder-actor-live",owner.process,error(libc::ESRCH)));}Ok((process,client))
  })();
  match result{Ok((process,client))=>Ok(Self{process,client,child:Mutex::new(Some(child))}),Err(failure)=>{if child.try_wait().map_err(|failure|admission_failure("holder-try-wait",owner.process,failure))?.is_none(){child.kill().map_err(|failure|admission_failure("holder-failed-start-cleanup-kill",owner.process,failure))?;}child.wait().map_err(|failure|admission_failure("holder-failed-start-cleanup-wait",owner.process,failure))?;Err(failure)}}
 }
}
impl Holder {
 fn new(config:&Config,owner:Owner,controller:ProcessIdentity)->Arc<Self>{
  Arc::new(Self{owner,config:config.clone(),controller,native:Mutex::new(None),registered:AtomicBool::new(false),requests:Mutex::new(BTreeMap::new()),next_request:AtomicU64::new(1),dispatch:Mutex::new(()),rebinding:AtomicBool::new(false)})
 }
 fn spawn(config:&Config,owner:Owner,controller:ProcessIdentity)->io::Result<Arc<Self>>{let holder=Self::new(config,owner,controller);holder.native()?;Ok(holder)}
 /// The holder process, started now if it has not been.
 fn native(&self)->io::Result<Arc<Native>>{
  let mut native=self.native.lock().unwrap();
  if let Some(native)=native.as_ref(){return Ok(native.clone());}
  let started=Arc::new(Native::spawn(&self.config,self.owner,self.controller)?);*native=Some(started.clone());Ok(started)
 }
 fn started(&self)->Option<Arc<Native>>{self.native.lock().unwrap().clone()}
 /// Live unless its holder process has died.
 fn alive(&self)->bool{self.started().is_none_or(|native|native.process.is_live())}
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
   if self.rebinding.swap(true,Ordering::AcqRel){return Err(admission_failure("rebind-busy",self.owner.process,error(libc::EBUSY)));}
   self.requests.lock().unwrap().values().filter(|(operation,_)|*operation==Operation::Wait).map(|(_,request)|*request).collect::<Vec<_>>()};
  let result=(||{
   let holder=match self.started(){Some(holder)=>holder,None=>{self.requests.lock().unwrap().clear();return Ok(());}};
   for waiting in waits {let native=self.next_request.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|next|next.checked_add(1)).map_err(|_|admission_failure("rebind-request-overflow",self.owner.process,error(libc::EOVERFLOW)))?;let mut cancel=Frame::new(Operation::Cancel,native);cancel.ticket=waiting;let response=holder.client.call_right(cancel,None).map_err(|failure|admission_failure("rebind-cancel-call",self.owner.process,failure))?;if response.errno!=0{return Err(admission_failure("rebind-cancel-reply",self.owner.process,error(response.errno)));}}
   self.requests.lock().unwrap().clear();Ok(())
  })();
  self.rebinding.store(false,Ordering::Release);result
 }
 fn stop(&self)->io::Result<()> {
  let Some(native)=self.started()else{return Ok(())};
  let mut child=native.child.lock().unwrap();let Some(mut owned)=child.take()else{return Ok(())};
  let result=(||{
   if let Some(status)=owned.try_wait()?{return if status.success(){Ok(())}else{Err(error(libc::ECHILD))};}
   let reply=native.client.call_right(Frame::new(Operation::Exit,u64::MAX),None)?;
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
  for holder in dead{if let Err(failure)=holder.stop(){eprintln!("[posix-owner] stage=retire-dead controller={:?} actor={:?} error={failure}",self.controller,holder.owner.process);return Err(failure);}}
  if let Some(namespace)=self.namespace.lock().unwrap().clone(){crate::process_namespace::InitNamespaceEntry::retire_dead(&namespace.table,self.controller)?;}
  Ok(())
 }
 fn retire_all(&self)->io::Result<()> {
  let owners=std::mem::take(&mut *self.owners.lock().unwrap());let mut failure=None;
  for holder in owners.into_values(){if let Err(error)=holder.stop(){failure=Some(error);}}
  match failure{Some(error)=>Err(error),None=>Ok(())}
 }
 fn admit_fork(&self,actor:ProcessIdentity)->io::Result<Arc<Holder>> {
  let failed=|stage:&str,failure:io::Error|admission_failure(stage,actor,failure);
  let namespace=self.namespace.lock().unwrap().clone().ok_or_else(||failed("fork-bound-namespace",error(libc::EPERM)))?;
  let mut info:libc::proc_bsdinfo=unsafe{std::mem::zeroed()};let size=std::mem::size_of_val(&info)as i32;
  if unsafe{libc::proc_pidinfo(actor.host_pid,libc::PROC_PIDTBSDINFO,1,(&mut info as *mut libc::proc_bsdinfo).cast(),size)}!=size{return Err(failed("fork-actor-metadata",error(libc::ESRCH)));}
  let parent=self.owners.lock().unwrap().values().find(|holder|holder.owner.process.host_pid==info.pbi_ppid as i32&&holder.owner.process.is_live()).cloned().ok_or_else(||{admission_diagnostic(format_args!("[posix-owner] stage=fork-parent-lookup controller={:?} actor={actor:?} parent_host_pid={}",self.controller,info.pbi_ppid));failed("fork-parent-lookup",error(libc::EPERM))})?;
  let init=crate::process_namespace::InitRegistration::read(&namespace.table).map_err(|failure|failed("fork-init-record",failure))?;
  let parent_namespace=crate::process_namespace::mount_namespace_of(&namespace.table,parent.owner.process).map_err(|failure|failed("fork-parent-namespace",failure))?;
  let child_namespace=crate::process_namespace::mount_namespace_of(&namespace.table,actor).map_err(|failure|failed("fork-child-namespace",if failure.kind()==io::ErrorKind::NotFound{error(libc::EPERM)}else{failure}))?;
  if init.process!=namespace.init.process||init.mount_namespace!=namespace.init.mount_namespace
   ||child_namespace!=parent_namespace{return Err(failed("fork-namespace-match",error(libc::EPERM)));}
  if !actor.is_live()||!parent.owner.process.is_live(){return Err(failed("fork-parent-child-live",error(libc::ESRCH)));}
  let owner=Owner{process:actor,guest_pid:if actor==namespace.init.process{1}else{actor.host_pid}};
  let holder=Holder::new(&self.config,owner,self.controller);
  if !actor.is_live()||!parent.owner.process.is_live(){holder.stop().map_err(|failure|failed("fork-holder-stop",failure))?;return Err(failed("fork-post-spawn-live",error(libc::ESRCH)));}
  if crate::process_namespace::mount_namespace_of(&namespace.table,actor).map_err(|failure|failed("fork-post-child-namespace",failure))?!=child_namespace
   ||crate::process_namespace::mount_namespace_of(&namespace.table,parent.owner.process).map_err(|failure|failed("fork-post-parent-namespace",failure))?!=parent_namespace{holder.stop().map_err(|failure|failed("fork-holder-stop",failure))?;return Err(failed("fork-post-namespace-match",error(libc::EPERM)));}
  let mut owners=self.owners.lock().unwrap();
  if let Some(existing)=owners.get(&key(actor)).cloned(){drop(owners);holder.stop().map_err(|failure|failed("fork-duplicate-holder-stop",failure))?;return Ok(existing);}
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
  let holder=Holder::new(&self.config,Owner{process:actor,guest_pid},self.controller);
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
   response.errno=match holder.as_ref(){Some(holder)if holder.owner.process==actor&&actor.is_live()&&holder.alive()=>{match holder.rebind(){Ok(())=>{response.guest_pid=holder.owner.guest_pid;response.ticket=u64::from(holder.registered.load(Ordering::Acquire));0},Err(failure)=>failure.raw_os_error().unwrap_or(libc::EIO)}},_=>libc::EPERM};
   reply.send(response)?;return Ok(None);
  }
  // Without a holder process nothing is registered, so a close releases
  // nothing.
  if frame.operation==Operation::Close&&holder.as_ref().is_some_and(|holder|holder.owner.process==actor&&holder.started().is_none()){reply.send(frame)?;return Ok(None);}
  let prepared=(||{
   let holder=holder.ok_or_else(||error(libc::EPERM))?;
   if !actor.is_live()||holder.owner.process!=actor||!holder.alive(){return Err(error(libc::ESRCH));}
   if frame.operation==Operation::Exit{return Err(error(libc::EPERM));}
   if holder.started().is_none()&&frame.operation==Operation::Cancel{return Err(error(libc::ENOENT));}
   let client=holder.native()?.client.try_clone()?;
   // An attach to this holder (an exec of the same process) may find files
   // registered before it; the attach reply says so.
   if frame.operation==Operation::Register{holder.registered.store(true,Ordering::Release);}
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
   response=loop{match pending.wait(Duration::from_millis(100)){Ok(value)=>break value,Err(failure)if matches!(failure.raw_os_error(),Some(libc::ETIMEDOUT)|Some(libc::EINTR))=>{if !actor.is_live()||!holder.alive(){return Err(error(libc::ESRCH));}},Err(failure)=>return Err(failure)}};
   if frame.operation==Operation::Get&&response.errno==0&&response.range.kind!=2 {
    // The controller's exact live holder registry is authoritative. Host
    // processes outside this guest namespace have no guest PID.
    let owners=self.owners.lock().unwrap();
    response.guest_pid=owners.values().find(|entry|entry.started().is_some_and(|native|native.process.host_pid==response.host_pid&&native.process.is_live())&&entry.owner.process.is_live()).map_or(0,|entry|entry.owner.guest_pid);
    response.host_pid=0;
   }
   Ok(())
  })();
  if let Err(failure)=result{response.errno=failure.raw_os_error().unwrap_or(libc::EIO);}
  holder.finished(frame.request,native_request);response.request=frame.request;
  reply.send(response)
 }
}
pub struct Controller {config:Config,process:ProcessIdentity,state:Arc<State>,stop:Arc<AtomicBool>,thread:Mutex<Option<thread::JoinHandle<io::Result<()>>>>}
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
   if let Err(failure)=&result{eprintln!("[posix-owner] stage=controller-terminal controller={:?} error={failure}",serving.controller);}
   let cleanup=serving.retire_all();
   for request in requests{match request.join(){Ok(Ok(()))=>{},Ok(Err(failure))=>serving.record(failure),Err(_)=>serving.record(error(libc::EIO))}}
   result.and(cleanup)
  })?;
  Ok(Self{config,process,state,stop,thread:Mutex::new(Some(thread))})
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
  if let Some(holder)=owners.get(&key(owner.process)){if holder.owner!=owner{return Err(error(libc::EINVAL));}let native=holder.native()?;if !native.process.is_live(){return Err(error(libc::ECHILD));}return Ok(native.process);}
  if owners.values().any(|entry|entry.owner.guest_pid==owner.guest_pid&&entry.owner.process.is_live()){return Err(error(libc::EEXIST));}
  let holder=Holder::spawn(&self.config,owner,self.process)?;let process=holder.native()?.process;owners.insert(key(owner.process),holder);Ok(process)
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
 pub fn shutdown(&self)->io::Result<()>{self.join()}
 fn join(&self)->io::Result<()>{self.stop.store(true,Ordering::Release);let thread=self.thread.lock().map_err(|_|error(libc::EIO))?.take();match thread{Some(thread)=>thread.join().map_err(|_|error(libc::EIO))?,None=>Ok(())}}
}
impl Drop for Controller {fn drop(&mut self){if let Err(failure)=self.join(){eprintln!("POSIX controller shutdown: {failure}");}}}

#[cfg(test)]
mod diagnostics_tests {
 use super::*;
 use std::{ffi::CString,fs::File,os::fd::{AsRawFd,FromRawFd,OwnedFd}};
 struct Stderr {saved:OwnedFd,capture:File}
 impl Stderr {
  fn capture(path:&std::path::Path)->Self {
   let capture=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(path).unwrap();
   let saved=unsafe{libc::fcntl(2,libc::F_DUPFD_CLOEXEC,3)};assert!(saved>=0);
   assert_eq!(unsafe{libc::dup2(capture.as_raw_fd(),2)},2);
   Self{saved:unsafe{OwnedFd::from_raw_fd(saved)},capture}
  }
  fn text(&self)->String {
   use std::os::unix::fs::FileExt;
   let mut bytes=vec![0;self.capture.metadata().unwrap().len()as usize];self.capture.read_exact_at(&mut bytes,0).unwrap();String::from_utf8(bytes).unwrap()
  }
 }
 impl Drop for Stderr {fn drop(&mut self){assert_eq!(unsafe{libc::dup2(self.saved.as_raw_fd(),2)},2);}}
 struct Actions(libc::posix_spawn_file_actions_t);
 impl Drop for Actions {fn drop(&mut self){assert_eq!(unsafe{libc::posix_spawn_file_actions_destroy(&mut self.0)},0);}}
 #[test]
 fn native_spawn_file_action_failure_preserves_errno_and_stage() {
  const MARKER:&str="posix-native-spawn-diagnostics-fixture";
  if !std::env::args().any(|arg|arg==MARKER){
   let output=Command::new(std::env::current_exe().unwrap()).args(["--exact","posix_broker::diagnostics_tests::native_spawn_file_action_failure_preserves_errno_and_stage","--skip",MARKER,"--nocapture"]).output().unwrap();
   assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
   assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_SPAWN_DIAGNOSTICS_EXECUTED"));return;
  }
  let actual=ProcessIdentity::running(std::process::id()as i32).unwrap();
  let root=std::env::temp_dir().join(format!("aim-posix-spawn-diag-{}-{}",actual.host_pid,actual.start_microseconds));std::fs::create_dir(&root).unwrap();
  let capture=Stderr::capture(&root.join("stderr"));
  let file=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(root.join("source")).unwrap();
  let raw=unsafe{libc::fcntl(file.as_raw_fd(),libc::F_DUPFD_CLOEXEC,128)};assert!(raw>=128);
  let source=unsafe{OwnedFd::from_raw_fd(raw)};
  let mut actions=Actions(unsafe{std::mem::zeroed()});assert_eq!(unsafe{libc::posix_spawn_file_actions_init(&mut actions.0)},0);
  assert_eq!(unsafe{libc::posix_spawn_file_actions_adddup2(&mut actions.0,raw,2)},0);
  drop(source);
  assert_eq!(unsafe{libc::fcntl(raw,libc::F_GETFD)},-1);assert_eq!(io::Error::last_os_error().raw_os_error(),Some(libc::EBADF));
  let executable=CString::new("/usr/bin/true").unwrap();
  let arguments=[executable.as_ptr()as*mut libc::c_char,std::ptr::null_mut()];let environment=[std::ptr::null_mut()];let mut pid=0;
  let returned=unsafe{libc::posix_spawn(&mut pid,executable.as_ptr(),&actions.0,std::ptr::null(),arguments.as_ptr(),environment.as_ptr())};
  if returned==0{assert_eq!(unsafe{libc::waitpid(pid,std::ptr::null_mut(),0)},pid);}
  assert_eq!(returned,libc::EBADF,"actual invalid owned file action must fail before execution");
  drop(actions);
  let config=Config{endpoint:"unused-native-spawn-diagnostics".into(),holder:PathBuf::from("/usr/bin/true"),startup_timeout:Duration::from_secs(1)};
  let owner=Owner{process:actual,guest_pid:1};let failure=error(returned);let kind=failure.kind();let message=failure.to_string();
  let preserved=spawn_failure(&config,owner,actual,failure);
  assert_eq!(preserved.raw_os_error(),Some(libc::EBADF));assert_eq!(preserved.kind(),kind);assert_eq!(preserved.to_string(),message);
  let text=capture.text();let prefix=format!("[posix-owner] stage=holder-command-spawn controller_pid={} ",actual.host_pid);
  assert_eq!(text.lines().filter(|line|line.starts_with(&prefix)).count(),1,"{text}");
  assert!(text.contains("host_errno=Some(9)"),"{text}");assert!(text.contains(&format!("actor={actual:?}")),"{text}");
  for fd in 0..3{assert_eq!(text.lines().filter(|line|line.contains("stage=holder-command-spawn-stdio ")&&line.contains(&format!(" fd={fd} "))).count(),1,"{text}");}
  assert!(text.contains("stdin=inherit stdout=inherit stderr=inherit"),"{text}");
  // fd2 is our capture alias here, not an unowned inherited descriptor.
  assert_eq!(unsafe{libc::close(2)},0);
  let closed=std::panic::catch_unwind(||spawn_failure(&config,owner,actual,error(returned))).unwrap();
  assert_eq!(closed.raw_os_error(),Some(libc::EBADF));assert_eq!(closed.to_string(),message);
  drop(capture);drop(file);std::fs::remove_dir_all(root).unwrap();
  println!("NATIVE_SPAWN_DIAGNOSTICS_EXECUTED");
 }
}
