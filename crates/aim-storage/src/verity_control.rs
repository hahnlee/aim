//! Authenticated native fs-verity control frames (#226). No guest fd is exposed.
use crate::{private_fd::PrivateFd, process_namespace::ProcessIdentity};
use std::{io::{self,Read,Write},os::fd::{AsRawFd,FromRawFd,OwnedFd},os::unix::net::UnixStream,time::Duration};
const MAGIC:&[u8;8]=b"AIMVCTL1";
const SIZE:usize=80;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u32)]
pub enum Operation { Register=1,MapBegin=2,MapLive=3,Unmap=4,Prepare=5,Prepared=6,Abort=7,Aborted=8,Commit=9,Committed=10,Error=11,MapAbort=12,ForkHandoff=13,EnableBegin=14,EnablePrepared=15,EnablePublished=16,EnableComplete=17,ForkBegin=18,ForkIdentity=19 }
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Frame {pub operation:Operation,pub transaction:u64,pub identity:[u8;36],pub generation:u64,pub error:i32}
impl Frame {
 pub fn encode(&self)->[u8;SIZE]{let mut out=[0;SIZE];out[..8].copy_from_slice(MAGIC);out[8..12].copy_from_slice(&(self.operation as u32).to_le_bytes());out[16..24].copy_from_slice(&self.transaction.to_le_bytes());out[24..60].copy_from_slice(&self.identity);out[60..68].copy_from_slice(&self.generation.to_le_bytes());out[68..72].copy_from_slice(&self.error.to_le_bytes());out}
 pub fn decode(input:[u8;SIZE])->io::Result<Self>{if &input[..8]!=MAGIC||input[12..16].iter().chain(input[72..].iter()).any(|b|*b!=0){return Err(error(libc::EPROTO));}let operation=match u32::from_le_bytes(input[8..12].try_into().unwrap()){1=>Operation::Register,2=>Operation::MapBegin,3=>Operation::MapLive,4=>Operation::Unmap,5=>Operation::Prepare,6=>Operation::Prepared,7=>Operation::Abort,8=>Operation::Aborted,9=>Operation::Commit,10=>Operation::Committed,11=>Operation::Error,12=>Operation::MapAbort,13=>Operation::ForkHandoff,14=>Operation::EnableBegin,15=>Operation::EnablePrepared,16=>Operation::EnablePublished,17=>Operation::EnableComplete,18=>Operation::ForkBegin,19=>Operation::ForkIdentity,_=>return Err(error(libc::EPROTO))};Ok(Self{operation,transaction:u64::from_le_bytes(input[16..24].try_into().unwrap()),identity:input[24..60].try_into().unwrap(),generation:u64::from_le_bytes(input[60..68].try_into().unwrap()),error:i32::from_le_bytes(input[68..72].try_into().unwrap())})}
}
fn error(code:i32)->io::Error{io::Error::from_raw_os_error(code)}
/// PID is read from the kernel socket peer, then bound to its actual start time.
pub fn authenticate(stream:&UnixStream,expected:Option<ProcessIdentity>)->io::Result<ProcessIdentity>{
 let mut pid=0i32;let mut length=std::mem::size_of::<i32>()as libc::socklen_t;
 if unsafe{libc::getsockopt(stream.as_raw_fd(),libc::SOL_LOCAL,libc::LOCAL_PEERPID,(&mut pid as *mut i32).cast(),&mut length)}!=0{return Err(io::Error::last_os_error());}
 let(mut uid,mut gid)=(0,0);if unsafe{libc::getpeereid(stream.as_raw_fd(),&mut uid,&mut gid)}!=0{return Err(io::Error::last_os_error());}
 if uid!=unsafe{libc::geteuid()}{return Err(error(libc::EPERM));}
 let actual=ProcessIdentity::running(pid)?;if expected.is_some_and(|e|e!=actual){return Err(error(libc::EPERM));}Ok(actual)
}
pub struct Channel {stream:PrivateFd,pub peer:ProcessIdentity}
impl Channel {
 pub fn shutdown_handle(&self)->io::Result<PrivateFd>{self.stream.try_clone()}
 fn borrowed(&self)->std::mem::ManuallyDrop<UnixStream>{std::mem::ManuallyDrop::new(unsafe{UnixStream::from_raw_fd(self.stream.as_raw_fd())})}
 pub fn new(stream:UnixStream,expected:Option<ProcessIdentity>,timeout:Duration)->io::Result<Self>{Self::from_private(PrivateFd::adopt(stream.into())?,expected,timeout)}
 fn from_private(stream:PrivateFd,expected:Option<ProcessIdentity>,timeout:Duration)->io::Result<Self>{let borrowed=std::mem::ManuallyDrop::new(unsafe{UnixStream::from_raw_fd(stream.as_raw_fd())});let peer=authenticate(&borrowed,expected)?;borrowed.set_read_timeout(Some(timeout))?;borrowed.set_write_timeout(Some(timeout))?;let enabled=1i32;if unsafe{libc::setsockopt(stream.as_raw_fd(),libc::SOL_SOCKET,libc::SO_NOSIGPIPE,(&enabled as*const i32).cast(),std::mem::size_of::<i32>()as _)}<0{return Err(io::Error::last_os_error());}Ok(Self{stream,peer})}
 pub fn send(&mut self,frame:&Frame,proof:Option<&PrivateFd>)->io::Result<()> {
  if !self.peer.is_live(){return Err(error(libc::ESRCH));}
  let bytes=frame.encode();let mut vector=libc::iovec{iov_base:bytes.as_ptr()as*mut _,iov_len:SIZE};let mut message:libc::msghdr=unsafe{std::mem::zeroed()};message.msg_iov=&mut vector;message.msg_iovlen=1;
  let mut control=[0usize;4];if let Some(fd)=proof{message.msg_control=control.as_mut_ptr().cast();message.msg_controllen=unsafe{libc::CMSG_SPACE(std::mem::size_of::<i32>()as u32)}as _;unsafe{let header=libc::CMSG_FIRSTHDR(&message);(*header).cmsg_level=libc::SOL_SOCKET;(*header).cmsg_type=libc::SCM_RIGHTS;(*header).cmsg_len=libc::CMSG_LEN(std::mem::size_of::<i32>()as u32)as _;std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<i32>(),fd.as_raw_fd());}}
  let sent=unsafe{libc::sendmsg(self.stream.as_raw_fd(),&message,0)};if sent<0{return Err(io::Error::last_os_error());}if sent==0{return Err(error(libc::EPIPE));}if (sent as usize)<SIZE{self.borrowed().write_all(&bytes[sent as usize..])?;}Ok(())
 }
 pub fn receive(&mut self)->io::Result<(Frame,Option<PrivateFd>)>{
  let mut bytes=[0u8;SIZE];
  let (received,mut descriptors)=loop{
   let mut ready=libc::pollfd{fd:self.stream.as_raw_fd(),events:libc::POLLIN,revents:0};
   let wait=unsafe{libc::poll(&mut ready,1,self.borrowed().read_timeout()?.unwrap_or(Duration::from_secs(5)).as_millis().min(i32::MAX as u128)as i32)};
   if wait==0{return Err(error(libc::ETIMEDOUT));}if wait<0{let failure=io::Error::last_os_error();if failure.kind()==io::ErrorKind::Interrupted{continue;}return Err(failure);}
   let result=crate::private_fd::receive_allocations(||{
    let mut vector=libc::iovec{iov_base:bytes.as_mut_ptr().cast(),iov_len:SIZE};let mut control=[0usize;4];let mut message:libc::msghdr=unsafe{std::mem::zeroed()};message.msg_iov=&mut vector;message.msg_iovlen=1;message.msg_control=control.as_mut_ptr().cast();message.msg_controllen=std::mem::size_of_val(&control)as _;
    let received=unsafe{libc::recvmsg(self.stream.as_raw_fd(),&mut message,libc::MSG_DONTWAIT)};if received<0{return Err(io::Error::last_os_error());}if received==0{return Err(error(libc::EPIPE));}
    let mut descriptors=Vec::new();unsafe{let mut h=libc::CMSG_FIRSTHDR(&message);while !h.is_null(){if (*h).cmsg_level==libc::SOL_SOCKET&&(*h).cmsg_type==libc::SCM_RIGHTS{if ((*h).cmsg_len as usize)<libc::CMSG_LEN(0)as usize{return Err(error(libc::EPROTO));}let n=((*h).cmsg_len as usize-libc::CMSG_LEN(0)as usize)/std::mem::size_of::<i32>();for i in 0..n{let fd=std::ptr::read_unaligned(libc::CMSG_DATA(h).cast::<i32>().add(i));descriptors.push(OwnedFd::from_raw_fd(fd));}}h=libc::CMSG_NXTHDR(&message,h);}}
    if message.msg_flags&(libc::MSG_CTRUNC|libc::MSG_TRUNC)!=0||descriptors.len()>1{return Err(error(libc::EPROTO));}Ok((received as usize,descriptors))
   });
   match result{Err(failure)if failure.kind()==io::ErrorKind::WouldBlock=>continue,other=>break other?}
  };
  if received<SIZE{if self.borrowed().read_exact(&mut bytes[received..]).is_err(){return Err(error(libc::EPROTO));}}
  let frame=Frame::decode(bytes)?;if !self.peer.is_live(){return Err(error(libc::ESRCH));}Ok((frame,descriptors.pop()))
 }
}

