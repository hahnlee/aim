//! Socket carrier retirement runs after actual descriptor destruction and locks.
use crate::errno::Errno;
#[derive(Default)]
struct Retirement { generation:u64, acknowledged:u64, error:Option<Errno> }
static BARRIER:std::sync::Mutex<()>=std::sync::Mutex::new(());
static OWNER:std::sync::Mutex<Retirement>=std::sync::Mutex::new(Retirement{generation:0,acknowledged:0,error:None});
pub(crate) fn note_socket_close(){let mut owner=OWNER.lock().unwrap();owner.generation+=1;}
fn barrier(force:bool)->Result<(),Errno>{
    let _barrier=BARRIER.lock().unwrap();
    let generation={let owner=OWNER.lock().unwrap();if !force&&owner.generation==owner.acknowledged&&owner.error.is_none(){return Ok(());}owner.generation};
    let result=super::net::drain_socket_carriers();
    let mut owner=OWNER.lock().unwrap();
    match result{
        Ok(())=>{owner.acknowledged=owner.acknowledged.max(generation);if owner.acknowledged==owner.generation{owner.error=None;}Ok(())},
        Err(error)=>{owner.error=Some(error);diagnostic(format_args!("stage=carrier-owner errno={error} generation={generation} pending_preserved=true"));Err(error)}
    }
}
pub(crate) fn flush()->Result<(),Errno>{barrier(false)}
pub(crate) fn release_checkpoint()->u64{OWNER.lock().unwrap().generation}
pub(crate) fn after_release(checkpoint:u64,result:i64)->i64{
    // A concurrent release belongs to the same shared carrier service owner.
    // Old pending failures alone never trigger an unrelated syscall retry.
    if release_checkpoint()!=checkpoint{
        if let Err(error)=flush(){diagnostic(format_args!("stage=release-epilogue operation_result={result} retirement_errno={error}"));}
    }
    result
}
#[cfg(test)]
pub(crate) fn pending()->bool{let owner=OWNER.lock().unwrap();owner.generation!=owner.acknowledged||owner.error.is_some()}
/// Called before socket I/O/readiness, never after consuming guest data.
pub(crate) fn observe_socket()->Result<(),Errno>{barrier(true)}
pub(crate) fn observes(pin:&super::fdtab::Pinned)->bool{
    match pin.kind(){Some(super::fdtab::Kind::Sock(_))=>true,Some(super::fdtab::Kind::Epoll(ep))=>super::epoll::observes_sockets(ep,{use std::os::fd::AsRawFd;pin.descriptor().as_raw_fd()},&mut std::collections::HashSet::new()),_=>false}
}
pub(crate) fn run(operation:impl FnOnce()->i64)->i64{
    let result=operation();
    // Failure remains in the carrier owner. It cannot change an already
    // completed operation or that descriptor's own close error.
    if let Err(error)=flush(){diagnostic(format_args!("stage=retirement-deferred operation_result={result} retirement_errno={error}"));}
    result
}

/// Error-only diagnostic cohort; never changes errno or guest return values.
pub(crate) fn diagnostic(arguments:std::fmt::Arguments<'_>){
    static RECORDS:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
    if RECORDS.fetch_add(1,std::sync::atomic::Ordering::Relaxed)>=256{return;}
    let saved=unsafe{*libc::__error()};
    crate::diag!("[fd-close-owner] pid={} tid={} {}",unsafe{libc::getpid()},super::thread::gettid(),arguments);
    unsafe{*libc::__error()=saved;}
}

