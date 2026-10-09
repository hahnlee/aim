//! One-descriptor SCM carrier. Kernel peer lifetime includes queued SCM_RIGHTS;
//! synchronous enable-time draining removes closed carriers before writer EX.
use std::{collections::HashMap,io,os::{fd::{AsRawFd,BorrowedFd,OwnedFd},unix::net::UnixStream},sync::{LazyLock,Mutex,MutexGuard}};
use crate::{mach,proxy_file::socket_identity,wire::{RegularMetadata,Writer}};
pub const CLASS:u32=4;
struct Entry { server:UnixStream,backing:OwnedFd,writer:Option<OwnedFd>,metadata:RegularMetadata }
static REGISTRY:LazyLock<Mutex<HashMap<(u64,u64),Entry>>>=LazyLock::new(Default::default);
/// Owned send rights stay opaque until the ABI allocates private descriptors.
pub struct FilePort(mach::Port);
impl FilePort { pub(crate) fn new(port:mach::Port)->Self{Self(port)} pub fn as_port(&self)->mach::Port{self.0} }
impl Drop for FilePort { fn drop(&mut self){mach::release_send(self.0);} }
/// Rights drop before the registry guard: no in-progress export can orphan a
/// temporary writer reference beyond drain's linearization point.
pub(crate) struct ReplyGuard { ports:Vec<FilePort>,data:Vec<u8>,_registry:MutexGuard<'static,HashMap<(u64,u64),Entry>> }
impl ReplyGuard {
 pub fn ports(&self)->Vec<(mach::Port,u32)>{self.ports.iter().map(|port|(port.as_port(),mach::COPY_SEND)).collect()}
 pub fn data(&self)->&[u8]{&self.data}
}
pub struct Resolved { pub backing:FilePort,pub writer:Option<crate::regular_file::WriterPort>,pub metadata:RegularMetadata }
fn error(value:i32)->io::Error{io::Error::from_raw_os_error(value)}
fn port(fd:i32)->io::Result<FilePort>{mach::fd_to_port(fd).map(FilePort::new).ok_or_else(||error(libc::EBADF))}
fn closed(entry:&Entry)->io::Result<bool>{
 let mut byte=0u8;
 let n=unsafe{libc::recv(entry.server.as_raw_fd(),(&mut byte as *mut u8).cast(),1,libc::MSG_PEEK|libc::MSG_DONTWAIT)};
 if n==0{return Ok(true);}if n>0{return Err(error(libc::EPROTO));}
 let failure=io::Error::last_os_error();if failure.kind()==io::ErrorKind::WouldBlock{return Ok(false);}Err(failure)
}
fn drain_locked(registry:&mut HashMap<(u64,u64),Entry>,identity:Option<&[u8;36]>)->io::Result<()> {
 let mut dead=Vec::new();
 for (key,entry) in registry.iter(){if identity.is_none_or(|identity|entry.metadata.identity==*identity)&&closed(entry)?{dead.push(*key);}}
 for key in dead {registry.remove(&key);}
 Ok(())
}
pub(crate) fn create(backing:OwnedFd,writer:Option<OwnedFd>,metadata:RegularMetadata)->io::Result<ReplyGuard>{
 use std::os::fd::AsFd;
 crate::regular_file::validate(backing.as_fd(),writer.as_ref().map(AsFd::as_fd),&metadata)?;
 let mut registry=REGISTRY.lock().unwrap();drain_locked(&mut registry,None)?;
 let (client,server)=UnixStream::pair()?;let identity=socket_identity(client.as_raw_fd())?;let peer=socket_identity(server.as_raw_fd())?;
 if identity.0==0||peer!=(identity.1,identity.0){return Err(error(libc::EPROTO));}
 let carrier=port(client.as_raw_fd())?;
 if registry.contains_key(&identity){return Err(error(libc::EPROTO));}
 registry.insert(identity,Entry{server,backing,writer,metadata});
 let mut data=Writer::default();data.u32(CLASS);
 Ok(ReplyGuard{ports:vec![carrier],data:data.0,_registry:registry})
}
pub(crate) fn resolve(carrier:BorrowedFd<'_>)->io::Result<ReplyGuard>{
 let identity=socket_identity(carrier.as_raw_fd())?;let registry=REGISTRY.lock().unwrap();
 let entry=registry.get(&identity).ok_or_else(||error(libc::EINVAL))?;
 if socket_identity(entry.server.as_raw_fd())?!=(identity.1,identity.0){return Err(error(libc::EPROTO));}
 let mut ports=vec![port(entry.backing.as_raw_fd())?];if let Some(writer)=&entry.writer{ports.push(port(writer.as_raw_fd())?);}
 let mut data=Writer::default();entry.metadata.encode(&mut data);
 Ok(ReplyGuard{ports,data:data.0,_registry:registry})
}
pub(crate) fn drain(identity:&[u8;36])->io::Result<()> {drain_locked(&mut REGISTRY.lock().unwrap(),Some(identity))}
pub(crate) fn registered_class(fd:i32)->io::Result<u32>{
 let identity=match socket_identity(fd){Ok(identity)=>identity,Err(failure)=>{
  let mut kind=0i32;let mut length=4;
  if unsafe{libc::getsockopt(fd,libc::SOL_SOCKET,libc::SO_TYPE,(&mut kind as *mut i32).cast(),&mut length)}<0&&io::Error::last_os_error().raw_os_error()==Some(libc::ENOTSOCK){return Ok(0);}return Err(failure);
 }};
 let registry=REGISTRY.lock().unwrap();
 match registry.get(&identity){Some(entry)if socket_identity(entry.server.as_raw_fd())?==(identity.1,identity.0)=>Ok(CLASS),_=>Ok(0)}
}

#[cfg(test)]
mod tests {
 use super::*;
 use std::{fs::{File,OpenOptions},os::fd::{AsFd,FromRawFd},sync::{mpsc,atomic::{AtomicU64,Ordering}}};
 struct Fixture {path:std::path::PathBuf,backing:File,writer:File,contender:File,metadata:RegularMetadata}
 impl Fixture {
  fn new()->Self{
   static NEXT:AtomicU64=AtomicU64::new(0);
   let path=std::env::temp_dir().join(format!("aim-scm-regular-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));std::fs::create_dir(&path).unwrap();
   let backing=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("backing")).unwrap();
   let writer=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("writers")).unwrap();
   let contender=OpenOptions::new().read(true).write(true).open(path.join("writers")).unwrap();
   assert_eq!(unsafe{libc::flock(writer.as_raw_fd(),libc::LOCK_SH)},0);
   let metadata=RegularMetadata{flags:2,uid:1000,gid:1001,identity:crate::regular_file::identity(backing.as_fd()).unwrap(),writer:true};
   Self{path,backing,writer,contender,metadata}
  }
  fn carrier(&self)->OwnedFd{
   let reply=create(self.backing.as_fd().try_clone_to_owned().unwrap(),Some(self.writer.as_fd().try_clone_to_owned().unwrap()),self.metadata.clone()).unwrap();
   let carrier=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(reply.ports[0].as_port()).unwrap())};drop(reply);carrier
  }
 }
 fn busy(fd:i32){assert_eq!(unsafe{libc::flock(fd,libc::LOCK_EX|libc::LOCK_NB)},-1);assert_eq!(io::Error::last_os_error().raw_os_error(),Some(libc::EWOULDBLOCK));}
 #[test]
 fn queued_scm_rights_keep_writer_and_one_guest_descriptor_until_actual_last_close(){
  const MARKER:&str="REGULAR_SCM_QUEUE_LIFETIME_FIXTURE_EXECUTED";
  if crate::socket_scm::tests::isolated_exec_fixture("regular_scm::tests::queued_scm_rights_keep_writer_and_one_guest_descriptor_until_actual_last_close",MARKER){return;}
  let fixture=Fixture::new();let carrier=fixture.carrier();
  let Fixture{path,backing,writer,contender,metadata}=fixture;drop(writer);drop(backing);
  let (sender,receiver)=UnixStream::pair().unwrap();
  crate::proxy_file::send(sender.as_raw_fd(),b"guest",carrier.as_raw_fd()).unwrap();drop(carrier);
  drain(&metadata.identity).unwrap();busy(contender.as_raw_fd());
  let mut data=[0;5];let (count,carrier)=crate::proxy_file::receive_flags(receiver.as_raw_fd(),&mut data,0).unwrap();assert_eq!(count,5);assert_eq!(&data,b"guest");
  let carrier=carrier.unwrap();assert_eq!(registered_class(carrier.as_raw_fd()).unwrap(),CLASS);
  let resolved=resolve(carrier.as_fd()).unwrap();let mut reader=crate::wire::Reader::new(resolved.data());assert_eq!(RegularMetadata::decode(&mut reader).unwrap(),metadata);assert_eq!(reader.remaining(),0);
  let actual_backing=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(resolved.ports[0].as_port()).unwrap())};
  let actual_writer=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(resolved.ports[1].as_port()).unwrap())};
  assert_eq!(crate::regular_file::identity(actual_backing.as_fd()).unwrap(),metadata.identity);
  drop(resolved);drop(carrier);drain(&metadata.identity).unwrap();busy(contender.as_raw_fd());
  drop(actual_backing);drop(actual_writer);assert_eq!(unsafe{libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
  std::fs::remove_dir_all(path).unwrap();println!("{MARKER}");
 }
 #[test]
 fn paused_export_serializes_receiver_death_and_temporary_writer_drop_before_drain(){
  const MARKER:&str="REGULAR_SCM_EXPORT_LIFETIME_FIXTURE_EXECUTED";
  if crate::socket_scm::tests::isolated_exec_fixture("regular_scm::tests::paused_export_serializes_receiver_death_and_temporary_writer_drop_before_drain",MARKER){return;}
  let fixture=Fixture::new();let carrier=fixture.carrier();let Fixture{path,backing,writer,contender,metadata}=fixture;drop(writer);drop(backing);
  let (ready,started)=mpsc::channel();let (finish,wait)=mpsc::channel();
  let exporter=std::thread::spawn(move||{let reply=resolve(carrier.as_fd()).unwrap();drop(carrier);ready.send(()).unwrap();wait.recv().unwrap();drop(reply);});
  started.recv().unwrap();assert!(REGISTRY.try_lock().is_err(),"reply exports retain the actual resource mutex");busy(contender.as_raw_fd());
  let identity=metadata.identity;let (done,completed)=mpsc::channel();
  let draining=std::thread::spawn(move||{drain(&identity).unwrap();done.send(()).unwrap();});
  finish.send(()).unwrap();completed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();exporter.join().unwrap();draining.join().unwrap();
  assert_eq!(unsafe{libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0,"no temporary writer refs remain after drain returns");std::fs::remove_dir_all(path).unwrap();println!("{MARKER}");
 }
 #[test]
 fn unrelated_native_socket_is_not_a_carrier(){
  let (first,second)=UnixStream::pair().unwrap();assert_eq!(registered_class(first.as_raw_fd()).unwrap(),0);assert!(resolve(second.as_fd()).is_err());
 }
}

#[cfg(test)]
mod reply_failure_tests {
 use super::*;
 use std::{fs::OpenOptions,os::fd::{AsFd,FromRawFd}};
 #[test]
 fn regular_scm_bounded_reply_to_dead_receiver_drops_temporary_writer_refs(){
  const MARKER:&str="REGULAR_SCM_DEAD_REPLY_FIXTURE_EXECUTED";
  if crate::socket_scm::tests::isolated_exec_fixture("regular_scm::reply_failure_tests::regular_scm_bounded_reply_to_dead_receiver_drops_temporary_writer_refs",MARKER){return;}
  let path=std::env::temp_dir().join(format!("aim-scm-dead-reply-{}",std::process::id()));std::fs::create_dir(&path).unwrap();
  let backing=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("source")).unwrap();
  let writer=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("writers")).unwrap();
  let contender=OpenOptions::new().read(true).write(true).open(path.join("writers")).unwrap();assert_eq!(unsafe{libc::flock(writer.as_raw_fd(),libc::LOCK_SH)},0);
  let metadata=RegularMetadata{flags:2,uid:1000,gid:1001,identity:crate::regular_file::identity(backing.as_fd()).unwrap(),writer:true};
  let creation=create(backing.into(),Some(writer.into()),metadata.clone()).unwrap();let carrier=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(creation.ports[0].as_port()).unwrap())};drop(creation);
  let service=mach::new_port(true).unwrap();let reply_port=mach::new_port(true).unwrap();
  let request_port=port(carrier.as_raw_fd()).unwrap();
  let requester=std::thread::spawn(move||{let mut buffer=mach::Buffer::default();let request=mach::Msg{id:crate::wire::RESOLVE_REGULAR_SCM,ports:vec![(request_port.as_port(),mach::COPY_SEND)],data:vec![]};mach::call(&mut buffer,service,reply_port,&request).is_err()});
  let mut buffer=mach::Buffer::default();let request=mach::receive(&mut buffer,service).unwrap();
  let export=resolve(carrier.as_fd()).unwrap();drop(carrier);for port in &request.ports{mach::release_send(*port);}
  mach::destroy_receive(reply_port);assert!(requester.join().unwrap());
  let mut data=Writer::default();data.i32(0);data.0.extend_from_slice(export.data());let message=mach::Msg{id:crate::wire::REPLY,ports:export.ports(),data:data.0};
  // Darwin can consume/discard a dead send-once reply successfully. The
  // receiver's actual RPC error and final writer release are the oracle.
  if mach::reply_bounded(&mut buffer,request.reply,&message,1000).is_err(){mach::release_send(request.reply);}
  assert!(mach::reply_bounded(&mut buffer,mach::NULL,&message,1).is_err(),"invalid destinations return genuine send failure");
  drop(export);
  drain(&metadata.identity).unwrap();assert_eq!(unsafe{libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
  mach::release_send(reply_port);mach::drop_own_send(service);mach::destroy_receive(service);std::fs::remove_dir_all(path).unwrap();println!("{MARKER}");
 }
}