use std::collections::{BTreeMap,BTreeSet};
use std::sync::{Arc,Condvar,Mutex};
#[derive(Default)]
struct Inode {generation:u64,active:Option<u64>,members:BTreeMap<u64,usize>,admissions:usize,poisoned:bool}
#[derive(Default)]
struct Registry {next:u64,inodes:BTreeMap<[u8;36],Inode>,processes:BTreeMap<u64,ProcessIdentity>}
/// One native instance owns mapping admission and every live process generation.
#[derive(Default)]
pub struct Coordinator {state:Mutex<Registry>,changed:Condvar}
impl Coordinator {
 pub fn register(&self,process:ProcessIdentity)->io::Result<u64>{if !process.is_live(){return Err(error(libc::ESRCH));}let mut state=self.state.lock().unwrap();state.next=state.next.checked_add(1).ok_or_else(||error(libc::EOVERFLOW))?;let id=state.next;state.processes.insert(id,process);Ok(id)}
 pub fn admission(self:&Arc<Self>,process:u64,identity:[u8;36])->io::Result<Admission>{let mut state=self.state.lock().unwrap();loop{if !state.processes.get(&process).is_some_and(|p|p.is_live()){return Err(error(libc::ESRCH));}let inode=state.inodes.entry(identity).or_default();if inode.poisoned{return Err(error(libc::EIO));}if inode.active.is_none(){inode.admissions+=1;return Ok(Admission{owner:self.clone(),process,identity,published:false});}let(next,result)=self.changed.wait_timeout(state,Duration::from_secs(5)).unwrap();state=next;if result.timed_out(){return Err(error(libc::ETIMEDOUT));}}}
 pub fn unmap(&self,process:u64,identity:[u8;36])->io::Result<()>{let mut state=self.state.lock().unwrap();let inode=state.inodes.get_mut(&identity).ok_or_else(||error(libc::ENOENT))?;if inode.members.remove(&process).is_none(){return Err(error(libc::ENOENT));}self.changed.notify_all();Ok(())}
 pub fn reserve(self:&Arc<Self>,identity:[u8;36])->io::Result<Reservation>{let mut state=self.state.lock().unwrap();loop{let inode=state.inodes.entry(identity).or_default();if inode.poisoned{return Err(error(libc::EIO));}if inode.active.is_none()&&inode.admissions==0{break;}let(next,result)=self.changed.wait_timeout(state,Duration::from_secs(5)).unwrap();state=next;if result.timed_out(){return Err(error(libc::ETIMEDOUT));}}state.next=state.next.checked_add(1).ok_or_else(||error(libc::EOVERFLOW))?;let transaction=state.next;let members:Vec<u64>=state.inodes[&identity].members.keys().copied().collect();let mut peers=BTreeMap::new();for member in members{let process=state.processes[&member];if process.is_live(){peers.insert(member,process);}else{state.inodes.get_mut(&identity).unwrap().members.remove(&member);state.processes.remove(&member);}}state.inodes.get_mut(&identity).unwrap().active=Some(transaction);Ok(Reservation{owner:self.clone(),identity,transaction,peers,prepared:BTreeSet::new(),durable:false,complete:false})}
}
pub struct Admission {owner:Arc<Coordinator>,process:u64,identity:[u8;36],published:bool}
impl Admission {pub fn published(mut self)->io::Result<()>{let mut state=self.owner.state.lock().unwrap();if !state.processes.get(&self.process).is_some_and(|p|p.is_live()){return Err(error(libc::ESRCH));}let inode=state.inodes.get_mut(&self.identity).unwrap();inode.members.insert(self.process,1);inode.admissions-=1;self.published=true;self.owner.changed.notify_all();Ok(())}}
impl Drop for Admission {fn drop(&mut self){if !self.published{let mut state=self.owner.state.lock().unwrap();state.inodes.get_mut(&self.identity).unwrap().admissions-=1;self.owner.changed.notify_all();}}}
pub struct Reservation {owner:Arc<Coordinator>,pub identity:[u8;36],pub transaction:u64,pub peers:BTreeMap<u64,ProcessIdentity>,prepared:BTreeSet<u64>,durable:bool,complete:bool}
impl Reservation {
 pub fn prepared(&mut self,member:u64,process:ProcessIdentity)->io::Result<()>{if self.peers.get(&member)!=Some(&process)||!process.is_live(){return Err(error(libc::EPERM));}self.prepared.insert(member);Ok(())}
 pub fn ready(&self)->bool{self.peers.keys().all(|member|self.prepared.contains(member))}
 /// The actual storage owner calls this only after its durable atomic commit.
 pub fn published(&mut self)->io::Result<()>{if !self.ready(){return Err(error(libc::EBUSY));}self.durable=true;Ok(())}
 pub fn finish(mut self,acknowledged:&BTreeSet<u64>)->io::Result<()>{if acknowledged!=&self.prepared{return Err(error(libc::EIO));}let mut state=self.owner.state.lock().unwrap();let inode=state.inodes.get_mut(&self.identity).unwrap();if self.durable{inode.generation=inode.generation.checked_add(1).ok_or_else(||error(libc::EOVERFLOW))?;}inode.active=None;self.complete=true;self.owner.changed.notify_all();Ok(())}
}
impl Drop for Reservation {fn drop(&mut self){if !self.complete{let mut state=self.owner.state.lock().unwrap();let inode=state.inodes.get_mut(&self.identity).unwrap();if self.durable||!self.prepared.is_empty(){inode.poisoned=true;}inode.active=None;self.owner.changed.notify_all();}}}

