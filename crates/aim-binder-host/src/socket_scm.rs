//! Socket receipts travel in one opaque pipe writer. The registry reader
//! observes the last actual kernel reference, including unconnected recipients.
use std::{collections::HashMap,io,os::{fd::{AsFd,AsRawFd,BorrowedFd,OwnedFd},unix::net::UnixStream},sync::{LazyLock,Mutex,MutexGuard}};
use aim_storage::socket_inode::Receipt;
use std::os::unix::ffi::OsStrExt;
use crate::proxy_file::{send,receive_flags};
#[path="socket_scm_pipe_token.rs"] mod pipe_token;
pub const CLASS:u32=5;
struct Entry {reader:pipe_token::Reader,backing:OwnedFd,receipt:Receipt}
static REGISTRY:LazyLock<Mutex<HashMap<(u64,u64),Entry>>>=LazyLock::new(Default::default);
pub(crate) struct NativeReply {pub descriptor:Option<OwnedFd>,pub receipt:Option<Receipt>,pub class:u32,_registry:MutexGuard<'static,HashMap<(u64,u64),Entry>>}
pub struct Reply {pub descriptor:Option<OwnedFd>,pub receipt:Option<Receipt>,pub class:u32}
const CREATE:u8=1;const RESOLVE:u8=2;const CLASSIFY:u8=3;const DRAIN:u8=4;
const REQUEST:usize=41;const RESPONSE:usize=48;
fn carrier_identity(fd:i32)->io::Result<Option<(u64,u64)>>{pipe_token::key(fd)}
fn create_native(backing:OwnedFd,receipt:Receipt)->io::Result<NativeReply>{
 receipt.validate(backing.as_raw_fd())?;let mut registry=REGISTRY.lock().unwrap();prune(&mut registry)?;
 let(reader,carrier)=pipe_token::create()?;let identity=reader.key;
 if registry.contains_key(&identity){return Err(error(libc::EPROTO));}
 registry.insert(identity,Entry{reader,backing,receipt});
 Ok(NativeReply{descriptor:Some(carrier),receipt:Some(receipt),class:CLASS,_registry:registry})
}
fn resolve_native(carrier:BorrowedFd<'_>)->io::Result<NativeReply>{
 use std::os::fd::FromRawFd;
 let identity=carrier_identity(carrier.as_raw_fd())?.ok_or_else(||error(libc::ENOENT))?;let registry=REGISTRY.lock().unwrap();let entry=registry.get(&identity).ok_or_else(||error(libc::ENOENT))?;
 if !entry.reader.matches(identity)?{return Err(error(libc::ENOENT));}
 entry.receipt.validate(entry.backing.as_raw_fd())?;
 let fd=unsafe{libc::fcntl(entry.backing.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{return Err(io::Error::last_os_error());}
 Ok(NativeReply{descriptor:Some(unsafe{OwnedFd::from_raw_fd(fd)}),receipt:Some(entry.receipt),class:CLASS,_registry:registry})
}
fn drain_native()->io::Result<NativeReply>{let mut registry=REGISTRY.lock().unwrap();prune(&mut registry)?;Ok(NativeReply{descriptor:None,receipt:None,class:0,_registry:registry})}
/// Mach discovery carries address/authentication bytes only; actual socket
/// descriptors travel exclusively through the authenticated Unix channel.
#[derive(Clone,Debug)]
pub struct Endpoint{pub path:Vec<u8>,pub process:aim_storage::process_namespace::ProcessIdentity,pub uid:u32,pub nonce:[u8;16]}
impl Endpoint{
 pub(crate) fn encode(&self)->Vec<u8>{let mut writer=crate::wire::Writer::default();writer.bytes(&self.path).i32(self.process.host_pid).u64(self.process.start_seconds).u64(self.process.start_microseconds).u32(self.uid).bytes(&self.nonce);writer.0}
 pub(crate) fn decode(reader:&mut crate::wire::Reader<'_>)->Result<Self,i32>{
  let path=reader.bytes()?.to_vec();let pid=reader.i32()?;let sec=reader.u64()?;let us=reader.u64()?;let uid=reader.u32()?;let nonce=reader.bytes()?.try_into().map_err(|_|crate::wire::EPROTO)?;
  if path.is_empty()||path.len()>=104||path[0]!=b'/'||path.contains(&0)||pid<=0||us>=1_000_000{return Err(crate::wire::EPROTO);}
  Ok(Self{path,process:aim_storage::process_namespace::ProcessIdentity{host_pid:pid,start_seconds:sec,start_microseconds:us},uid,nonce})
 }
}
pub struct Service{endpoint:Endpoint,directory:std::path::PathBuf,identity:(u64,u64),stop:std::sync::Arc<std::sync::atomic::AtomicBool>,thread:Option<std::thread::JoinHandle<()>>,streams:std::sync::Arc<Mutex<Vec<UnixStream>>>,workers:std::sync::Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>}
impl Service{
 pub fn start()->io::Result<Self>{
  use std::os::unix::fs::{PermissionsExt,MetadataExt};
  let mut nonce=[0;16];unsafe{libc::arc4random_buf(nonce.as_mut_ptr().cast(),nonce.len());}
  let directory=std::env::temp_dir().join(format!("aim-scm-{}-{}",std::process::id(),nonce.iter().map(|byte|format!("{byte:02x}")).collect::<String>()));std::fs::create_dir(&directory)?;std::fs::set_permissions(&directory,std::fs::Permissions::from_mode(0o700))?;
  let listener=std::os::unix::net::UnixListener::bind(directory.join("s"))?;listener.set_nonblocking(true)?;
  use std::os::unix::ffi::OsStrExt;
  let endpoint=Endpoint{path:directory.join("s").as_os_str().as_bytes().to_vec(),process:aim_storage::process_namespace::ProcessIdentity::running(std::process::id()as i32)?,uid:unsafe{libc::geteuid()},nonce};
  let stat=std::fs::metadata(&directory)?;let identity=(stat.dev(),stat.ino());
  let stop=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));let streams=std::sync::Arc::new(Mutex::new(vec![]));let workers=std::sync::Arc::new(Mutex::new(vec![]));let worker_stop=stop.clone();let worker_streams=streams.clone();let worker_handles=workers.clone();let uid=endpoint.uid;
  let thread=std::thread::Builder::new().name("socket-scm-listener".into()).spawn(move||{
   while !worker_stop.load(std::sync::atomic::Ordering::Acquire){
    let mut poll=libc::pollfd{fd:listener.as_raw_fd(),events:libc::POLLIN,revents:0};if unsafe{libc::poll(&mut poll,1,-1)}<=0{continue;}
    let(server,_)=match listener.accept(){Ok(value)=>value,Err(_)=>continue};if worker_stop.load(std::sync::atomic::Ordering::Acquire){break;}
    let retained=match server.try_clone(){Ok(stream)=>stream,Err(_)=>continue};worker_streams.lock().unwrap().push(retained);
    if let Ok(handle)=std::thread::Builder::new().name("socket-scm-control".into()).spawn(move||{
     let _queue_root=match aim_storage::socket_queue_root::SocketQueueRoot::new(server.as_fd()){Ok(root)=>root,Err(_)=>return};
     let mut peer_uid=0;let mut peer_gid=0;if unsafe{libc::getpeereid(server.as_raw_fd(),&mut peer_uid,&mut peer_gid)}<0||peer_uid!=uid{return;}
     if wait_frame(server.as_raw_fd(),16,1000).is_err(){return;}let mut hello=[0;16];let Ok((count,descriptor))=receive_flags(server.as_raw_fd(),&mut hello,libc::MSG_DONTWAIT)else{return;};if count!=16||descriptor.is_some()||hello!=nonce{return;}
     let ready=[0x52u8];if unsafe{libc::send(server.as_raw_fd(),ready.as_ptr().cast(),1,0)}!=1{return;}
     let timeout=libc::timeval{tv_sec:1,tv_usec:0};if unsafe{libc::setsockopt(server.as_raw_fd(),libc::SOL_SOCKET,libc::SO_SNDTIMEO,(&timeout as*const libc::timeval).cast(),std::mem::size_of_val(&timeout)as u32)}<0{return;}
     serve_channel(server,_queue_root);
    }){worker_handles.lock().unwrap().push(handle);}
   }
  })?;
  Ok(Self{endpoint,directory,identity,stop,thread:Some(thread),streams,workers})
 }
 pub fn endpoint(&self)->&Endpoint{&self.endpoint}
}
impl Drop for Service{fn drop(&mut self){
 self.stop.store(true,std::sync::atomic::Ordering::Release);let _=UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&self.endpoint.path)));
 if let Some(thread)=self.thread.take(){if thread.join().is_err(){eprintln!("socket SCM listener unwound");}}
 for stream in self.streams.lock().unwrap().drain(..){let _=stream.shutdown(std::net::Shutdown::Both);}
 for worker in self.workers.lock().unwrap().drain(..){if worker.join().is_err(){eprintln!("socket SCM worker unwound");}}
 use std::os::unix::fs::MetadataExt;use std::os::unix::ffi::OsStrExt;
 if std::fs::metadata(&self.directory).is_ok_and(|stat|(stat.dev(),stat.ino())==self.identity){if let Err(error)=std::fs::remove_dir_all(&self.directory){eprintln!("socket SCM directory cleanup: {error}");}}
}}
/// Caller creates/connects the actual socket with its private allocation guard.
pub fn authenticate(channel:i32,endpoint:&Endpoint)->io::Result<()>{
 let mut peer=0i32;let mut length=4;if unsafe{libc::getsockopt(channel,0,2,(&mut peer as*mut i32).cast(),&mut length)}<0{return Err(io::Error::last_os_error());}
 let mut uid=0;let mut gid=0;if unsafe{libc::getpeereid(channel,&mut uid,&mut gid)}<0{return Err(io::Error::last_os_error());}
 if peer!=endpoint.process.host_pid||uid!=endpoint.uid||!endpoint.process.is_live(){return Err(error(libc::EPERM));}
 let deadline=std::time::Instant::now()+std::time::Duration::from_millis(1000);let mut sent=0;
 while sent<endpoint.nonce.len(){
  check_auth_deadline(deadline)?;
  let count=unsafe{libc::send(channel,endpoint.nonce[sent..].as_ptr().cast(),endpoint.nonce.len()-sent,libc::MSG_DONTWAIT|libc::MSG_NOSIGNAL)};
  if count>0{sent+=count as usize;continue;}if count==0{return Err(error(libc::EIO));}
  let failure=io::Error::last_os_error();match failure.kind(){io::ErrorKind::Interrupted=>continue,io::ErrorKind::WouldBlock=>wait_auth_io(channel,libc::POLLOUT,deadline)?,_=>return Err(failure)}
 }
 loop{
  check_auth_deadline(deadline)?;let mut ready=[0];
  match receive_flags(channel,&mut ready,libc::MSG_DONTWAIT){
   Ok((count,descriptor))=>{if count!=1||descriptor.is_some()||ready!=[0x52]{return Err(error(libc::EPROTO));}return Ok(());},
   Err(failure)=>match failure.kind(){io::ErrorKind::Interrupted=>continue,io::ErrorKind::WouldBlock=>wait_auth_io(channel,libc::POLLIN,deadline)?,_=>return Err(failure)},
  }
 }
}
fn check_auth_deadline(deadline:std::time::Instant)->io::Result<()>{if std::time::Instant::now()>=deadline{Err(io::Error::new(io::ErrorKind::TimedOut,"socket SCM authentication timeout"))}else{Ok(())}}
fn wait_auth_io(fd:i32,events:i16,deadline:std::time::Instant)->io::Result<()>{
 loop{
  check_auth_deadline(deadline)?;let remaining=deadline.saturating_duration_since(std::time::Instant::now()).as_millis().max(1).min(i32::MAX as u128)as i32;
  let mut event=libc::pollfd{fd,events,revents:0};let count=unsafe{libc::poll(&mut event,1,remaining)};
  if count<0{let failure=io::Error::last_os_error();if failure.kind()==io::ErrorKind::Interrupted{continue;}return Err(failure);}if count==0{continue;}
  if event.revents&libc::POLLNVAL!=0{return Err(error(libc::EBADF));}if event.revents&events!=0{return Ok(());}if event.revents&(libc::POLLERR|libc::POLLHUP)!=0{return Err(error(libc::ECONNRESET));}
 }
}
fn serve_channel(server:UnixStream,queue_root:aim_storage::socket_queue_root::SocketQueueRoot){
  loop{
   match wait_frame(server.as_raw_fd(),REQUEST,1000){Ok(())=>{},Err(failure)if failure.kind()==io::ErrorKind::TimedOut=>continue,Err(_)=>break}
   let mut bytes=[0;REQUEST];let received=match crate::proxy_file::receive_flags_phase(server.as_raw_fd(),&mut bytes,libc::MSG_DONTWAIT){
    Ok(received)=>Ok(received),
    Err(failure)if !failure.consumed&&matches!(failure.error.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>continue,
    Err(failure)=>Err(failure.error),
   };
   let result=received.and_then(|(count,descriptor)|{
    if count!=REQUEST{return Err(error(libc::EPROTO));}
    pipe_token::healthy()?;
    if bytes[0]==DRAIN{if descriptor.is_some()||bytes[1..].iter().any(|byte|*byte!=0){return Err(error(libc::EPROTO));}return drain_native();}
    let descriptor=descriptor.ok_or_else(||error(libc::EPROTO))?;
    match bytes[0]{CREATE=>create_native(descriptor,Receipt::from_bytes(&bytes[1..])?),RESOLVE=>resolve_native(descriptor.as_fd()),CLASSIFY=>{
     let class=crate::server::registered_descriptor_class(descriptor.as_raw_fd())?;let registry=REGISTRY.lock().unwrap();
     let receipt=if class==CLASS{let identity=carrier_identity(descriptor.as_raw_fd())?.ok_or_else(||error(libc::EPROTO))?;Some(registry.get(&identity).ok_or_else(||error(libc::ENOENT))?.receipt)}else{None};
     Ok(NativeReply{descriptor:None,receipt,class,_registry:registry})
    },_=>Err(error(libc::EPROTO))}
   });
   let mut output=[0u8;RESPONSE];let guard=match result{Ok(guard)=>{output[4..8].copy_from_slice(&guard.class.to_le_bytes());if let Some(receipt)=guard.receipt{output[8..].copy_from_slice(&receipt.to_bytes());}Some(guard)},Err(failure)=>{output[..4].copy_from_slice(&(crate::server::socket_scm_errno(failure)as i32).to_le_bytes());None}};
   let sent=if let Some(fd)=guard.as_ref().and_then(|guard|guard.descriptor.as_ref()){send(server.as_raw_fd(),&output,fd.as_raw_fd())}else{let count=unsafe{libc::send(server.as_raw_fd(),output.as_ptr().cast(),output.len(),0)};if count==output.len()as isize{Ok(())}else{Err(io::Error::last_os_error())}};
   drop(guard);if sent.is_err(){break;}
  }
 if let Err(failure)=queue_root.prepare_last_close(server.as_fd()){eprintln!("socket SCM terminal receive cleanup: {failure}");}
 drop(server);drop(queue_root);
 let retired={let mut registry=REGISTRY.lock().unwrap();prune(&mut registry)};
 if let Err(failure)=retired{eprintln!("socket token terminal retirement: {failure}");}
}
fn wait_frame(fd:i32,length:usize,timeout_ms:i32)->io::Result<()> {
 let start=std::time::Instant::now();
 loop{let remaining=timeout_ms as i64-start.elapsed().as_millis()as i64;if remaining<=0{return Err(io::Error::new(io::ErrorKind::TimedOut,"socket SCM frame timeout"));}
  let mut descriptor=libc::pollfd{fd,events:libc::POLLIN,revents:0};let ready=unsafe{libc::poll(&mut descriptor,1,remaining as i32)};
  if ready<0{let failure=io::Error::last_os_error();if failure.kind()==io::ErrorKind::Interrupted{continue;}return Err(failure);}if ready==0{continue;}
  let mut available=0i32;if unsafe{libc::ioctl(fd,libc::FIONREAD,&mut available)}<0{return Err(io::Error::last_os_error());}
  if available>=length as i32{return Ok(());}if descriptor.revents&(libc::POLLHUP|libc::POLLERR|libc::POLLNVAL)!=0{return Err(error(libc::ECONNRESET));}
 }
}
fn request(channel:i32,opcode:u8,descriptor:i32,receipt:Option<Receipt>)->io::Result<()> {let mut bytes=[0;REQUEST];bytes[0]=opcode;if let Some(receipt)=receipt{bytes[1..].copy_from_slice(&receipt.to_bytes());}send(channel,&bytes,descriptor)}
pub fn request_create(channel:i32,backing:i32,receipt:Receipt)->io::Result<()>{request(channel,CREATE,backing,Some(receipt))}
pub fn request_resolve(channel:i32,carrier:i32)->io::Result<()>{request(channel,RESOLVE,carrier,None)}
pub fn request_classify(channel:i32,descriptor:i32)->io::Result<()>{request(channel,CLASSIFY,descriptor,None)}
/// Complete this request/reply before observing peer EOF after carrier close.
pub fn request_drain(channel:i32)->io::Result<()>{let mut bytes=[0;REQUEST];bytes[0]=DRAIN;let count=unsafe{libc::send(channel,bytes.as_ptr().cast(),bytes.len(),0)};if count<0{return Err(io::Error::last_os_error());}if count!=REQUEST as isize{return Err(error(libc::EIO));}Ok(())}
/// Wait without holding the ABI allocation gate. The single channel owner
/// serializes requests and the following private receive allocation.
pub fn wait_reply(channel:i32)->io::Result<()>{wait_frame(channel,RESPONSE,1000)}
/// Call inside the ABI's receive/private allocation guard. No blocking I/O.
pub fn receive_reply(channel:i32)->io::Result<Reply>{
 let mut bytes=[0;RESPONSE];let(count,descriptor)=receive_flags(channel,&mut bytes,libc::MSG_DONTWAIT)?;
 decode_reply(&bytes,count,descriptor)
}
fn decode_reply(bytes:&[u8;RESPONSE],count:usize,descriptor:Option<OwnedFd>)->io::Result<Reply>{
 if count!=RESPONSE{return Err(error(libc::EPROTO));}let status=i32::from_le_bytes(bytes[..4].try_into().unwrap());if status!=0{return Err(error(crate::server::socket_scm_darwin_errno(status)));}
 let class=u32::from_le_bytes(bytes[4..8].try_into().unwrap());if !matches!(class,0|crate::proxy_file::CLASS|crate::path_file::CLASS|crate::regular_scm::CLASS|CLASS){return Err(error(libc::EPROTO));}
 let receipt=if class==CLASS{Some(Receipt::from_bytes(&bytes[8..])?)}else{if bytes[8..].iter().any(|byte|*byte!=0){return Err(error(libc::EPROTO));}None};
 Ok(Reply{descriptor,receipt,class})
}
fn error(value:i32)->io::Error{io::Error::from_raw_os_error(value)}
fn prune(registry:&mut HashMap<(u64,u64),Entry>)->io::Result<()> {
 let mut dead=vec![];
 for(key,entry)in registry.iter(){if entry.reader.eof()?{dead.push(*key);}}
 for key in dead{registry.remove(&key);}Ok(())
}
fn token_event(fd:i32,tag:usize,eof:bool)->io::Result<()>{
 let mut registry=REGISTRY.lock().unwrap();let key=registry.iter().find_map(|(key,entry)|(entry.reader.fd()==fd&&entry.reader.tag==tag).then_some(*key));
 if let Some(key)=key{let entry=registry.get(&key).unwrap();if entry.reader.eof()?{if !eof{return Err(error(libc::EPROTO));}registry.remove(&key);}}Ok(())
}
pub(crate) fn registered_class(fd:i32)->io::Result<u32>{let Some(identity)=carrier_identity(fd)?else{return Ok(0)};let registry=REGISTRY.lock().unwrap();match registry.get(&identity){Some(entry)if entry.reader.matches(identity)?=>Ok(CLASS),_=>Ok(0)}}

#[cfg(test)]
pub(crate) mod tests {
 use super::*;
 use std::io::Read;
 fn receive_ready(channel:i32)->io::Result<Reply>{
  let deadline=std::time::Instant::now()+std::time::Duration::from_millis(1000);
  loop{
   check_auth_deadline(deadline)?;let remaining=deadline.saturating_duration_since(std::time::Instant::now()).as_millis().max(1).min(1000)as i32;wait_frame(channel,RESPONSE,remaining)?;
   let mut bytes=[0;RESPONSE];match receive_flags(channel,&mut bytes,libc::MSG_DONTWAIT){
    Ok((count,descriptor))=>return decode_reply(&bytes,count,descriptor),
    Err(failure)if matches!(failure.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>continue,
    Err(failure)=>return Err(failure),
   }
  }
 }
 struct Channel{socket:OwnedFd,_root:aim_storage::socket_queue_root::SocketQueueRoot,_service:Service}
 impl Channel{
  fn new()->Self{let service=Service::start().unwrap();let stream=UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&service.endpoint().path))).unwrap();authenticate(stream.as_raw_fd(),service.endpoint()).unwrap();let root=aim_storage::socket_queue_root::SocketQueueRoot::new(stream.as_fd()).unwrap();Self{socket:stream.into(),_root:root,_service:service}}
  fn create(&self,backing:OwnedFd,receipt:Receipt)->OwnedFd{request_create(self.socket.as_raw_fd(),backing.as_raw_fd(),receipt).unwrap();let reply=receive_ready(self.socket.as_raw_fd()).unwrap();assert_eq!(reply.receipt,Some(receipt));reply.descriptor.unwrap()}
  fn resolve(&self,carrier:&OwnedFd)->Reply{request_resolve(self.socket.as_raw_fd(),carrier.as_raw_fd()).unwrap();receive_ready(self.socket.as_raw_fd()).unwrap()}
 }
 fn carrier(backing:OwnedFd,receipt:Receipt)->OwnedFd{Channel::new().create(backing,receipt)}
 fn live(peer:&UnixStream){let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as *mut u8).cast(),1,libc::MSG_DONTWAIT)},-1);assert_eq!(io::Error::last_os_error().kind(),io::ErrorKind::WouldBlock);}
 fn drain(){let channel=Channel::new();request_drain(channel.socket.as_raw_fd()).unwrap();let reply=receive_ready(channel.socket.as_raw_fd()).unwrap();assert_eq!(reply.class,0);assert!(reply.descriptor.is_none());assert!(reply.receipt.is_none());}
 fn eof(peer:&UnixStream){drain();let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as *mut u8).cast(),1,libc::MSG_DONTWAIT)},0,"registry must not anchor the real socket after final carrier close");}
 #[test]
 fn socket_scm_repeated_resolve_preserves_receipt_and_actual_socket_without_registry_anchor(){
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let carrier=carrier(backing.into(),receipt);
  for _ in 0..2{
   let reply=Channel::new().resolve(&carrier);assert_eq!(reply.receipt,Some(receipt));let descriptor=reply.descriptor.unwrap();receipt.validate(descriptor.as_raw_fd()).unwrap();drop(descriptor);live(&peer);
  }
  drop(carrier);eof(&peer);
 }
 #[test]
 fn socket_scm_outer_queued_rights_hold_socket_but_discard_closes_it_immediately(){
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let carrier=carrier(backing.into(),receipt);
  let(sender,receiver)=UnixStream::pair().unwrap();let root=aim_storage::socket_queue_root::SocketQueueRoot::new(receiver.as_fd()).unwrap();send(sender.as_raw_fd(),b"guest",carrier.as_raw_fd()).unwrap();drop(carrier);live(&peer);drop(receiver);drop(root);eof(&peer);
 }
 #[test]
 fn socket_scm_outer_receipt_preserves_payload_and_one_fd_then_retained_import(){
  let(backing,mut peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let carrier=carrier(backing.into(),receipt);
  let(sender,receiver)=UnixStream::pair().unwrap();let _root=aim_storage::socket_queue_root::SocketQueueRoot::new(receiver.as_fd()).unwrap();send(sender.as_raw_fd(),b"guest",carrier.as_raw_fd()).unwrap();drop(carrier);
  let mut data=[0;5];let(count,received)=receive_flags(receiver.as_raw_fd(),&mut data,0).unwrap();assert_eq!(count,5);assert_eq!(&data,b"guest");let received=received.unwrap();assert_eq!(registered_class(received.as_raw_fd()).unwrap(),CLASS);
  let reply=Channel::new().resolve(&received);let mut imported=std::fs::File::from(reply.descriptor.unwrap());drop(received);live(&peer);
  use std::io::Write;receipt.validate(imported.as_raw_fd()).unwrap();peer.write_all(b"real").unwrap();let mut bytes=[0;4];imported.read_exact(&mut bytes).unwrap();assert_eq!(&bytes,b"real");drop(imported);eof(&peer);
 }
 #[test]
 fn socket_scm_reexport_and_reply_receiver_death_release_actual_kernel_lease(){
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let carrier=carrier(backing.into(),receipt);
  let(first_sender,first_receiver)=UnixStream::pair().unwrap();send(first_sender.as_raw_fd(),b"x",carrier.as_raw_fd()).unwrap();drop(carrier);drain();live(&peer);
  let mut byte=[0];let(_,received)=receive_flags(first_receiver.as_raw_fd(),&mut byte,0).unwrap();let received=received.unwrap();
  let(second_sender,second_receiver)=UnixStream::pair().unwrap();send(second_sender.as_raw_fd(),b"y",received.as_raw_fd()).unwrap();drop(received);drain();live(&peer);
  drop(second_receiver);eof(&peer);
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let channel=Channel::new();
  request_create(channel.socket.as_raw_fd(),backing.as_raw_fd(),receipt).unwrap();drop(backing);wait_reply(channel.socket.as_raw_fd()).unwrap();
  channel._root.prepare_last_close(channel.socket.as_fd()).unwrap();drop(channel);eof(&peer);
 }
 pub(crate) fn isolated_exec_fixture(name:&str,marker:&str)->bool{
  if std::env::args().any(|arg|arg==marker){return false;}
  use std::{ffi::CString,os::fd::FromRawFd,os::unix::ffi::OsStrExt};
  let executable=CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();let arguments=[executable.clone(),CString::new("--exact").unwrap(),CString::new(name).unwrap(),CString::new("--skip").unwrap(),CString::new(marker).unwrap(),CString::new("--nocapture").unwrap()];let mut argv=arguments.iter().map(|arg|arg.as_ptr()as*mut libc::c_char).collect::<Vec<_>>();argv.push(std::ptr::null_mut());let environment=[std::ptr::null_mut::<libc::c_char>()];
  let mut fds=[0;2];assert_eq!(unsafe{libc::pipe(fds.as_mut_ptr())},0);let(reader,writer)=unsafe{(OwnedFd::from_raw_fd(fds[0]),OwnedFd::from_raw_fd(fds[1]))};
  struct Spawn{attributes:libc::posix_spawnattr_t,actions:libc::posix_spawn_file_actions_t}
  impl Drop for Spawn{fn drop(&mut self){unsafe{if !self.actions.is_null(){libc::posix_spawn_file_actions_destroy(&mut self.actions);}if !self.attributes.is_null(){libc::posix_spawnattr_destroy(&mut self.attributes);}}}}
  let mut spawn=Spawn{attributes:std::ptr::null_mut(),actions:std::ptr::null_mut()};let mut pid=0;
  unsafe{
   assert_eq!(libc::posix_spawnattr_init(&mut spawn.attributes),0);assert_eq!(libc::posix_spawn_file_actions_init(&mut spawn.actions),0);assert_eq!(libc::posix_spawnattr_setflags(&mut spawn.attributes,libc::POSIX_SPAWN_CLOEXEC_DEFAULT as i16),0);
   assert_eq!(libc::posix_spawn_file_actions_adddup2(&mut spawn.actions,writer.as_raw_fd(),1),0);assert_eq!(libc::posix_spawn_file_actions_adddup2(&mut spawn.actions,writer.as_raw_fd(),2),0);
   assert_eq!(libc::posix_spawn(&mut pid,executable.as_ptr(),&spawn.actions,&spawn.attributes,argv.as_ptr(),environment.as_ptr()),0);
  }
  struct Child(Option<i32>);impl Drop for Child{fn drop(&mut self){if let Some(pid)=self.0{unsafe{libc::kill(pid,libc::SIGKILL);libc::waitpid(pid,std::ptr::null_mut(),0);}}}}
  let mut child=Child(Some(pid));drop(spawn);drop(writer);let mut bytes=vec![];std::fs::File::from(reader).read_to_end(&mut bytes).unwrap();let mut status=0;assert_eq!(unsafe{libc::waitpid(pid,&mut status,0)},pid);child.0=None;
  let text=String::from_utf8_lossy(&bytes);assert!(libc::WIFEXITED(status)&&libc::WEXITSTATUS(status)==0,"isolated exec fixture failed: {text}");assert!(text.contains(marker),"isolated exec fixture did not execute: {text}");true
 }
 #[test]
 fn socket_scm_received_private_backing_and_carrier_do_not_anchor_in_exec_child(){
  const MARKER:&str="PRIVATE_SCM_CLOEXEC_FIXTURE_EXECUTED";
  if isolated_exec_fixture("socket_scm::tests::socket_scm_received_private_backing_and_carrier_do_not_anchor_in_exec_child",MARKER){return;}
  use std::process::{Command,Stdio};
  struct Child(std::process::Child);
  impl Drop for Child{fn drop(&mut self){if self.0.try_wait().unwrap().is_none(){self.0.kill().unwrap();self.0.wait().unwrap();}}}
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let carrier=carrier(backing.into(),receipt);
  let flags=unsafe{libc::fcntl(carrier.as_raw_fd(),libc::F_GETFD)};assert!(flags>=0);assert_ne!(flags&libc::FD_CLOEXEC,0);
  let identity=carrier_identity(carrier.as_raw_fd()).unwrap().unwrap();
  {let registry=REGISTRY.lock().unwrap();let backing=registry.get(&identity).unwrap().backing.as_raw_fd();let flags=unsafe{libc::fcntl(backing,libc::F_GETFD)};assert!(flags>=0);assert_ne!(flags&libc::FD_CLOEXEC,0);}
  let reply=Channel::new().resolve(&carrier);let imported=reply.descriptor.unwrap();let flags=unsafe{libc::fcntl(imported.as_raw_fd(),libc::F_GETFD)};assert!(flags>=0);assert_ne!(flags&libc::FD_CLOEXEC,0);drop(imported);
  let mut child=Child(Command::new("/bin/cat").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
  assert!(child.0.try_wait().unwrap().is_none());drop(carrier);eof(&peer);
  drop(child.0.stdin.take());assert!(child.0.wait().unwrap().success());println!("{MARKER}");
 }
 #[test]
 fn socket_scm_nonblocking_authentication_sends_one_hello_then_accepts_real_request(){
  let service=Service::start().unwrap();let channel=UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&service.endpoint().path))).unwrap();channel.set_nonblocking(true).unwrap();
  let root=aim_storage::socket_queue_root::SocketQueueRoot::new(channel.as_fd()).unwrap();authenticate(channel.as_raw_fd(),service.endpoint()).unwrap();
  let(first,_second)=UnixStream::pair().unwrap();request_classify(channel.as_raw_fd(),first.as_raw_fd()).unwrap();wait_reply(channel.as_raw_fd()).unwrap();let reply=receive_reply(channel.as_raw_fd()).unwrap();assert_eq!(reply.class,0);assert!(reply.descriptor.is_none());assert!(reply.receipt.is_none());
  root.prepare_last_close(channel.as_fd()).unwrap();drop(channel);drop(root);drop(service);
 }
 #[test]
 fn socket_scm_parallel_creators_preserve_exact_leases_and_original_backing_io(){
  const MARKER:&str="SCM_PARALLEL_PRIVATE_NAMESPACE_EXECUTED";if isolated_exec_fixture("socket_scm::tests::socket_scm_parallel_creators_preserve_exact_leases_and_original_backing_io",MARKER){return;}
  let service=std::sync::Arc::new(Service::start().unwrap());let mut workers=vec![];
  for _ in 0..4{let service=service.clone();workers.push(std::thread::spawn(move||{
   let channel=UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&service.endpoint().path))).unwrap();let root=aim_storage::socket_queue_root::SocketQueueRoot::new(channel.as_fd()).unwrap();authenticate(channel.as_raw_fd(),service.endpoint()).unwrap();
   for _ in 0..128{
    let(backing,mut peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();request_create(channel.as_raw_fd(),backing.as_raw_fd(),receipt).unwrap();let reply=receive_ready(channel.as_raw_fd()).unwrap();assert_eq!(reply.receipt,Some(receipt));let carrier=reply.descriptor.unwrap();drop(backing);
    request_resolve(channel.as_raw_fd(),carrier.as_raw_fd()).unwrap();let reply=receive_ready(channel.as_raw_fd()).unwrap();assert_eq!(reply.receipt,Some(receipt));let mut imported=std::fs::File::from(reply.descriptor.unwrap());receipt.validate(imported.as_raw_fd()).unwrap();drop(carrier);
    request_drain(channel.as_raw_fd()).unwrap();let reply=receive_ready(channel.as_raw_fd()).unwrap();assert_eq!(reply.class,0);assert!(reply.descriptor.is_none());
    use std::io::Write;peer.write_all(b"real").unwrap();let mut bytes=[0;4];imported.read_exact(&mut bytes).unwrap();assert_eq!(&bytes,b"real");drop(imported);
    let mut byte=0u8;let eof=unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)};
    if eof!=0{let failure=io::Error::last_os_error();eprintln!("parallel SCM final EOF result={eof} error={failure} registry_live={}",REGISTRY.lock().unwrap().values().any(|entry|entry.receipt==receipt));for fd in 0..unsafe{libc::getdtablesize()}{if receipt.validate(fd).is_ok(){eprintln!("parallel SCM live backing fd={fd} fdflags={}",unsafe{libc::fcntl(fd,libc::F_GETFD)});}}}
    assert_eq!(eof,0);
   }
   root.prepare_last_close(channel.as_fd()).unwrap();drop(channel);drop(root);
  }));}
  for worker in workers{worker.join().unwrap();}println!("{MARKER}");
 }
 #[test]
 fn socket_scm_rejects_foreign_receipt_and_unregistered_socket(){
  let(backing,peer)=UnixStream::pair().unwrap();let foreign=Receipt::mint(peer.as_raw_fd()).unwrap();assert!(create_native(backing.into(),foreign).is_err());
  let(first,second)=UnixStream::pair().unwrap();assert_eq!(registered_class(first.as_raw_fd()).unwrap(),0);assert!(resolve_native(second.as_fd()).is_err());
 }


 #[test]
 fn socket_scm_pipe_token_outlives_service_and_preserves_live_writer_alias(){
  const MARKER:&str="PIPE_TOKEN_SERVICE_LIFETIME_EXECUTED";if isolated_exec_fixture("socket_scm::tests::socket_scm_pipe_token_outlives_service_and_preserves_live_writer_alias",MARKER){return;}
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let token=carrier(backing.into(),receipt);let key=carrier_identity(token.as_raw_fd()).unwrap().unwrap();
  assert_eq!(registered_class(token.as_raw_fd()).unwrap(),CLASS);let alias=token.try_clone().unwrap();drop(token);live(&peer);
  let(reader,ordinary)=pipe_token::create().unwrap();assert_eq!(registered_class(ordinary.as_raw_fd()).unwrap(),0,"unregistered writer cannot impersonate class5");assert_eq!(registered_class(reader.fd()).unwrap(),0,"reader is not a writer capability");assert_ne!(unsafe{libc::fcntl(reader.fd(),libc::F_GETFD)}&libc::FD_CLOEXEC,0);drop(ordinary);drop(reader);
  // The creator service guard has already been dropped by carrier(). No
  // reconnect, DRAIN, process watch or receipt query may be needed for EOF.
  let mut event=libc::pollfd{fd:peer.as_raw_fd(),events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut event,1,0)},0);drop(alias);
  assert_eq!(unsafe{libc::poll(&mut event,1,1000)},1);let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},0);assert!(!REGISTRY.lock().unwrap().contains_key(&key));println!("{MARKER}");
 }
 #[test]
 fn socket_scm_reactor_start_failure_rejects_before_token_publish(){
  const MARKER:&str="PIPE_TOKEN_REACTOR_FAILURE_EXECUTED";if isolated_exec_fixture("socket_scm::tests::socket_scm_reactor_start_failure_rejects_before_token_publish",MARKER){return;}
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let mut limit:libc::rlimit=unsafe{std::mem::zeroed()};assert_eq!(unsafe{libc::getrlimit(libc::RLIMIT_NOFILE,&mut limit)},0);
  struct Limit(libc::rlimit);impl Drop for Limit{fn drop(&mut self){assert_eq!(unsafe{libc::setrlimit(libc::RLIMIT_NOFILE,&self.0)},0);}}
  let restore=Limit(limit);let blocked=libc::rlimit{rlim_cur:0,rlim_max:limit.rlim_max};assert_eq!(unsafe{libc::setrlimit(libc::RLIMIT_NOFILE,&blocked)},0);
  let error=match create_native(backing.into(),receipt){Ok(_)=>panic!("real kqueue allocation failure published a token"),Err(error)=>error};assert_eq!(error.raw_os_error(),Some(libc::EMFILE));assert!(REGISTRY.lock().unwrap().is_empty());
  let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},0,"failed creation must close its actual backing without a phantom C");drop(restore);
  let(backing,peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let reply=create_native(backing.into(),receipt).unwrap();let writer=reply.descriptor.as_ref().unwrap();assert_ne!(unsafe{libc::fcntl(writer.as_raw_fd(),libc::F_GETFD)}&libc::FD_CLOEXEC,0);let key=carrier_identity(writer.as_raw_fd()).unwrap().unwrap();assert_ne!(unsafe{libc::fcntl(reply._registry.get(&key).unwrap().reader.fd(),libc::F_GETFD)}&libc::FD_CLOEXEC,0);drop(reply);
  let mut ready=libc::pollfd{fd:peer.as_raw_fd(),events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut ready,1,1000)},1);assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},0);println!("{MARKER}");
 }
 #[test]
 #[ignore="owned SIGKILL helper for socket_scm_sigkill_final_queued_lease_wakes_blocking_peer_without_drain"]
 fn socket_scm_queued_lease_sigkill_child(){
  use std::io::Write;
  let argument=std::env::args().find(|arg|arg.starts_with("SOCKET_DEATH_ENDPOINT=")).unwrap();
  let encoded=argument.strip_prefix("SOCKET_DEATH_ENDPOINT=").unwrap();
  let bytes=(0..encoded.len()).step_by(2).map(|at|u8::from_str_radix(&encoded[at..at+2],16).unwrap()).collect::<Vec<_>>();
  let endpoint=Endpoint::decode(&mut crate::wire::Reader::new(&bytes)).unwrap();
  let channel=if std::env::args().any(|arg|arg=="SOCKET_DEATH_PREAUTH"){None}else{let channel=UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&endpoint.path))).unwrap();authenticate(channel.as_raw_fd(),&endpoint).unwrap();Some(channel)};
  let _control_root=channel.as_ref().map(|channel|aim_storage::socket_queue_root::SocketQueueRoot::new(channel.as_fd()).unwrap());
  let receiver=unsafe{BorrowedFd::borrow_raw(0)};
  let _receive_root=aim_storage::socket_queue_root::SocketQueueRoot::new(receiver).unwrap();
  let mut ready=libc::pollfd{fd:0,events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut ready,1,1000)},1);assert_ne!(ready.revents&libc::POLLIN,0);
  println!("SOCKET_DEATH_QUEUED_ONLY_READY");std::io::stdout().flush().unwrap();
  loop{unsafe{libc::pause();}}
 }
 #[test]
 fn socket_scm_sigkill_final_queued_lease_wakes_blocking_peer_without_drain(){death_proof(false);}
 #[test]
 fn socket_scm_preauth_sigkill_final_queued_lease_wakes_blocking_peer_without_drain(){death_proof(true);}
 fn death_proof(preauth:bool){
  const MARKER:&str="SOCKET_DEATH_RAW_BLOCKING_ORACLE_EXECUTED";
  let name=if preauth{"socket_scm::tests::socket_scm_preauth_sigkill_final_queued_lease_wakes_blocking_peer_without_drain"}else{"socket_scm::tests::socket_scm_sigkill_final_queued_lease_wakes_blocking_peer_without_drain"};
  if isolated_exec_fixture(name,MARKER){return;}
  use std::{io::BufRead,process::{Command,Stdio},sync::mpsc,time::Duration};
  struct Child(Option<std::process::Child>);
  impl Drop for Child{fn drop(&mut self){if let Some(mut child)=self.0.take(){if child.try_wait().unwrap().is_none(){child.kill().unwrap();}child.wait().unwrap();}}}
  let channel=Channel::new();let(backing,mut peer)=UnixStream::pair().unwrap();let receipt=Receipt::mint(backing.as_raw_fd()).unwrap();let carrier=channel.create(backing.into(),receipt);
  let(sender,receiver)=UnixStream::pair().unwrap();send(sender.as_raw_fd(),b"queued",carrier.as_raw_fd()).unwrap();drop(carrier);
  let encoded=channel._service.endpoint().encode().iter().map(|byte|format!("{byte:02x}")).collect::<String>();
  let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact","socket_scm::tests::socket_scm_queued_lease_sigkill_child","--ignored","--nocapture","--skip",&format!("SOCKET_DEATH_ENDPOINT={encoded}")]);if preauth{command.args(["--skip","SOCKET_DEATH_PREAUTH"]);}
  let mut child=Child(Some(command.stdin(Stdio::from(OwnedFd::from(receiver))).stdout(Stdio::piped()).spawn().unwrap()));
  drop(command); // The child owns the sole receive queue after spawn.
  let mut output=std::io::BufReader::new(child.0.as_mut().unwrap().stdout.take().unwrap());loop{let mut line=String::new();assert!(output.read_line(&mut line).unwrap()>0);if line.contains("SOCKET_DEATH_QUEUED_ONLY_READY"){break;}}
  drop(sender);live(&peer);
  let(started_send,started)=mpsc::channel();let(done_send,done)=mpsc::channel();
  let reader=std::thread::spawn(move||{started_send.send(()).unwrap();let mut byte=[0];let result=peer.read(&mut byte);done_send.send(result).unwrap();});
  started.recv().unwrap();assert!(matches!(done.recv_timeout(Duration::from_millis(50)),Err(mpsc::RecvTimeoutError::Timeout)),"peer must really be waiting before final owner death");
  let process=child.0.as_mut().unwrap();process.kill().unwrap();let status=process.wait().unwrap();assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status),Some(libc::SIGKILL));child.0=None;
  // This observation contains no CREATE, DRAIN, receipt resolution or guest
  // syscall barrier. A timeout is missing EOF, never successful cleanup.
  let observed=done.recv_timeout(Duration::from_secs(1));let woke=matches!(&observed,Ok(Ok(0)));
  eprintln!("SIGKILL_WITHOUT_DRAIN observed={observed:?}");
  if !woke{
   request_drain(channel.socket.as_raw_fd()).unwrap();let reply=receive_ready(channel.socket.as_raw_fd()).unwrap();assert_eq!(reply.class,0);assert!(reply.descriptor.is_none());
   let recovered=done.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();assert_eq!(recovered,0,"cleanup ACK must release actual native backing");eprintln!("SIGKILL_AFTER_EXPLICIT_CLEANUP_DRAIN EOF=0");
  }
  reader.join().unwrap();println!("{MARKER}");
  assert!(woke,"last queued carrier SIGKILL did not wake real blocking peer without an extra DRAIN");
 }

}
