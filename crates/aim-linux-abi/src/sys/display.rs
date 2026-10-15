//! Native display sockets retain a private control descriptor. Only the
//! actual guest duplicate is published; its event bytes remain unframed.
use super::{cred,fdtab,net};
use crate::{errno::{self,Errno},vfs};
use aim_storage::socket_inode::Receipt;
use std::os::fd::{AsRawFd,BorrowedFd,FromRawFd,OwnedFd};
fn io(error:std::io::Error)->Errno{errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}
fn allocated(source:BorrowedFd<'_>)->Result<Receipt,Errno>{
 let runtime=vfs::runtime_dir().ok_or(errno::ENODEV)?;let identity=cred::current();
 aim_storage::socket_inode::allocated(runtime,source.as_raw_fd(),identity.uid[3],identity.gid[3]).map_err(io)
}
fn publish(source:BorrowedFd<'_>,receipt:Receipt)->Result<i32,Errno>{
 receipt.validate(source.as_raw_fd()).map_err(io)?;
 let file={let _guard=fdtab::lifecycle();let fd=unsafe{libc::fcntl(source.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{return Err(errno::last());}let file=unsafe{OwnedFd::from_raw_fd(fd)};net::adopt(fd);file};
 let fd=file.as_raw_fd();
 let result=net::install_socket_receipt(fd,receipt).and_then(|()|{let _guard=fdtab::lifecycle();fdtab::publish_typed_guest(fd)});
 match result{Ok(())=>{use std::os::fd::IntoRawFd;Ok(file.into_raw_fd())},Err(error)=>{let _guard=fdtab::lifecycle();fdtab::on_close(fd);Err(error)}}
}
pub fn init(){aim_host_display::set_hooks(aim_host_display::Hooks{allocated,publish});}

#[cfg(test)]mod tests{
 use super::*;use std::{io::Write,os::fd::AsFd};
 static ORIGINALS:std::sync::Mutex<Vec<i32>>=std::sync::Mutex::new(Vec::new());
 fn observed(source:BorrowedFd<'_>)->Result<Receipt,Errno>{ORIGINALS.lock().unwrap().push(source.as_raw_fd());allocated(source)}
 #[test]fn actual_display_connect_publishes_raw_event_stream_and_windows(){
  if fdtab::isolated_kernel_test("sys::display::tests::actual_display_connect_publishes_raw_event_stream_and_windows"){return;}
  let(_guard,_root)=vfs::test_view();fdtab::install_storage_registrar().unwrap();
  aim_host_display::set_hooks(aim_host_display::Hooks{allocated:observed,publish});
  let path=std::env::temp_dir().join(format!("aim-display-fd-{}.sock",std::process::id()));
  let listener=std::os::unix::net::UnixListener::bind(&path).unwrap();aim_host_display::set_server(&path);
  let event=aim_hostcall::display::Event{kind:7,_reserved:0,timestamp_ns:42,period_ns:16666667,sent_ns:43};assert_eq!(std::mem::size_of_val(&event),32);
  let expected=aim_host_display::wire::bytes(&event).to_vec();let sent=expected.clone();
  let server=std::thread::spawn(move||{for _ in 0..2{let(mut stream,_)=listener.accept().unwrap();let hello=aim_host_display::wire::recv_record::<aim_host_display::wire::Request>(stream.as_fd()).unwrap().unwrap();assert_eq!(hello.id,aim_host_display::wire::VERSION);match hello.op{aim_host_display::wire::OP_HELLO=>{let answer=aim_hostcall::display::Connect{width:1920,height:1080,..Default::default()};stream.write_all(aim_host_display::wire::bytes(&answer)).unwrap();stream.write_all(&sent).unwrap();},aim_host_display::wire::OP_WINDOWS=>{stream.write_all(aim_host_display::wire::bytes(&aim_hostcall::display::Windows{mode:2,_reserved:0})).unwrap();stream.write_all(&sent).unwrap();},other=>panic!("unexpected hello {other}")}}});
  for function in [aim_hostcall::display::FN_CONNECT,aim_hostcall::display::FN_WINDOWS]{
   let mut connect=aim_hostcall::display::Connect::default();let mut windows=aim_hostcall::display::Windows::default();
   let(arg,len)=if function==aim_hostcall::display::FN_CONNECT{(&mut connect as *mut _ as u64,std::mem::size_of_val(&connect))}else{(&mut windows as *mut _ as u64,std::mem::size_of_val(&windows))};
   let fd=unsafe{(aim_host_display::MODULE.call)(function,arg,len as u64)};assert!(fd>=0,"actual handshake/producer {fd}");
   assert_eq!(super::super::fs::fcntl([fd as u64,1,0,0,0,0]),1);
   let private=*ORIGINALS.lock().unwrap().last().unwrap();assert!(fdtab::is_hidden(private)||unsafe{libc::fcntl(private,libc::F_GETFD)}<0);
   assert_eq!(super::super::fs::read([private as u64,1,1,0,0,0]),-(errno::EBADF as i64));
   let mut stat=[0u8;128];assert_eq!(super::super::fs::fstat([fd as u64,stat.as_mut_ptr()as u64,0,0,0,0]),0);
   let mut poll=libc::pollfd{fd:fd as i32,events:libc::POLLIN,revents:0};let timeout=[1i64,0];assert_eq!(super::super::poll::ppoll([&mut poll as *mut _ as u64,1,timeout.as_ptr()as u64,0,0,0]),1);
   let mut received=[0u8;32];assert_eq!(super::super::fs::read([fd as u64,received.as_mut_ptr()as u64,32,0,0,0]),32);assert_eq!(received.as_slice(),expected.as_slice(),"native display Event is not a Linux SCM frame");
   assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
  }
  server.join().unwrap();std::fs::remove_file(path).unwrap();
 }
}