/// Native listener authenticates the endpoint before accepting a registration.
pub struct Server {store:Mutex<Option<Arc<crate::fsverity::Store>>>,admission_listener:std::os::unix::net::UnixListener,listener:std::os::unix::net::UnixListener,pub coordinator:Arc<Coordinator>,channels:Mutex<BTreeMap<u64,Arc<Mutex<Channel>>>>,timeout:Duration,active_admissions:std::sync::atomic::AtomicUsize}
impl Server {
 pub fn bind(path:&std::path::Path,timeout:Duration)->io::Result<Self>{use std::os::unix::fs::{MetadataExt,PermissionsExt};let parent=path.parent().ok_or_else(||error(libc::EINVAL))?;std::fs::create_dir_all(parent)?;let metadata=std::fs::symlink_metadata(parent)?;if !metadata.is_dir()||metadata.uid()!=unsafe{libc::geteuid()}{return Err(error(libc::EPERM));}std::fs::set_permissions(parent,std::fs::Permissions::from_mode(0o700))?;let listener=std::os::unix::net::UnixListener::bind(path)?;let admission_path=path.with_extension("admission");let admission_listener=std::os::unix::net::UnixListener::bind(&admission_path)?;std::fs::set_permissions(&admission_path,std::fs::Permissions::from_mode(0o600))?;std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o600))?;Ok(Self{store:Mutex::new(None),admission_listener,listener,coordinator:Arc::new(Coordinator::default()),channels:Mutex::new(BTreeMap::new()),timeout,active_admissions:std::sync::atomic::AtomicUsize::new(0)})}
 pub fn accept(&self)->io::Result<(u64,ProcessIdentity)>{let(stream,_)=self.listener.accept()?;let channel=Channel::new(stream,None,self.timeout)?;let process=channel.peer;let member=self.coordinator.register(process)?;let mut channel=channel;channel.send(&Frame{operation:Operation::Register,transaction:member,identity:[0;36],generation:0,error:0},None)?;self.channels.lock().unwrap().insert(member,Arc::new(Mutex::new(channel)));Ok((member,process))}
 pub fn prepare(&self,identity:[u8;36],proof:&PrivateFd)->io::Result<Transition>{let mut reservation=self.coordinator.reserve(identity)?;let channels=self.channels.lock().unwrap();let mut participants=Vec::new();for (&member,&process) in &reservation.peers{let channel=channels.get(&member).ok_or_else(||error(libc::ENOTCONN))?.clone();participants.push((member,process,channel));}drop(channels);
  for (member,process,channel) in &participants{reservation.prepared.insert(*member);let result=(||{let mut peer=channel.lock().unwrap();peer.send(&Frame{operation:Operation::Prepare,transaction:reservation.transaction,identity,generation:0,error:0},Some(proof))?;let(ack,fd)=peer.receive()?;if fd.is_some()||ack.operation!=Operation::Prepared||ack.transaction!=reservation.transaction||ack.identity!=identity{return Err(error(if ack.error!=0{ack.error}else{libc::EPROTO}));}reservation.prepared(*member,*process)})();if let Err(failure)=result{let transition=Transition{reservation:Some(reservation),participants};if let Err(abort)=transition.abort(){return Err(abort);}return Err(failure);}}
  Ok(Transition{reservation:Some(reservation),participants})
 }
}
pub struct Transition {reservation:Option<Reservation>,participants:Vec<(u64,ProcessIdentity,Arc<Mutex<Channel>>)>}
impl Transition {
 pub fn ready(&self)->bool{self.reservation.as_ref().is_some_and(Reservation::ready)}
 pub fn published(&mut self)->io::Result<()>{self.reservation.as_mut().ok_or_else(||error(libc::EINVAL))?.published()}
 fn finish(mut self,operation:Operation)->io::Result<()>{let reservation=self.reservation.as_ref().ok_or_else(||error(libc::EINVAL))?;let mut acknowledged=BTreeSet::new();let mut failure=None;for(member,process,channel)in &self.participants{if !reservation.prepared.contains(member){continue;}if !process.is_live(){acknowledged.insert(*member);continue;}let result=(||{let mut peer=channel.lock().unwrap();peer.send(&Frame{operation,transaction:reservation.transaction,identity:reservation.identity,generation:0,error:0},None)?;let(ack,proof)=peer.receive()?;let expected=if operation==Operation::Commit{Operation::Committed}else{Operation::Aborted};if proof.is_some()||ack.operation!=expected||ack.transaction!=reservation.transaction||ack.identity!=reservation.identity{return Err(error(if ack.error!=0{ack.error}else{libc::EPROTO}));}Ok(())})();match result{Ok(())=>{acknowledged.insert(*member);},Err(error)=>{failure=Some(error);}}}if let Some(error)=failure{return Err(error);}self.reservation.take().unwrap().finish(&acknowledged)}
 pub fn commit(self)->io::Result<()>{if !self.reservation.as_ref().is_some_and(|r|r.durable){return Err(error(libc::EINVAL));}self.finish(Operation::Commit)}
 pub fn abort(self)->io::Result<()>{if self.reservation.as_ref().is_some_and(|r|r.durable){return Err(error(libc::EPERM));}self.finish(Operation::Abort)}
}
/// A process connects using an explicit native instance endpoint and expected
/// init incarnation. Its dedicated worker consumes this authenticated channel.
pub fn connect(path:&std::path::Path,init:ProcessIdentity,timeout:Duration)->io::Result<Channel>{use std::os::unix::fs::MetadataExt;let parent=path.parent().ok_or_else(||error(libc::EINVAL))?;let directory=std::fs::symlink_metadata(parent)?;if !directory.is_dir()||directory.mode()&0o777!=0o700||directory.uid()!=unsafe{libc::geteuid()}{return Err(error(libc::EPERM));}let socket=std::fs::symlink_metadata(path)?;if socket.mode()&0o777!=0o600||socket.uid()!=unsafe{libc::geteuid()}{return Err(error(libc::EPERM));}{
 use std::os::unix::ffi::OsStrExt;let bytes=path.as_os_str().as_bytes();let mut address:libc::sockaddr_un=unsafe{std::mem::zeroed()};if bytes.len()>=address.sun_path.len(){return Err(error(libc::ENAMETOOLONG));}address.sun_family=libc::AF_UNIX as _;address.sun_len=std::mem::size_of::<libc::sockaddr_un>()as _;for (slot,byte)in address.sun_path.iter_mut().zip(bytes){*slot=*byte as _;}
 let descriptor=PrivateFd::allocate(||{let fd=unsafe{libc::socket(libc::AF_UNIX,libc::SOCK_STREAM,0)};if fd<0{return Err(io::Error::last_os_error());}let owned=unsafe{OwnedFd::from_raw_fd(fd)};if unsafe{libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC)}<0{return Err(io::Error::last_os_error());}if unsafe{libc::fcntl(fd,libc::F_SETFL,libc::O_NONBLOCK)}<0{return Err(io::Error::last_os_error());}if unsafe{libc::connect(fd,(&address as*const libc::sockaddr_un).cast(),std::mem::size_of_val(&address)as _)}<0{let failure=io::Error::last_os_error();if !matches!(failure.raw_os_error(),Some(libc::EINPROGRESS)|Some(libc::EAGAIN)){return Err(failure);}}Ok(owned)})?;
 let mut poll=libc::pollfd{fd:descriptor.as_raw_fd(),events:libc::POLLOUT,revents:0};let result=unsafe{libc::poll(&mut poll,1,timeout.as_millis().min(i32::MAX as u128)as i32)};if result==0{return Err(error(libc::ETIMEDOUT));}if result<0{return Err(io::Error::last_os_error());}let mut status=0i32;let mut length=std::mem::size_of::<i32>()as libc::socklen_t;if unsafe{libc::getsockopt(descriptor.as_raw_fd(),libc::SOL_SOCKET,libc::SO_ERROR,(&mut status as*mut i32).cast(),&mut length)}<0{return Err(io::Error::last_os_error());}if status!=0{return Err(error(status));}if unsafe{libc::fcntl(descriptor.as_raw_fd(),libc::F_SETFL,0)}<0{return Err(io::Error::last_os_error());}Channel::from_private(descriptor,Some(init),timeout)
 }}
