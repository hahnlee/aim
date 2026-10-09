//! An opaque writer's final kernel reference produces real pipe EOF, including
//! queued rights, fork aliases and recipients that never connected to a service.
use std::{io,mem,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}}};
pub(super) type Key=(u64,u64);
#[repr(C)] struct FileInfo{flags:u32,status:u32,offset:i64,kind:i32,guard:u32}
#[repr(C)] struct PipeInfo{stat:libc::vinfo_stat,handle:u64,peer:u64,status:i32,reserved:i32}
#[repr(C)] struct PipeFd{file:FileInfo,pipe:PipeInfo}
fn failure(code:i32)->io::Error{io::Error::from_raw_os_error(code)}
fn info(fd:i32)->io::Result<Key>{
 let mut value:PipeFd=unsafe{mem::zeroed()};let size=mem::size_of_val(&value)as i32;
 let count=unsafe{libc::proc_pidfdinfo(libc::getpid(),fd,6,(&mut value as*mut PipeFd).cast(),size)};
 if count!=size{let error=io::Error::last_os_error();return Err(if error.raw_os_error()==Some(0){failure(libc::EIO)}else{error});}
 Ok((value.pipe.handle,value.pipe.peer))
}
/// Only anonymous, write-end pipe descriptors can be registered capabilities.
pub(super) fn key(fd:i32)->io::Result<Option<Key>>{
 let mut stat:libc::stat=unsafe{mem::zeroed()};if unsafe{libc::fstat(fd,&mut stat)}<0{return Err(io::Error::last_os_error());}
 if stat.st_mode&libc::S_IFMT!=libc::S_IFIFO{return Ok(None);}
 let flags=unsafe{libc::fcntl(fd,libc::F_GETFL)};if flags<0{return Err(io::Error::last_os_error());}if flags&libc::O_ACCMODE!=libc::O_WRONLY{return Ok(None);}
 match info(fd){Ok(key)=>Ok(Some(key)),Err(error)if error.raw_os_error()==Some(libc::EBADF)=>{
  // PROC_PIDFDPIPEINFO uses EBADF for a live vnode FIFO, too. Preserve a
  // genuinely closed descriptor error instead of misclassifying that FIFO.
  if unsafe{libc::fstat(fd,&mut stat)}<0{return Err(io::Error::last_os_error());}Ok(None)
 },Err(error)=>Err(error)}
}
pub(super) struct Reader{descriptor:OwnedFd,pub key:Key,pub tag:usize}
impl Reader{
 pub fn fd(&self)->i32{self.descriptor.as_raw_fd()}
 pub fn matches(&self,key:Key)->io::Result<bool>{let current=info(self.fd())?;Ok(self.key==key&&current==(key.1,key.0))}
 pub fn eof(&self)->io::Result<bool>{
  let mut byte=0u8;
  loop{let count=unsafe{libc::read(self.fd(),(&mut byte as*mut u8).cast(),1)};if count==0{return Ok(true);}if count>0{return Err(failure(libc::EPROTO));}
   let error=io::Error::last_os_error();match error.kind(){io::ErrorKind::Interrupted=>continue,io::ErrorKind::WouldBlock=>return Ok(false),_=>return Err(error)}
  }
 }
}
struct Reactor{descriptor:OwnedFd,next:AtomicUsize,error:Mutex<Option<i32>>}
static REACTOR:Mutex<Option<Arc<Reactor>>>=Mutex::new(None);
impl Reactor{
 fn start()->io::Result<Arc<Self>>{
  let fd=unsafe{libc::kqueue()};if fd<0{return Err(io::Error::last_os_error());}let descriptor=unsafe{OwnedFd::from_raw_fd(fd)};
  if unsafe{libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC)}<0{return Err(io::Error::last_os_error());}
  let owner=Arc::new(Self{descriptor,next:AtomicUsize::new(1),error:Mutex::new(None)});let worker=owner.clone();
  // This one registry-owned reactor has process lifetime, independent of
  // service guards. It holds no backing socket or writer reference.
  std::thread::Builder::new().name("socket-token-owner".into()).spawn(move||worker.run())?;Ok(owner)
 }
 fn healthy(&self)->io::Result<()>{match *self.error.lock().unwrap(){Some(code)=>Err(failure(code)),None=>Ok(())}}
 fn record(&self,error:&io::Error){*self.error.lock().unwrap()=Some(error.raw_os_error().unwrap_or(libc::EIO));eprintln!("socket token owner: {error}");}
 fn register(&self,fd:i32)->io::Result<usize>{
  self.healthy()?;let tag=self.next.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|next|next.checked_add(1)).map_err(|_|failure(libc::EOVERFLOW))?;
  let change=libc::kevent{ident:fd as usize,filter:libc::EVFILT_READ,flags:libc::EV_ADD|libc::EV_CLEAR|libc::EV_RECEIPT,fflags:0,data:0,udata:tag as*mut _};let mut receipt:libc::kevent=unsafe{mem::zeroed()};let zero=libc::timespec{tv_sec:0,tv_nsec:0};
  let count=unsafe{libc::kevent(self.descriptor.as_raw_fd(),&change,1,&mut receipt,1,&zero)};
  if count<0{return Err(io::Error::last_os_error());}if count!=1||receipt.flags&libc::EV_ERROR==0{return Err(failure(libc::EPROTO));}if receipt.data!=0{return Err(failure(receipt.data as i32));}Ok(tag)
 }
 fn run(&self){
  loop{let mut events:[libc::kevent;32]=unsafe{mem::zeroed()};let count=unsafe{libc::kevent(self.descriptor.as_raw_fd(),std::ptr::null(),0,events.as_mut_ptr(),events.len()as i32,std::ptr::null())};
   if count<0{let error=io::Error::last_os_error();if error.kind()==io::ErrorKind::Interrupted{continue;}self.record(&error);return;}
   for event in &events[..count as usize]{let result=if event.flags&libc::EV_ERROR!=0{Err(failure(event.data as i32))}else if event.filter!=libc::EVFILT_READ{Err(failure(libc::EPROTO))}else{super::token_event(event.ident as i32,event.udata as usize,event.flags&libc::EV_EOF!=0)};if let Err(error)=result{self.record(&error);}}
  }
 }
}
fn reactor()->io::Result<Arc<Reactor>>{let mut state=REACTOR.lock().unwrap();if state.is_none(){*state=Some(Reactor::start()?);}let owner=state.as_ref().unwrap().clone();owner.healthy()?;Ok(owner)}
pub(super) fn healthy()->io::Result<()>{if let Some(owner)=REACTOR.lock().unwrap().as_ref(){owner.healthy()?;}Ok(())}
pub(super) fn create()->io::Result<(Reader,OwnedFd)>{
 let owner=reactor()?;let mut fds=[0;2];if unsafe{libc::pipe(fds.as_mut_ptr())}<0{return Err(io::Error::last_os_error());}
 let reader=unsafe{OwnedFd::from_raw_fd(fds[0])};let writer=unsafe{OwnedFd::from_raw_fd(fds[1])};
 for descriptor in [&reader,&writer]{if unsafe{libc::fcntl(descriptor.as_raw_fd(),libc::F_SETFD,libc::FD_CLOEXEC)}<0{return Err(io::Error::last_os_error());}}
 let flags=unsafe{libc::fcntl(reader.as_raw_fd(),libc::F_GETFL)};if flags<0||unsafe{libc::fcntl(reader.as_raw_fd(),libc::F_SETFL,flags|libc::O_NONBLOCK)}<0{return Err(io::Error::last_os_error());}
 let key=key(writer.as_raw_fd())?.ok_or_else(||failure(libc::EPROTO))?;if info(reader.as_raw_fd())?!=(key.1,key.0){return Err(failure(libc::EPROTO));}
 let tag=owner.register(reader.as_raw_fd())?;Ok((Reader{descriptor:reader,key,tag},writer))
}