#[cfg(test)]
mod tests{
 use super::*;
 use std::{io::{BufRead,Write},process::{Command,Stdio}};
 const SERVER:&str="sys::close_effects::tests::real_carrier_service_child";
 #[test]
 #[ignore="actual owned native service child of full_dispatch_close_reports_real_pending_retirement_failure"]
 fn real_carrier_service_child(){
  let marker=std::env::args().find(|arg|arg.starts_with("CLOSE_OWNER_SERVER=")).unwrap();let name=marker.strip_prefix("CLOSE_OWNER_SERVER=").unwrap();let _server=aim_binder_host::server::Server::start(name).unwrap();println!("CLOSE_OWNER_SERVICE_READY");std::io::stdout().flush().unwrap();let mut command=String::new();std::io::stdin().read_line(&mut command).unwrap();
 }
 #[test]
 fn full_dispatch_close_reports_real_pending_retirement_failure(){
  if super::super::fdtab::isolated_kernel_test("sys::close_effects::tests::full_dispatch_close_reports_real_pending_retirement_failure"){return;}
  use super::super::{fdtab,net,fs,binder};
  let(_view,_directory)=crate::vfs::test_view();let name=format!("dev.aim.close-owner.{}",std::process::id());
  struct Child(Option<std::process::Child>);impl Drop for Child{fn drop(&mut self){if let Some(mut child)=self.0.take(){if child.try_wait().unwrap().is_none(){child.kill().unwrap();}child.wait().unwrap();}}}
  let mut child=Child(Some(Command::new(std::env::current_exe().unwrap()).args(["--exact",SERVER,"--ignored","--nocapture","--skip",&format!("CLOSE_OWNER_SERVER={name}")]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap()));
  let mut output=std::io::BufReader::new(child.0.as_mut().unwrap().stdout.take().unwrap());loop{let mut line=String::new();assert!(output.read_line(&mut line).unwrap()>0);if line.contains("CLOSE_OWNER_SERVICE_READY"){break;}}
  binder::init(&name).unwrap();
  let pair=||{let mut fds=[-1i32;2];assert_eq!(net::socketpair([1,1,0,fds.as_mut_ptr()as u64,0,0]),0);fds};let subject=pair();let carrier_queue=pair();
  let mut control=[0u8;24];control[..8].copy_from_slice(&20u64.to_ne_bytes());control[8..12].copy_from_slice(&1i32.to_ne_bytes());control[12..16].copy_from_slice(&1i32.to_ne_bytes());control[16..20].copy_from_slice(&subject[0].to_ne_bytes());let mut byte=[b'x'];let iov=libc::iovec{iov_base:byte.as_mut_ptr().cast(),iov_len:1};let message=[0u64,0,&iov as*const _ as u64,1,control.as_mut_ptr()as u64,24,0];assert_eq!(net::sendmsg([carrier_queue[0]as u64,message.as_ptr()as u64,0,0,0,0]),1);
  let mut pipe=[-1i32;2];assert_eq!(fs::pipe2([pipe.as_mut_ptr()as u64,0,0,0,0,0]),0);
  fdtab::close_owned_guest(subject[0],false).unwrap();assert!({let owner=OWNER.lock().unwrap();owner.generation>owner.acknowledged},"actual socket close must queue retirement");
  let process=child.0.as_mut().unwrap();process.kill().unwrap();process.wait().unwrap();child.0=None;
  let fd=pipe[0];let mut context:crate::context::GuestContext=unsafe{std::mem::zeroed()};context.x[8]=57;context.x[0]=fd as u64;super::super::dispatch(&mut context);
  assert_eq!(context.x[0]as i64,0,"carrier failure cannot replace actual pipe close success");assert_eq!(unsafe{libc::fcntl(fd,libc::F_GETFD)},-1,"actual pipe close completed without retirement override");assert!(!fdtab::visible(fd));assert!({let owner=OWNER.lock().unwrap();owner.generation>owner.acknowledged},"failure stays pending rather than silently discarded");
  context.x[8]=172;super::super::dispatch(&mut context);assert_eq!(context.x[0]as i32,unsafe{libc::getpid()},"getpid has no carrier dependency");
  assert_eq!(fs::close([fd as u64,0,0,0,0,0]),-(crate::errno::EBADF as i64),"own close errno survives retirement failure");
  let mut observation=[-1i32;2];assert_eq!(fs::pipe2([observation.as_mut_ptr()as u64,0,0,0,0,0]),0);
  let mut descriptor=libc::pollfd{fd:observation[1],events:libc::POLLOUT,revents:0};let zero=libc::timespec{tv_sec:0,tv_nsec:0};
  assert_eq!(super::super::poll::ppoll([&mut descriptor as*mut _ as u64,1,&zero as*const _ as u64,0,0,0]),1,"pipe-only poll does not contact carrier owner");
  let mut set=vec![0u64;observation[1]as usize/64+1];set[observation[1]as usize/64]|=1u64<<(observation[1]%64);
  assert_eq!(super::super::poll::pselect6([observation[1]as u64+1,0,set.as_mut_ptr()as u64,0,&zero as*const _ as u64,0]),1,"pipe-only select ignores unrelated owner fault");
  let ep=super::super::epoll::epoll_create1([0;6]);assert!(ep>=0);let event=[4u64,7];assert_eq!(super::super::epoll::epoll_ctl([ep as u64,1,observation[1]as u64,event.as_ptr()as u64,0,0]),0);let mut output=[0u64;2];
  assert_eq!(super::super::epoll::epoll_pwait([ep as u64,output.as_mut_ptr()as u64,1,0,0,0]),1,"pipe-only epoll ignores unrelated owner fault");
  let nested=super::super::epoll::epoll_create1([0;6]);assert!(nested>=0);let read_event=[1u64,8];assert_eq!(super::super::epoll::epoll_ctl([nested as u64,1,ep as u64,read_event.as_ptr()as u64,0,0]),0);
  assert_eq!(super::super::epoll::epoll_pwait([nested as u64,output.as_mut_ptr()as u64,1,0,0,0]),1,"nested pipe-only epoll has no carrier dependency");
  assert_eq!(super::super::epoll::epoll_ctl([ep as u64,1,carrier_queue[1]as u64,read_event.as_ptr()as u64,0,0]),0);
  assert!(super::super::epoll::epoll_pwait([nested as u64,output.as_mut_ptr()as u64,1,0,0,0])<0,"nested socket interest requires genuine owner barrier");
  fdtab::close_owned_guest(nested as i32,false).unwrap();
  fdtab::close_owned_guest(ep as i32,false).unwrap();
  let error=std::thread::spawn(flush).join().unwrap().unwrap_err();assert!(error>0,"another thread must retain and retry the actual pending owner obligation");assert!(OWNER.lock().unwrap().error.is_some());
  let mut received=[0u8;1];assert!(fs::read([carrier_queue[1]as u64,received.as_mut_ptr()as u64,1,0,0,0])<0,"socket observation fails before consuming queued payload");
  use std::os::fd::AsRawFd;let pin=fdtab::pin_guest(carrier_queue[1]).unwrap();assert_eq!(unsafe{libc::recv(pin.descriptor().as_raw_fd(),received.as_mut_ptr().cast(),1,libc::MSG_PEEK|libc::MSG_DONTWAIT)},1);assert_eq!(received[0],b'x');drop(pin);
  // Preserve the diagnosed production failure; the isolated fixture owns the
  // remaining actual resources and closes them without asserting fake success.
  for fd in [observation[0],observation[1],pipe[1],subject[1],carrier_queue[0],carrier_queue[1]]{fdtab::close_owned_guest(fd,false).unwrap();}*OWNER.lock().unwrap()=Retirement::default();
  println!("CLOSE_OWNER_REAL_NONINTERFERENCE_EXECUTED");
 }
}