impl Server {
 pub fn serve_admissions(self:Arc<Self>)->io::Result<()> {for stream in self.admission_listener.incoming(){let stream=stream?;if self.active_admissions.fetch_add(1,std::sync::atomic::Ordering::AcqRel)>=256{self.active_admissions.fetch_sub(1,std::sync::atomic::Ordering::AcqRel);drop(stream);continue;}let owner=self.clone();std::thread::Builder::new().name("verity-admission".into()).spawn(move||{match Channel::new(stream,None,owner.timeout).and_then(|channel|owner.admission_request(channel)){Ok(())=>{},Err(failure)=>eprintln!("verity admission failed: {failure}")}owner.active_admissions.fetch_sub(1,std::sync::atomic::Ordering::AcqRel);})?;}Ok(())}
 pub fn accept_admission(&self)->io::Result<()>{let(stream,_)=self.admission_listener.accept()?;self.admission_request(Channel::new(stream,None,self.timeout)?)}
 pub fn admission_request(&self,mut channel:Channel)->io::Result<()> {
  let(frame,proof)=channel.receive()?;if proof.is_some(){return Err(error(libc::EPROTO));}
  {let state=self.coordinator.state.lock().unwrap();if state.processes.get(&frame.transaction)!=Some(&channel.peer){return Err(error(libc::EPERM));}}
  if frame.operation==Operation::ForkBegin{return self.fork_request(channel,frame);}
  if frame.operation==Operation::Unmap{self.coordinator.unmap(frame.transaction,frame.identity)?;return channel.send(&frame,None);}
  if frame.operation!=Operation::MapBegin{return Err(error(libc::EPROTO));}
  let guard=match self.coordinator.admission(frame.transaction,frame.identity){Ok(guard)=>guard,Err(failure)=>{channel.send(&Frame{operation:Operation::Error,error:failure.raw_os_error().unwrap_or(libc::EIO),..frame},None)?;return Err(failure);}};
  let generation=self.coordinator.state.lock().unwrap().inodes[&frame.identity].generation;
  channel.send(&Frame{generation,..frame},None)?;
  let(done,proof)=channel.receive()?;
  if proof.is_some()||done.transaction!=frame.transaction||done.identity!=frame.identity{return Err(error(libc::EPROTO));}
  match done.operation{Operation::MapLive=>guard.published()?,Operation::MapAbort=>drop(guard),Operation::ForkHandoff=>{let state=self.coordinator.state.lock().unwrap();let process=state.processes.get(&done.generation).ok_or_else(||error(libc::ESRCH))?;let mut info:libc::proc_bsdinfo=unsafe{std::mem::zeroed()};let size=std::mem::size_of_val(&info)as i32;if unsafe{libc::proc_pidinfo(process.host_pid,libc::PROC_PIDTBSDINFO,1,(&mut info as*mut libc::proc_bsdinfo).cast(),size)}!=size||info.pbi_ppid as i32!=channel.peer.host_pid||!process.is_live()||!state.inodes[&frame.identity].members.contains_key(&done.generation){return Err(error(libc::EPERM));}drop(state);drop(guard);},_=>return Err(error(libc::EPROTO))}
  channel.send(&done,None)
 }
}
pub struct Client {path:std::path::PathBuf,init:ProcessIdentity,pub member:u64,timeout:Duration}
pub struct RemoteAdmission {channel:Channel,frame:Frame}
impl RemoteAdmission {pub fn generation(&self)->u64{self.frame.generation}}
impl RemoteAdmission {
 pub fn handoff(mut self,child_member:u64)->io::Result<()>{self.frame.operation=Operation::ForkHandoff;self.frame.generation=child_member;self.channel.send(&self.frame,None)?;let(reply,proof)=self.channel.receive()?;if proof.is_some()||reply!=self.frame{return Err(error(libc::EPROTO));}Ok(())}
 pub fn published(mut self)->io::Result<()>{self.frame.operation=Operation::MapLive;self.channel.send(&self.frame,None)?;let(reply,proof)=self.channel.receive()?;if proof.is_some()||reply!=self.frame{return Err(error(libc::EPROTO));}Ok(())}
 pub fn abort(mut self)->io::Result<()>{self.frame.operation=Operation::MapAbort;self.channel.send(&self.frame,None)?;let(reply,proof)=self.channel.receive()?;if proof.is_some()||reply!=self.frame{return Err(error(libc::EPROTO));}Ok(())}
}
impl Client {
 pub fn attach(path:&std::path::Path,init:ProcessIdentity,timeout:Duration)->io::Result<(Self,Channel)>{let mut channel=connect(path,init,timeout)?;let(frame,proof)=channel.receive()?;if frame.operation!=Operation::Register||proof.is_some()||frame.transaction==0{return Err(error(libc::EPROTO));}Ok((Self{path:path.with_extension("admission"),init,member:frame.transaction,timeout},channel))}
 pub fn admission(&self,identity:[u8;36])->io::Result<RemoteAdmission>{let mut channel=connect(&self.path,self.init,self.timeout)?;let frame=Frame{operation:Operation::MapBegin,identity,transaction:self.member,generation:0,error:0};channel.send(&frame,None)?;let(reply,proof)=channel.receive()?;if reply.operation==Operation::Error{return Err(error(reply.error));}if proof.is_some()||reply.operation!=frame.operation||reply.transaction!=frame.transaction||reply.identity!=frame.identity{return Err(error(libc::EPROTO));}Ok(RemoteAdmission{channel,frame:reply})}
 pub fn unmap(&self,identity:[u8;36])->io::Result<()>{let mut channel=connect(&self.path,self.init,self.timeout)?;let frame=Frame{operation:Operation::Unmap,identity,transaction:self.member,generation:0,error:0};channel.send(&frame,None)?;let(reply,proof)=channel.receive()?;if proof.is_some()||reply!=frame{return Err(error(libc::EPROTO));}Ok(())}
}
#[cfg(test)]
mod tests {
 use super::*;use std::os::unix::net::UnixListener;use std::process::{Command,Stdio};
 #[test]
 fn disconnected_accepted_peer_is_rejected_before_socket_configuration(){
  let root=std::env::temp_dir().join(format!("aim-control-disconnected-{}",std::process::id()));std::fs::create_dir(&root).unwrap();
  let listener=UnixListener::bind(root.join("ctl")).unwrap();let client=UnixStream::connect(root.join("ctl")).unwrap();let(stream,_)=listener.accept().unwrap();drop(client);
  assert_eq!(stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap_err().raw_os_error(),Some(libc::EINVAL));
  let failure=match Channel::new(stream,None,Duration::from_secs(2)){Err(failure)=>failure,Ok(_)=>panic!("disconnected peer authenticated")};
  assert_eq!(failure.raw_os_error(),Some(libc::ENOTCONN));drop(listener);std::fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn bound_owner_without_listener_threads_still_retires_owned_sockets(){
  use std::os::unix::fs::MetadataExt;
  let directory=EndpointDirectory::new().unwrap();let path=directory.path.join("ctl");
  let server=Arc::new(Server::bind(&path,Duration::from_secs(1)).unwrap());
  let paths=[path.clone(),path.with_extension("admission")];let mut inodes=[(0,0);2];
  for(index,path)in paths.iter().enumerate(){let stat=std::fs::symlink_metadata(path).unwrap();inodes[index]=(stat.dev(),stat.ino());}
  let root=directory.path.clone();let owner=RunningServer{server,process:ProcessIdentity::running(std::process::id()as i32).unwrap(),path,inodes,enable_inode:None,owned_directory:Some(directory),cleaned:false,stop:Arc::new(std::sync::atomic::AtomicBool::new(false)),threads:vec![]};
  owner.shutdown().unwrap();assert!(!root.exists());
 }
 #[test]
 fn owned_short_endpoints_support_long_runtime_and_cleanup_all_three_sockets(){
  use std::os::unix::{ffi::OsStrExt,fs::PermissionsExt};
  let root=std::env::temp_dir().join(format!("aim-verity-long-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())).join("long-native-runtime-component".repeat(8));
  std::fs::create_dir_all(&root).unwrap();assert!(root.as_os_str().as_bytes().len()>104);
  std::fs::write(root.join("data"),vec![37;4096]).unwrap();let data=std::fs::File::open(root.join("data")).unwrap();
  let store=Arc::new(crate::fsverity::Store::new(&root.join("proof"),&root.join("leases")).unwrap());
  let owner=RunningServer::start_private_with_store(Duration::from_secs(2),store.clone()).unwrap();
  let endpoint=owner.endpoint().to_path_buf();let directory=endpoint.parent().unwrap().to_path_buf();
  assert!(!endpoint.starts_with(&root));assert_eq!(std::fs::metadata(&directory).unwrap().permissions().mode()&0o777,0o700);
  for path in [endpoint.clone(),endpoint.with_extension("admission"),endpoint.with_extension("enable")]{assert!(path.as_os_str().as_bytes().len()<104);assert!(path.exists());}
  let config=OwnerConfig{endpoint:endpoint.clone(),process:owner.process};config.write(&root.join("owner")).unwrap();
  let loaded=OwnerConfig::read(&root.join("owner")).unwrap();assert_eq!(loaded.endpoint,endpoint);assert_eq!(loaded.process,owner.process);
  let(client,_control)=Client::attach(&loaded.endpoint,loaded.process,Duration::from_secs(2)).unwrap();
  client.admission([7;36]).unwrap().published().unwrap();client.unmap([7;36]).unwrap();
  let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let blob=guard.build(crate::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let view=blob.metadata_view().unwrap();
  let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();
  let proof=PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(view.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let transition=client.enable_transition(identity.to_bytes(),&proof).unwrap();let committed=guard.commit(blob).unwrap();transition.published(&committed).unwrap();assert!(store.lookup(identity).unwrap().is_some());
  owner.shutdown().unwrap();assert!(!directory.exists());
  let second=RunningServer::start_private_with_store(Duration::from_secs(2),store).unwrap();let second_directory=second.endpoint().parent().unwrap().to_path_buf();assert_ne!(directory,second_directory);drop(second);assert!(!second_directory.exists());
  std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
 }
 #[test]
 #[ignore="owned subprocess helper invoked by authenticated_cross_process_frames"]
 fn control_child(){let Some(path)=std::env::args().find_map(|a|a.strip_prefix("--control-endpoint=").map(String::from))else{return;};let mut channel=Channel::new(UnixStream::connect(path).unwrap(),None,Duration::from_secs(2)).unwrap();let(frame,proof)=channel.receive().unwrap();assert!(proof.is_some());channel.send(&Frame{operation:Operation::Prepared,..frame},None).unwrap();}
 #[test]
 #[ignore="owned subprocess for authenticated_fork_handoff"]
 fn fork_child(){let Some(root)=std::env::args().find_map(|a|a.strip_prefix("--fork-owner=").map(std::path::PathBuf::from))else{return;};let config=OwnerConfig::read(&root.join("owner")).unwrap();let(client,_channel)=Client::attach(&config.endpoint,config.process,Duration::from_secs(2)).unwrap();let data=std::fs::File::open(root.join("data")).unwrap();let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();let partial=std::env::args().any(|arg|arg=="--partial-fork-batch");let batch=partial||std::env::args().any(|arg|arg=="--full-fork-batch");if batch{for index in 0..if partial{256}else{257}{let file=std::fs::File::open(root.join(format!("inode-{index}"))).unwrap();let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();client.admission(identity.to_bytes()).unwrap().published().unwrap();}}else{client.admission(identity.to_bytes()).unwrap().published().unwrap();}println!("{}",client.member);let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();}
 #[test]
 fn authenticated_fork_handoff_requires_actual_live_child_membership(){use std::io::BufRead;let root=std::env::temp_dir().join(format!("aim-control-fork-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let endpoint=root.join("ctl");let owner=RunningServer::start(&endpoint,Duration::from_secs(2)).unwrap();OwnerConfig{endpoint:std::fs::canonicalize(&endpoint).unwrap(),process:owner.process}.write(&root.join("owner")).unwrap();let(parent,_channel)=Client::attach(&endpoint,owner.process,Duration::from_secs(2)).unwrap();std::fs::write(root.join("data"),b"actual fork inode").unwrap();let data=std::fs::File::open(root.join("data")).unwrap();let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();let lease=parent.fork_lease([identity.to_bytes()]).unwrap();let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","verity_control::tests::fork_child","--ignored","--nocapture","--skip",&format!("--fork-owner={}",root.display())]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();struct ChildGuard(std::process::Child);impl Drop for ChildGuard{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();let _=self.0.wait();}}}let mut child=ChildGuard(child);let mut reader=std::io::BufReader::new(child.0.stdout.take().unwrap());let member=loop{let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0);if let Ok(member)=line.trim().parse::<u64>(){break member;}};lease.child_published(member).unwrap();child.0.stdin.take().unwrap().write_all(b"done\n").unwrap();assert!(child.0.wait().unwrap().success());owner.shutdown().unwrap();std::fs::remove_dir_all(root).unwrap();}
 #[test]
 fn remote_issuer_requires_actual_store_publication_receipt(){let root=std::env::temp_dir().join(format!("aim-control-issuer-{}",std::process::id()));std::fs::create_dir(&root).unwrap();std::fs::write(root.join("data"),vec![37;4096]).unwrap();let data=std::fs::File::open(root.join("data")).unwrap();let store=Arc::new(crate::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap());let owner=RunningServer::start_with_store(&root.join("ctl"),Duration::from_secs(2),store.clone()).unwrap();let(client,_control)=Client::attach(&root.join("ctl"),owner.process,Duration::from_secs(2)).unwrap();let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);let blob=guard.build(crate::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let view=blob.metadata_view().unwrap();let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();let proof=PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(view.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}}).unwrap();let invalid=client.enable_transition(identity.to_bytes(),&proof).unwrap();assert!(invalid.published(&view).is_err());assert!(store.lookup(identity).unwrap().is_none());let transition=client.enable_transition(identity.to_bytes(),&proof).unwrap();assert!(store.lookup(identity).unwrap().is_none());let committed=guard.commit(blob).unwrap();transition.published(&committed).unwrap();assert!(store.lookup(identity).unwrap().is_some());owner.shutdown().unwrap();std::fs::remove_dir_all(root).unwrap();}
 #[test]
 fn batched_fork_handoff_checks_all_257_actual_child_memberships(){use std::io::BufRead;let root=std::env::temp_dir().join(format!("aim-batch-handoff-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let endpoint=root.join("ctl");let owner=RunningServer::start(&endpoint,Duration::from_secs(5)).unwrap();OwnerConfig{endpoint:std::fs::canonicalize(&endpoint).unwrap(),process:owner.process}.write(&root.join("owner")).unwrap();let(parent,_channel)=Client::attach(&endpoint,owner.process,Duration::from_secs(5)).unwrap();std::fs::write(root.join("data"),b"actual fork inode").unwrap();let data=std::fs::File::open(root.join("data")).unwrap();let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();let mut identities=Vec::new();for index in 0..257{let path=root.join(format!("inode-{index}"));std::fs::write(&path,b"actual child mapped inode").unwrap();let file=std::fs::File::open(path).unwrap();identities.push(crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap().to_bytes());}let lease=parent.fork_lease(identities).unwrap();let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","verity_control::tests::fork_child","--ignored","--nocapture","--skip",&format!("--fork-owner={}",root.display()),"--skip","--full-fork-batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();struct ChildGuard(std::process::Child);impl Drop for ChildGuard{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();let _=self.0.wait();}}}let mut child=ChildGuard(child);let mut reader=std::io::BufReader::new(child.0.stdout.take().unwrap());let member=loop{let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0);if let Ok(member)=line.trim().parse::<u64>(){break member;}};lease.child_published(member).unwrap();child.0.stdin.take().unwrap().write_all(b"done\n").unwrap();assert!(child.0.wait().unwrap().success());owner.shutdown().unwrap();std::fs::remove_dir_all(root).unwrap();}

 #[test]
 fn batched_fork_handoff_rejects_child_with_only_256_memberships(){use std::io::BufRead;let root=std::env::temp_dir().join(format!("aim-batch-partial-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let endpoint=root.join("ctl");let owner=RunningServer::start(&endpoint,Duration::from_secs(5)).unwrap();OwnerConfig{endpoint:std::fs::canonicalize(&endpoint).unwrap(),process:owner.process}.write(&root.join("owner")).unwrap();let(parent,_channel)=Client::attach(&endpoint,owner.process,Duration::from_secs(5)).unwrap();std::fs::write(root.join("data"),b"actual fork inode").unwrap();let data=std::fs::File::open(root.join("data")).unwrap();let identity=crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&data)).unwrap();let mut identities=Vec::new();for index in 0..257{let path=root.join(format!("inode-{index}"));std::fs::write(&path,b"actual child mapped inode").unwrap();let file=std::fs::File::open(path).unwrap();identities.push(crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap().to_bytes());}let lease=parent.fork_lease(identities).unwrap();let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","verity_control::tests::fork_child","--ignored","--nocapture","--skip",&format!("--fork-owner={}",root.display()),"--skip","--partial-fork-batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();struct ChildGuard(std::process::Child);impl Drop for ChildGuard{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();let _=self.0.wait();}}}let mut child=ChildGuard(child);let mut reader=std::io::BufReader::new(child.0.stdout.take().unwrap());let member=loop{let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0);if let Ok(member)=line.trim().parse::<u64>(){break member;}};assert!(lease.child_published(member).is_err());std::thread::sleep(Duration::from_millis(20));child.0.stdin.take().unwrap().write_all(b"done\n").unwrap();assert!(child.0.wait().unwrap().success());owner.shutdown().unwrap();std::fs::remove_dir_all(root).unwrap();}

 #[test]
 fn batched_fork_admits_257_real_inodes_atomically_and_rejects_duplicates(){let root=std::env::temp_dir().join(format!("aim-fork-batch-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let endpoint=root.join("ctl");let owner=RunningServer::start(&endpoint,Duration::from_secs(2)).unwrap();let(client,_control)=Client::attach(&endpoint,owner.process,Duration::from_secs(2)).unwrap();let mut identities=Vec::new();for i in 0..257{let path=root.join(format!("inode-{i}"));std::fs::write(&path,b"actual mapped inode").unwrap();let file=std::fs::File::open(path).unwrap();identities.push(crate::fsverity::Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap().to_bytes());}let lease=client.fork_lease(identities.clone()).unwrap();{let state=owner.server.coordinator.state.lock().unwrap();assert!(identities.iter().all(|identity|state.inodes[identity].admissions==1));}lease.abort().unwrap();{let state=owner.server.coordinator.state.lock().unwrap();assert!(identities.iter().all(|identity|state.inodes[identity].admissions==0));}
  let mut malformed=connect(&endpoint.with_extension("admission"),owner.process,Duration::from_secs(2)).unwrap();let frame=Frame{operation:Operation::ForkBegin,transaction:client.member,identity:[0;36],generation:2,error:0};malformed.send(&frame,None).unwrap();for _ in 0..2{malformed.send(&Frame{operation:Operation::ForkIdentity,identity:identities[0],..frame.clone()},None).unwrap();}assert!(malformed.receive().is_err());let lease=client.fork_lease(identities.clone()).unwrap();drop(lease);let deadline=std::time::Instant::now()+Duration::from_secs(1);loop{let done={let state=owner.server.coordinator.state.lock().unwrap();identities.iter().all(|identity|state.inodes[identity].admissions==0)};if done{break;}assert!(std::time::Instant::now()<deadline,"EOF did not release all fork admissions");std::thread::sleep(Duration::from_millis(5));}for identity in &identities{let reservation=owner.server.coordinator.reserve(*identity).unwrap();assert!(reservation.ready());reservation.finish(&BTreeSet::new()).unwrap();}
  for count in [65537,1]{let mut bad=connect(&endpoint.with_extension("admission"),owner.process,Duration::from_secs(2)).unwrap();let frame=Frame{operation:Operation::ForkBegin,transaction:client.member,identity:[0;36],generation:count,error:0};bad.send(&frame,None).unwrap();if count==1{let descriptor=PrivateFd::allocate(||Ok(std::fs::File::open(root.join("inode-0"))?.into())).unwrap();bad.send(&Frame{operation:Operation::ForkIdentity,identity:identities[0],..frame},Some(&descriptor)).unwrap();}assert!(bad.receive().is_err());}let lease=client.fork_lease(identities.clone()).unwrap();lease.abort().unwrap();std::thread::sleep(Duration::from_millis(20));owner.shutdown().unwrap();std::fs::remove_dir_all(root).unwrap();}
 #[test]
 fn repeated_live_membership_is_one_actual_owner_until_final_unmap(){let owner=Arc::new(Coordinator::default());let process=ProcessIdentity::running(std::process::id()as i32).unwrap();let member=owner.register(process).unwrap();let identity=[8;36];owner.admission(member,identity).unwrap().published().unwrap();owner.admission(member,identity).unwrap().published().unwrap();owner.unmap(member,identity).unwrap();let reservation=owner.reserve(identity).unwrap();assert!(reservation.peers.is_empty());reservation.finish(&BTreeSet::new()).unwrap();}
 #[test]
 fn actual_owner_config_binds_live_epoch_and_rejects_malformed_version(){let root=std::env::temp_dir().join(format!("aim-verity-config-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let endpoint=root.join("ctl");let owner=RunningServer::start(&endpoint,Duration::from_millis(100)).unwrap();let config=OwnerConfig{endpoint:std::fs::canonicalize(&endpoint).unwrap(),process:owner.process};let path=root.join("owner");config.write(&path).unwrap();let loaded=OwnerConfig::read(&path).unwrap();assert_eq!(loaded.process,owner.process);assert_eq!(loaded.endpoint,config.endpoint);std::fs::write(&path,b"AIMVERITY2\tinvalid\n").unwrap();assert_eq!(OwnerConfig::read(&path).err().unwrap().raw_os_error(),Some(libc::EINVAL));owner.shutdown().unwrap();std::fs::remove_file(path).unwrap();std::fs::remove_dir(root).unwrap();}
 #[test]
 fn authenticated_cross_process_frames(){let path=std::env::temp_dir().join(format!("aim-verity-control-{}",std::process::id()));let listener=UnixListener::bind(&path).unwrap();let child=Command::new(std::env::current_exe().unwrap()).args(["--exact","verity_control::tests::control_child","--ignored","--nocapture","--skip",&format!("--control-endpoint={}",path.display())]).stdout(Stdio::null()).spawn().unwrap();let(stream,_)=listener.accept().unwrap();let process=ProcessIdentity::running(child.id()as i32).unwrap();let mut channel=Channel::new(stream,Some(process),Duration::from_secs(2)).unwrap();let file=std::fs::File::open(std::env::current_exe().unwrap()).unwrap();let proof=PrivateFd::adopt(file.into()).unwrap();let frame=Frame{operation:Operation::Prepare,transaction:3,identity:[9;36],generation:0,error:0};channel.send(&frame,Some(&proof)).unwrap();let(reply,fd)=channel.receive().unwrap();assert_eq!(reply.operation,Operation::Prepared);assert!(fd.is_none());let mut child=child;assert!(child.wait().unwrap().success());assert!(!process.is_live());std::fs::remove_file(path).unwrap();}
 #[test]
 fn admission_rollback_and_postpublication_failure_are_owner_coherent(){let owner=Arc::new(Coordinator::default());let member=owner.register(ProcessIdentity::running(std::process::id()as i32).unwrap()).unwrap();let identity=[1;36];let admission=owner.admission(member,identity).unwrap();drop(admission);let empty=owner.reserve(identity).unwrap();assert!(empty.ready());empty.finish(&BTreeSet::new()).unwrap();owner.admission(member,identity).unwrap().published().unwrap();let mut reserved=owner.reserve(identity).unwrap();reserved.prepared(member,ProcessIdentity::running(std::process::id()as i32).unwrap()).unwrap();reserved.published().unwrap();drop(reserved);assert_eq!(owner.admission(member,identity).err().unwrap().raw_os_error(),Some(libc::EIO));}
}

/// Boot-owned native endpoint lifetime. Only its captured socket inodes may be
/// unlinked; a replacement endpoint or PID incarnation is never cleaned up.
// Native transport paths are independent of the guest's runtime path length.
struct EndpointDirectory {path:std::path::PathBuf,identity:Option<(u64,u64)>}
impl EndpointDirectory {
 fn new()->io::Result<Self>{use std::os::unix::{ffi::OsStrExt,fs::MetadataExt};
  let mut template=b"/tmp/av-XXXXXX\0".to_vec();
  if unsafe{libc::mkdtemp(template.as_mut_ptr().cast())}.is_null(){return Err(io::Error::last_os_error());}
  let raw=std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&template[..template.len()-1]));
  let mut owned=Self{path:raw,identity:None};
  let metadata=std::fs::symlink_metadata(&owned.path)?;
  owned.identity=Some((metadata.dev(),metadata.ino()));
  owned.path=std::fs::canonicalize(&owned.path)?;Ok(owned)
 }
 fn remove(&self)->io::Result<()>{use std::os::unix::fs::MetadataExt;
  let metadata=match std::fs::symlink_metadata(&self.path){Ok(metadata)=>metadata,Err(failure)if failure.kind()==io::ErrorKind::NotFound=>return Ok(()),Err(failure)=>return Err(failure)};
  if !metadata.is_dir()||self.identity.is_some_and(|identity|(metadata.dev(),metadata.ino())!=identity){return Err(error(libc::ESTALE));}
  std::fs::remove_dir(&self.path)
 }
}
impl Drop for EndpointDirectory{fn drop(&mut self){if let Err(failure)=self.remove(){eprintln!("verity endpoint directory cleanup: {failure}");}}}
pub struct RunningServer {pub server:Arc<Server>,pub process:ProcessIdentity,path:std::path::PathBuf,inodes:[(u64,u64);2],enable_inode:Option<(u64,u64)>,owned_directory:Option<EndpointDirectory>,cleaned:bool,stop:Arc<std::sync::atomic::AtomicBool>,threads:Vec<std::thread::JoinHandle<io::Result<()>>>}
impl RunningServer {
 pub fn start(path:&std::path::Path,timeout:Duration)->io::Result<Self>{use std::os::unix::fs::MetadataExt;let process=ProcessIdentity::running(std::process::id()as i32)?;let server=Arc::new(Server::bind(path,timeout)?);server.listener.set_nonblocking(true)?;server.admission_listener.set_nonblocking(true)?;let paths=[path.to_path_buf(),path.with_extension("admission")];let mut inodes=[(0,0);2];for(i,p)in paths.iter().enumerate(){let stat=std::fs::symlink_metadata(p)?;inodes[i]=(stat.dev(),stat.ino());}let stop=Arc::new(std::sync::atomic::AtomicBool::new(false));let mut running=Self{server:server.clone(),process,path:path.into(),inodes,enable_inode:None,owned_directory:None,cleaned:false,stop:stop.clone(),threads:Vec::new()};
  for admission in [false,true]{let owner=server.clone();let stopped=stop.clone();running.threads.push(std::thread::Builder::new().name(if admission{"verity-admission-listener"}else{"verity-control-listener"}.into()).spawn(move||{let mut workers:Vec<std::thread::JoinHandle<io::Result<()>>>=Vec::new();while !stopped.load(std::sync::atomic::Ordering::Acquire){for index in (0..workers.len()).rev(){if workers[index].is_finished(){match workers.swap_remove(index).join(){Ok(Ok(()))=>{},Ok(Err(failure))if matches!(failure.raw_os_error(),Some(libc::EPIPE)|Some(libc::ECONNRESET))=>{},Ok(Err(failure))=>eprintln!("verity admission worker failed: {failure}"),Err(_)=>return Err(error(libc::EIO))}}}let listener=if admission{&owner.admission_listener}else{&owner.listener};match listener.accept(){Ok((stream,_))=>{if workers.len()>=256{drop(stream);continue;}let current=owner.clone();workers.push(std::thread::spawn(move||{let channel=Channel::new(stream,None,current.timeout)?;if admission{current.admission_request(channel)}else{let process=channel.peer;let member=current.coordinator.register(process)?;let mut channel=channel;channel.send(&Frame{operation:Operation::Register,transaction:member,identity:[0;36],generation:0,error:0},None)?;current.channels.lock().unwrap().insert(member,Arc::new(Mutex::new(channel)));Ok(())}}));},Err(failure)if failure.kind()==io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(5)),Err(failure)=>return Err(failure)}}for worker in workers{match worker.join(){Ok(Ok(()))=>{},Ok(Err(failure))if matches!(failure.raw_os_error(),Some(libc::EPIPE)|Some(libc::ECONNRESET))=>{},Ok(Err(failure))=>return Err(failure),Err(_)=>return Err(error(libc::EIO))}}Ok(())})?);}
  Ok(running)
 }
 pub fn start_with_store(path:&std::path::Path,timeout:Duration,store:Arc<crate::fsverity::Store>)->io::Result<Self>{use std::os::unix::fs::{MetadataExt,PermissionsExt};let mut running=Self::start(path,timeout)?;*running.server.store.lock().unwrap()=Some(store);let endpoint=path.with_extension("enable");let listener=std::os::unix::net::UnixListener::bind(&endpoint)?;std::fs::set_permissions(&endpoint,std::fs::Permissions::from_mode(0o600))?;listener.set_nonblocking(true)?;let stat=std::fs::symlink_metadata(&endpoint)?;running.enable_inode=Some((stat.dev(),stat.ino()));let owner=running.server.clone();let stopped=running.stop.clone();running.threads.push(std::thread::Builder::new().name("verity-enable-listener".into()).spawn(move||{let mut workers:Vec<std::thread::JoinHandle<io::Result<()>>>=Vec::new();while !stopped.load(std::sync::atomic::Ordering::Acquire){for index in (0..workers.len()).rev(){if workers[index].is_finished(){match workers.swap_remove(index).join(){Ok(Ok(()))=>{},Ok(Err(failure))=>eprintln!("verity enable request failed: {failure}"),Err(_)=>return Err(error(libc::EIO))}}}match listener.accept(){Ok((stream,_))=>{if workers.len()>=32{drop(stream);continue;}let current=owner.clone();workers.push(std::thread::spawn(move||current.enable_request(Channel::new(stream,None,current.timeout)?)));},Err(failure)if failure.kind()==io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(5)),Err(failure)=>return Err(failure)}}for worker in workers{match worker.join(){Ok(Ok(()))=>{},Ok(Err(failure))=>eprintln!("verity enable request failed: {failure}"),Err(_)=>return Err(error(libc::EIO))}}Ok(())})?);Ok(running)}
 /// Allocate a real short Unix endpoint, retaining its exclusive native
 /// directory until the server has stopped and all owned sockets are gone.
 pub fn start_private_with_store(timeout:Duration,store:Arc<crate::fsverity::Store>)->io::Result<Self>{
  let directory=EndpointDirectory::new()?;let endpoint=directory.path.join("ctl");
  let mut running=Self::start_with_store(&endpoint,timeout,store)?;
  running.owned_directory=Some(directory);Ok(running)
 }
 pub fn endpoint(&self)->&std::path::Path{&self.path}
 pub fn shutdown(mut self)->io::Result<()>{self.stop_and_join()}
 fn stop_and_join(&mut self)->io::Result<()>{use std::os::unix::fs::MetadataExt;if self.cleaned{return Ok(());}self.stop.store(true,std::sync::atomic::Ordering::Release);let mut failure=None;for thread in self.threads.drain(..){match thread.join(){Ok(Ok(()))=>{},Ok(Err(error))=>failure=Some(error),Err(_)=>failure=Some(error(libc::EIO))}}self.server.channels.lock().unwrap().clear();if !self.process.is_live(){return Err(error(libc::ESRCH));}for(index,path)in [self.path.clone(),self.path.with_extension("admission")].iter().enumerate(){match std::fs::symlink_metadata(path){Ok(stat)if(stat.dev(),stat.ino())==self.inodes[index]=>std::fs::remove_file(path)?,Ok(_)=>return Err(error(libc::ESTALE)),Err(error)if error.kind()==io::ErrorKind::NotFound=>{},Err(error)=>return Err(error)}}if let Some(expected)=self.enable_inode{let path=self.path.with_extension("enable");match std::fs::symlink_metadata(&path){Ok(stat)if(stat.dev(),stat.ino())==expected=>std::fs::remove_file(path)?,Ok(_)=>return Err(error(libc::ESTALE)),Err(failure)if failure.kind()==io::ErrorKind::NotFound=>{},Err(failure)=>return Err(failure)}}if let Some(directory)=self.owned_directory.as_ref(){directory.remove()?;}self.cleaned=true;if let Some(error)=failure{Err(error)}else{Ok(())}}
}
impl Drop for RunningServer {fn drop(&mut self){if let Err(failure)=self.stop_and_join(){eprintln!("verity coordinator shutdown failed: {failure}");}}}

#[derive(Clone,Debug)]
pub struct OwnerConfig {pub endpoint:std::path::PathBuf,pub process:ProcessIdentity}
impl OwnerConfig {
 pub fn write(&self,path:&std::path::Path)->io::Result<()>{use std::os::unix::{ffi::OsStrExt,fs::OpenOptionsExt};if !self.process.is_live(){return Err(error(libc::ESRCH));}let endpoint=std::fs::canonicalize(&self.endpoint)?;let bytes=endpoint.as_os_str().as_bytes();if bytes.len()>=104{return Err(error(libc::ENAMETOOLONG));}let hex:String=bytes.iter().map(|byte|format!("{byte:02x}")).collect();let text=format!("AIMVERITY1\t{}\t{}\t{}\t{}\n",self.process.host_pid,self.process.start_seconds,self.process.start_microseconds,hex);let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)?;file.write_all(text.as_bytes())?;file.sync_all()}
 pub fn read(path:&std::path::Path)->io::Result<Self>{use std::os::unix::{ffi::OsStringExt,fs::MetadataExt};let stat=std::fs::symlink_metadata(path)?;if !stat.is_file()||stat.uid()!=unsafe{libc::geteuid()}||stat.mode()&0o777!=0o600{return Err(error(libc::EPERM));}let text=std::fs::read_to_string(path)?;if text.len()>512||!text.ends_with('\n'){return Err(error(libc::EINVAL));}let fields:Vec<_>=text.trim_end_matches('\n').split('\t').collect();if fields.len()!=5||fields[0]!="AIMVERITY1"{return Err(error(libc::EINVAL));}let process=ProcessIdentity{host_pid:fields[1].parse().map_err(|_|error(libc::EINVAL))?,start_seconds:fields[2].parse().map_err(|_|error(libc::EINVAL))?,start_microseconds:fields[3].parse().map_err(|_|error(libc::EINVAL))?};if !process.is_live(){return Err(error(libc::ESRCH));}if fields[4].len()%2!=0||!fields[4].as_bytes().iter().all(u8::is_ascii_hexdigit){return Err(error(libc::EINVAL));}let bytes=(0..fields[4].len()).step_by(2).map(|at|u8::from_str_radix(&fields[4][at..at+2],16).map_err(|_|error(libc::EINVAL))).collect::<io::Result<Vec<_>>>()?;let endpoint=std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes));if std::fs::canonicalize(&endpoint)?!=endpoint{return Err(error(libc::EPERM));}Ok(Self{endpoint,process})}
}

/// Parent holds admission without adding a mapping membership. A child must
/// register its own actual process and publish every restored identity first.
pub struct ForkLease {channel:Channel,frame:Frame}
impl Client {
 pub fn fork_lease(&self,identities:impl IntoIterator<Item=[u8;36]>)->io::Result<ForkLease>{let identities=identities.into_iter().collect::<BTreeSet<_>>();if identities.len()>65536{return Err(error(libc::E2BIG));}let mut channel=connect(&self.path,self.init,self.timeout)?;let frame=Frame{operation:Operation::ForkBegin,transaction:self.member,identity:[0;36],generation:identities.len()as u64,error:0};channel.send(&frame,None)?;for identity in identities{channel.send(&Frame{operation:Operation::ForkIdentity,identity,..frame.clone()},None)?;}let(reply,proof)=channel.receive()?;if reply.operation==Operation::Error{return Err(error(reply.error));}if proof.is_some()||reply!=frame{return Err(error(libc::EPROTO));}Ok(ForkLease{channel,frame})}
}
impl ForkLease {
 pub fn child_published(mut self,child_member:u64)->io::Result<()>{self.frame.operation=Operation::ForkHandoff;self.frame.generation=child_member;self.channel.send(&self.frame,None)?;let(reply,proof)=self.channel.receive()?;if proof.is_some()||reply!=self.frame{return Err(error(libc::EPROTO));}Ok(())}
 pub fn abort(mut self)->io::Result<()>{self.frame.operation=Operation::MapAbort;self.channel.send(&self.frame,None)?;let(reply,proof)=self.channel.receive()?;if proof.is_some()||reply!=self.frame{return Err(error(libc::EPROTO));}Ok(())}
}
impl Coordinator {
 fn batch_admission(self:&Arc<Self>,process:u64,identities:&BTreeSet<[u8;36]>,timeout:Duration)->io::Result<Vec<Admission>>{let deadline=std::time::Instant::now()+timeout;let mut state=self.state.lock().unwrap();loop{if !state.processes.get(&process).is_some_and(|p|p.is_live()){return Err(error(libc::ESRCH));}let mut active=false;for identity in identities{let inode=state.inodes.entry(*identity).or_default();if inode.poisoned{return Err(error(libc::EIO));}active|=inode.active.is_some();}if !active{break;}let remaining=deadline.saturating_duration_since(std::time::Instant::now());if remaining.is_zero(){return Err(error(libc::ETIMEDOUT));}let(next,result)=self.changed.wait_timeout(state,remaining).unwrap();state=next;if result.timed_out(){return Err(error(libc::ETIMEDOUT));}}let mut guards=Vec::new();guards.try_reserve_exact(identities.len()).map_err(|_|error(libc::ENOMEM))?;for identity in identities{state.inodes.get_mut(identity).unwrap().admissions+=1;guards.push(Admission{owner:self.clone(),process,identity:*identity,published:false});}Ok(guards)}
}
impl Server {
 fn fork_request(&self,mut channel:Channel,frame:Frame)->io::Result<()>{if frame.generation>65536||frame.identity!=[0;36]||frame.error!=0{return Err(error(libc::EPROTO));}let deadline=std::time::Instant::now()+self.timeout;let mut identities=BTreeSet::new();for _ in 0..frame.generation{if std::time::Instant::now()>=deadline{return Err(error(libc::ETIMEDOUT));}let(next,proof)=channel.receive()?;if proof.is_some()||next.operation!=Operation::ForkIdentity||next.transaction!=frame.transaction||next.generation!=frame.generation||next.error!=0||!identities.insert(next.identity){return Err(error(libc::EPROTO));}crate::fsverity::Identity::from_bytes(&next.identity)?;}let guards=match self.coordinator.batch_admission(frame.transaction,&identities,self.timeout){Ok(guards)=>guards,Err(failure)=>{channel.send(&Frame{operation:Operation::Error,error:failure.raw_os_error().unwrap_or(libc::EIO),..frame},None)?;return Err(failure);}};channel.send(&frame,None)?;let(done,proof)=channel.receive()?;if proof.is_some()||done.transaction!=frame.transaction||done.identity!=[0;36]||done.error!=0{return Err(error(libc::EPROTO));}match done.operation{Operation::MapAbort=>{},Operation::ForkHandoff=>{let state=self.coordinator.state.lock().unwrap();let process=state.processes.get(&done.generation).ok_or_else(||error(libc::ESRCH))?;let mut info:libc::proc_bsdinfo=unsafe{std::mem::zeroed()};let size=std::mem::size_of_val(&info)as i32;if !process.is_live()||unsafe{libc::proc_pidinfo(process.host_pid,libc::PROC_PIDTBSDINFO,1,(&mut info as*mut libc::proc_bsdinfo).cast(),size)}!=size||info.pbi_ppid as i32!=channel.peer.host_pid||identities.iter().any(|identity|!state.inodes[identity].members.contains_key(&done.generation)){return Err(error(libc::EPERM));}},_=>return Err(error(libc::EPROTO))}drop(guards);channel.send(&done,None)}
}

fn metadata_error(failure:crate::fsverity::Error)->io::Error{match failure{crate::fsverity::Error::Linux(code)=>error(code),crate::fsverity::Error::Io(error)=>error}}
fn file_identity(file:&impl std::os::fd::AsFd)->io::Result<(u64,u64)>{let mut stat:libc::stat=unsafe{std::mem::zeroed()};if unsafe{libc::fstat(file.as_fd().as_raw_fd(),&mut stat)}<0{return Err(io::Error::last_os_error());}Ok((stat.st_dev as u64,stat.st_ino))}
impl Server {
 pub fn enable_request(&self,mut channel:Channel)->io::Result<()> {
  let store=self.store.lock().unwrap().clone().ok_or_else(||error(libc::ENODEV))?;
  let(frame,proof)=channel.receive()?;if frame.operation!=Operation::EnableBegin{return Err(error(libc::EPROTO));}
  {let state=self.coordinator.state.lock().unwrap();if state.processes.get(&frame.transaction)!=Some(&channel.peer){return Err(error(libc::EPERM));}}
  let identity=crate::fsverity::Identity::from_bytes(&frame.identity)?;
  let proof=proof.ok_or_else(||error(libc::EPROTO))?;
  let prepared=crate::fsverity::Metadata::from_private_fd(proof.try_clone()?,identity).map_err(metadata_error)?;
  let expected=file_identity(&prepared.backing_descriptor())?;
  let mut transition=self.prepare(frame.identity,&proof)?;
  let ack=Frame{operation:Operation::EnablePrepared,..frame.clone()};
  let exchange=(||{channel.send(&ack,None)?;channel.receive()})();
  let marker=store.lookup(identity).map_err(metadata_error)?;
  let committed=marker.as_ref().is_some_and(|metadata|file_identity(&metadata.backing_descriptor()).ok()==Some(expected));
  if committed{transition.published()?;}
  match exchange {
   Ok((published,receipt))if published.operation==Operation::Abort&&published.transaction==frame.transaction&&!committed=>{transition.abort()?;channel.send(&Frame{operation:Operation::Aborted,..frame},None)},
   Ok((published,receipt))if published.operation==Operation::EnablePublished&&published.transaction==frame.transaction&&published.identity==frame.identity=>{
    let receipt=crate::fsverity::Metadata::from_private_fd(receipt.ok_or_else(||error(libc::EPROTO))?,identity).map_err(metadata_error)?;
    if !committed||file_identity(&receipt.backing_descriptor())?!=expected||receipt.descriptor().digest()!=prepared.descriptor().digest(){if committed{transition.published()?;transition.commit()?;}else{transition.abort()?;}return Err(error(libc::EPROTO));}
    transition.published()?;transition.commit()?;channel.send(&Frame{operation:Operation::EnableComplete,..frame},None)
   },
   _=>{if committed{transition.published()?;transition.commit()?;}else{transition.abort()?;}Err(error(libc::EIO))}
  }
 }
}
pub struct RemoteTransition {channel:Channel,frame:Frame}
impl Client {
 pub fn enable_transition(&self,identity:[u8;36],proof:&PrivateFd)->io::Result<RemoteTransition>{let endpoint=self.path.with_extension("enable");let mut channel=connect(&endpoint,self.init,self.timeout)?;let frame=Frame{operation:Operation::EnableBegin,transaction:self.member,identity,generation:0,error:0};channel.send(&frame,Some(proof))?;let(reply,fd)=channel.receive()?;if fd.is_some()||reply.operation!=Operation::EnablePrepared||reply.transaction!=frame.transaction||reply.identity!=identity{return Err(error(libc::EPROTO));}Ok(RemoteTransition{channel,frame})}
}
impl RemoteTransition {
 pub fn published(mut self,metadata:&crate::fsverity::Metadata)->io::Result<()>{let proof=PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}})?;self.frame.operation=Operation::EnablePublished;self.channel.send(&self.frame,Some(&proof))?;let(reply,fd)=self.channel.receive()?;if fd.is_some()||reply.operation!=Operation::EnableComplete||reply.transaction!=self.frame.transaction{return Err(error(libc::EIO));}Ok(())}
 pub fn abort(mut self)->io::Result<()>{self.frame.operation=Operation::Abort;self.channel.send(&self.frame,None)?;let(reply,fd)=self.channel.receive()?;if fd.is_some()||reply.operation!=Operation::Aborted{return Err(error(libc::EIO));}Ok(())}
}
