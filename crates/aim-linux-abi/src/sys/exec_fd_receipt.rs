//! Kernel-private close-on-exec receipts survive only a successful host exec.
use crate::{errno::{self,Errno},vfs};
use aim_storage::{inode_lease::Identity,private_fd::{PrivateFd,PrivateFile},process_namespace::ProcessIdentity};
use sha2::{Digest,Sha256};
use std::{fs::{self,File},io::Read,os::{fd::{AsFd,AsRawFd,FromRawFd},unix::fs::{OpenOptionsExt,PermissionsExt,MetadataExt}},path::PathBuf};
const HEADER:usize=128;
const RECORD:usize=40;
const MAGIC:&[u8;8]=b"AIMEXCL2";
fn io(error:std::io::Error)->Errno{errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}
pub struct Receipt{writer:PrivateFile,carrier:PrivateFd,path:PathBuf,bytes:Vec<u8>,captured:Vec<(i32,i32,Identity)>,process:ProcessIdentity,identity:Identity}
pub struct HostExecFlags(Vec<(i32,i32)>);
impl Drop for HostExecFlags{fn drop(&mut self){for(fd,flags)in&self.0{unsafe{libc::fcntl(*fd,libc::F_SETFD,*flags);}}}}
impl Drop for Receipt{fn drop(&mut self){if let Err(error)=fs::remove_file(&self.path){if error.kind()!=std::io::ErrorKind::NotFound{crate::diag!("exec receipt cleanup: {error}");}}}}
impl Receipt{
 pub fn prepare()->Result<Option<Self>,Errno>{
  let runtime=vfs::runtime_dir().ok_or(errno::EINVAL)?;let directory=runtime.join("exec-fd-receipts");
  match fs::create_dir(&directory){Ok(())=>fs::set_permissions(&directory,fs::Permissions::from_mode(0o700)).map_err(io)?,Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(error)=>return Err(io(error))}
  let metadata=fs::symlink_metadata(&directory).map_err(io)?;
  if !metadata.is_dir()||metadata.mode()&0o777!=0o700||metadata.uid()!=unsafe{libc::geteuid()}{return Err(errno::EPERM);}
  let directory=fs::canonicalize(directory).map_err(io)?;
  let mut nonce=[0u8;16];if unsafe{libc::getentropy(nonce.as_mut_ptr().cast(),nonce.len())}<0{return Err(errno::last());}
  let path=directory.join(nonce.iter().map(|byte|format!("{byte:02x}")).collect::<String>());
  let writer=PrivateFile::allocate(||File::options().create_new(true).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path)).map_err(io)?;
  let result=(||{
   let bytes=vec![0;HEADER];
   if unsafe{libc::ftruncate(writer.as_raw_fd(),HEADER as i64)}<0{return Err(errno::last());}
   let carrier=PrivateFd::allocate(||Ok(File::options().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path)?.into())).map_err(io)?;
   if unsafe{libc::fcntl(carrier.as_raw_fd(),libc::F_SETFD,0)}<0{return Err(errno::last());}
   let process=ProcessIdentity::running(unsafe{libc::getpid()}).map_err(io)?;let identity=Identity::from_fd(carrier.as_fd()).map_err(io)?;
   Ok((carrier,bytes,process,identity))
  })();
  match result{Ok((carrier,bytes,process,identity))=>Ok(Some(Self{writer,carrier,path,bytes,captured:Vec::new(),process,identity})),Err(error)=>{fs::remove_file(&path).map_err(io)?;Err(error)}}
 }
 pub fn fd(&self)->i32{self.carrier.as_raw_fd()}
 /// Caller holds the lifecycle gate through capture and the actual exec syscall.
 pub fn capture(&mut self)->Result<(),Errno>{
  let mut captured=Vec::new();
  for fd in super::fd_visibility::visible(){
   let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{return Err(errno::last());}if flags&libc::FD_CLOEXEC==0{continue;}
   let identity=Identity::from_fd(unsafe{std::os::fd::BorrowedFd::borrow_raw(fd)}).map_err(io)?;
   captured.push((fd,flags,identity));
  }
  let count=captured.len();let count32=u32::try_from(count).map_err(|_|errno::ENOMEM)?;
  let length=count.checked_mul(RECORD).and_then(|records|HEADER.checked_add(records)).ok_or(errno::ENOMEM)?;let file_length=i64::try_from(length).map_err(|_|errno::ENOMEM)?;
  let mut bytes=Vec::new();bytes.try_reserve_exact(length).map_err(|_|errno::ENOMEM)?;bytes.resize(length,0);
  for(index,(fd,_,identity))in captured.iter().enumerate(){let at=HEADER+index*RECORD;bytes[at..at+4].copy_from_slice(&fd.to_le_bytes());bytes[at+4..at+RECORD].copy_from_slice(&identity.to_bytes());}
  bytes[..8].copy_from_slice(MAGIC);bytes[8..12].copy_from_slice(&self.process.host_pid.to_le_bytes());
  bytes[16..24].copy_from_slice(&self.process.start_seconds.to_le_bytes());bytes[24..32].copy_from_slice(&self.process.start_microseconds.to_le_bytes());
  bytes[32..68].copy_from_slice(&self.identity.to_bytes());bytes[68..72].copy_from_slice(&count32.to_le_bytes());
  let digest=Sha256::digest(&bytes[HEADER..]);bytes[80..112].copy_from_slice(&digest);
  if unsafe{libc::ftruncate(self.writer.as_raw_fd(),file_length)}<0{return Err(errno::last());}
  let mut written=0;while written<length{let n=unsafe{libc::pwrite(self.writer.as_raw_fd(),bytes[written..].as_ptr().cast(),length-written,written as i64)};if n<0{return Err(errno::last());}if n==0{return Err(errno::EIO);}written+=n as usize;}
  self.bytes=bytes;self.captured=captured;Ok(())
 }
 pub fn retain_for_host_exec(&self)->Result<HostExecFlags,Errno>{
  let mut changed=HostExecFlags(Vec::new());
  for(fd,original,identity)in&self.captured{
   if !super::fdtab::visible(*fd){return Err(errno::EPERM);}
   let flags=unsafe{libc::fcntl(*fd,libc::F_GETFD)};if flags<0{return Err(errno::last());}if flags!=*original||flags&libc::FD_CLOEXEC==0{return Err(errno::EPERM);}
   let current=Identity::from_fd(unsafe{std::os::fd::BorrowedFd::borrow_raw(*fd)}).map_err(io)?;if current!=*identity{return Err(errno::EPERM);}
   if unsafe{libc::fcntl(*fd,libc::F_SETFD,flags&!libc::FD_CLOEXEC)}<0{return Err(errno::last());}
   changed.0.push((*fd,flags));
  }
  Ok(changed)
 }
}
struct OpenReceipt{file:PrivateFile,path:PathBuf,header:[u8;HEADER]}
fn open(fd:i32)->Result<OpenReceipt,Errno>{
 if fd<0||super::fdtab::visible(fd){return Err(errno::EPERM);}
 let file=PrivateFd::adopt(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)}).map_err(io)?;
 let mut file=PrivateFile::from_private_fd(file);let flags=unsafe{libc::fcntl(file.as_raw_fd(),libc::F_GETFL)};
 if flags<0{return Err(errno::last());}if flags&libc::O_ACCMODE!=libc::O_RDONLY{return Err(errno::EPERM);}
 let path=PathBuf::from(crate::xrt::fd_path(file.as_raw_fd()).ok_or(errno::EPERM)?);
 let directory=vfs::runtime_dir().ok_or(errno::EPERM)?.join("exec-fd-receipts");
 let owner=fs::symlink_metadata(&directory).map_err(io)?;
 if !owner.is_dir()||owner.mode()&0o777!=0o700||owner.uid()!=unsafe{libc::geteuid()}{return Err(errno::EPERM);}
 let directory=fs::canonicalize(directory).map_err(io)?;
 if path.parent()!=Some(directory.as_path())||fs::canonicalize(&path).map_err(io)?!=path{return Err(errno::EPERM);}
 let metadata=fs::symlink_metadata(&path).map_err(io)?;
 let identity=Identity::from_fd(file.as_fd()).map_err(io)?;
 if !metadata.is_file()||metadata.mode()&0o777!=0o600||metadata.uid()!=unsafe{libc::geteuid()}||metadata.dev()!=identity.dev||metadata.ino()!=identity.ino{return Err(errno::EPERM);}
 let mut header=[0;HEADER];file.read_exact(&mut header).map_err(io)?;
 let process=ProcessIdentity::running(unsafe{libc::getpid()}).map_err(io)?;
 if &header[..8]!=MAGIC||header[8..12]!=process.host_pid.to_le_bytes()||header[16..24]!=process.start_seconds.to_le_bytes()||header[24..32]!=process.start_microseconds.to_le_bytes()||header[32..68]!=Identity::from_fd(file.as_fd()).map_err(io)?.to_bytes()||header[12..16].iter().chain(header[72..80].iter()).chain(header[112..].iter()).any(|byte|*byte!=0){return Err(errno::EPERM);}
 Ok(OpenReceipt{file,path,header})
}
pub fn discard(fd:i32)->Result<(),Errno>{
 let OpenReceipt{file,path,..}=open(fd)?;
 fs::remove_file(path).map_err(io)?;
 drop(file);
 Ok(())
}
pub fn consume(fd:i32)->Result<(),Errno>{
 let OpenReceipt{mut file,path,header}=open(fd)?;
 let count=u32::from_le_bytes(header[68..72].try_into().unwrap())as usize;if count>1<<20{return Err(errno::EINVAL);}
 let mut bytes=vec![0;count*RECORD];file.read_exact(&mut bytes).map_err(io)?;
 if Sha256::digest(&bytes).as_slice()!=&header[80..112]{return Err(errno::EIO);}
 let mut targets=Vec::with_capacity(count);
 for bytes in bytes.chunks_exact(RECORD){
  let target=i32::from_le_bytes(bytes[..4].try_into().unwrap());
  if target<0||!super::fdtab::visible(target){return Err(errno::EPERM);}
  let identity=Identity::from_bytes(&bytes[4..]).map_err(io)?;
  let actual=Identity::from_fd(unsafe{std::os::fd::BorrowedFd::borrow_raw(target)}).map_err(io)?;
  if actual!=identity{return Err(errno::EPERM);}
  targets.push(target);
 }
 fs::remove_file(path).map_err(io)?;
 drop(file);
 super::fdtab::close_exec_batch(targets)
}

#[cfg(test)]
mod tests{
 use super::*;
 use std::{io::{BufRead,Write},process::{Command,Stdio},time::{Duration,Instant}};
 const CHILD:&str="sys::exec_fd_receipt::tests::exec_child";
 fn option(name:&str)->String{std::env::args().find_map(|arg|arg.strip_prefix(name).map(String::from)).unwrap()}
 #[test]
 #[ignore="executed by actual_host_exec_releases_only_committed_cloexec_locks"]
 fn exec_child(){
  let root=PathBuf::from(option("ROOT="));let locator=root.join("run/posix-owner");
  let second=option("PHASE=")=="second";let writer=option("WRITER=").parse().unwrap();
  if !second{let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();assert_eq!(line.trim(),"admitted");}
  vfs::init(&root.join("root"),Some(&root.join("run/path-map"))).unwrap();super::super::fdtab::install_storage_registrar().unwrap();
  let mut inherited=vec![0,1,2,writer];
  if second{inherited.push(option("TARGET=").parse().unwrap());}
  super::super::fdtab::bootstrap(inherited,&[]).unwrap();
  super::super::cred::init(super::super::cred::Identity::default(),Some(root.join("run/identity/by-pid")));
  super::super::posix_locks::start(Some(&locator)).unwrap();
  if second{
   super::super::fdtab::restore_regular_exec(&option("REGULAR=")).unwrap();
   let target=option("TARGET=").parse().unwrap();
   let Some(super::super::fdtab::Kind::Regular(description))=super::super::fdtab::get(target)else{panic!("regular exec owner missing")};
   assert!(description.writer.is_some(),"CLOEXEC writer carrier was not restored before commit");
   consume(option("RECEIPT=").parse().unwrap()).unwrap();println!("SUCCESS_EXEC_OK");return;
  }
  assert_eq!(unsafe{libc::fcntl(writer,libc::F_SETFD,libc::FD_CLOEXEC)},0);
  let name=std::ffi::CString::new("/data/receipt-lock").unwrap();
  let fd=super::super::fs::openat([vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,2,0,0,0]);assert!(fd>=0);let fd=fd as i32;
  let lock=super::super::posix_locks::LinuxFlock{kind:1,whence:0,pad:0,start:0,len:0,pid:0,pad2:0};
  assert_eq!(super::super::fs::fcntl([fd as u64,6,(&lock as*const _)as u64,0,0,0]),0);
  assert_eq!(super::super::fs::fcntl([fd as u64,2,libc::FD_CLOEXEC as u64,0,0,0]),0);
  let mut receipt=Receipt::prepare().unwrap().unwrap();
  let guard=super::super::fdtab::lifecycle();receipt.capture().unwrap();
  let missing=std::ffi::CString::new(root.join("missing-executable").to_str().unwrap()).unwrap();let args=[missing.as_ptr(),std::ptr::null()];let env=[std::ptr::null()];
  assert_eq!(unsafe{libc::execve(missing.as_ptr(),args.as_ptr(),env.as_ptr())},-1);assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::ENOENT));
  drop(guard);drop(receipt);
  let mut discarded=Receipt::prepare().unwrap().unwrap();
  let guard=super::super::fdtab::lifecycle();discarded.capture().unwrap();drop(guard);
  let duplicate=unsafe{libc::fcntl(discarded.fd(),libc::F_DUPFD_CLOEXEC,3)};assert!(duplicate>=0);
  discard(duplicate).unwrap();
  assert!(super::super::fdtab::visible(fd));
  assert!(unsafe{libc::fcntl(fd,libc::F_GETFD)}>=0,"discarding a failed exec receipt must preserve its CLOEXEC target");
  drop(discarded);println!("FAILED_EXEC_OK");
  let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();assert_eq!(line.trim(),"exec");
  let mut receipt=Receipt::prepare().unwrap().unwrap();
  let regular=super::super::fdtab::regular_exec_text().unwrap();
  let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact",CHILD,"--ignored","--nocapture","--skip",&format!("ROOT={}",root.display()),"--skip","PHASE=second","--skip",&format!("RECEIPT={}",receipt.fd()),"--skip",&format!("TARGET={fd}"),"--skip",&format!("WRITER={writer}"),"--skip",&format!("REGULAR={regular}")]);
  let _guard=super::super::fdtab::lifecycle();receipt.capture().unwrap();
  let _writers=super::super::fdtab::prepare_regular_exec().unwrap();
  let _cloexec=receipt.retain_for_host_exec().unwrap();
  use std::os::unix::process::CommandExt;panic!("actual exec failed: {}",command.exec());
 }
 struct Child(std::process::Child);
 impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
 fn marker(reader:&mut std::io::BufReader<std::process::ChildStdout>,marker:&str){
  let deadline=Instant::now()+Duration::from_secs(15);
  loop{
   assert!(Instant::now()<deadline,"child marker timed out: {marker}");
   let mut poll=libc::pollfd{fd:reader.get_ref().as_raw_fd(),events:libc::POLLIN,revents:0};
   if reader.buffer().is_empty()&&unsafe{libc::poll(&mut poll,1,100)}<=0{continue;}
   let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0,"child ended before {marker}");if line.contains(marker){break;}
  }
 }
 fn probe(file:&File)->bool{
  let lock=libc::flock{l_start:0,l_len:0,l_pid:0,l_type:libc::F_WRLCK,l_whence:libc::SEEK_SET as i16};
  let result=unsafe{libc::fcntl(file.as_raw_fd(),libc::F_SETLK,&lock)};
  if result<0{assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::EAGAIN));false}
 else{let unlock=libc::flock{l_type:libc::F_UNLCK,..lock};assert_eq!(unsafe{libc::fcntl(file.as_raw_fd(),libc::F_SETLK,&unlock)},0);true}
 }
 #[test]
 fn large_nofile_limit_keeps_empty_receipt_header_sized(){
  // It captures every open fd, which tests alongside open and close.
  if super::super::fdtab::isolated_kernel_test("sys::exec_fd_receipt::tests::large_nofile_limit_keeps_empty_receipt_header_sized"){return;}
  let mut limit:libc::rlimit=unsafe{std::mem::zeroed()};assert_eq!(unsafe{libc::getrlimit(libc::RLIMIT_NOFILE,&mut limit)},0);
  let legacy=HEADER+36*limit.rlim_cur.min(1<<20)as usize;assert!(legacy>1<<20,"test requires a large RLIMIT_NOFILE");
  let(_view,_root)=vfs::test_view();let mut receipt=Receipt::prepare().unwrap().unwrap();
  assert_eq!(receipt.bytes.len(),HEADER);assert_eq!(fs::metadata(&receipt.path).unwrap().len(),HEADER as u64);
  let guard=super::super::fdtab::lifecycle();receipt.capture().unwrap();drop(guard);
  let exact=HEADER+receipt.captured.len()*RECORD;assert_eq!(receipt.bytes.len(),exact);assert_eq!(fs::metadata(&receipt.path).unwrap().len(),exact as u64);assert!(exact<legacy);
 }
 #[test]
 fn actual_host_exec_releases_only_committed_cloexec_locks(){
  let(_view,root)=vfs::test_view();fs::write(root.join("data/receipt-lock"),b"lock bytes").unwrap();
  let actual=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let table=root.join("run/identity/by-pid");
  let namespace=aim_storage::mount_namespace::Namespace::open(vfs::runtime_dir().unwrap(),&format!("receipt-{}",actual.host_pid)).unwrap();
  namespace.initialize(&fs::read_to_string(root.join("run/path-map")).unwrap()).unwrap();
  let registration=aim_storage::process_namespace::InitRegistration::register(&table,actual,namespace.id()).unwrap();
  let Some(holder)=aim_paths::input(aim_paths::host_bin("aim-lock-holder"),"host/aim-lock-holder")else{return};
  let controller=aim_storage::posix_broker::Controller::start(aim_storage::posix_broker::Config{endpoint:format!("com.aim.exec-close.{}",actual.host_pid),holder,startup_timeout:Duration::from_secs(5)}).unwrap();
  controller.bind_namespace(&table,&registration).unwrap();controller.owner_config().unwrap().write(&root.join("run/posix-owner")).unwrap();
  let mut wake=[-1;2];assert_eq!(unsafe{libc::pipe(wake.as_mut_ptr())},0);assert_eq!(unsafe{libc::fcntl(wake[1],libc::F_SETFD,0)},0);
  let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact",CHILD,"--ignored","--nocapture","--skip",&format!("ROOT={}",root.display()),"--skip","PHASE=first","--skip",&format!("WRITER={}",wake[1])]).stdin(Stdio::piped()).stdout(Stdio::piped());
  let mut child=Child(command.spawn().unwrap());let process=ProcessIdentity::running(child.0.id()as i32).unwrap();
  assert_eq!(unsafe{libc::close(wake[1])},0);
  aim_storage::process_namespace::register_mount_namespace(&table,process,&registration.mount_namespace).unwrap();
  controller.register_guest(aim_storage::posix_control::Owner{process,guest_pid:process.host_pid}).unwrap();
  writeln!(child.0.stdin.as_mut().unwrap(),"admitted").unwrap();
  let mut reader=std::io::BufReader::new(child.0.stdout.take().unwrap());marker(&mut reader,"FAILED_EXEC_OK");
  let file=File::options().read(true).write(true).open(root.join("data/receipt-lock")).unwrap();assert!(!probe(&file),"failed exec must preserve the actual native lock");
  writeln!(child.0.stdin.as_mut().unwrap(),"exec").unwrap();
  let mut event=libc::pollfd{fd:wake[0],events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut event,1,15000)},1,"successful exec did not close its CLOEXEC wake descriptor");
  let mut byte=0u8;assert_eq!(unsafe{libc::read(wake[0],(&mut byte as*mut u8).cast(),1)},0,"wake descriptor must report EOF");
  assert!(probe(&file),"all close owner effects must finish before the first exec descriptor exposes EOF");
  marker(&mut reader,"SUCCESS_EXEC_OK");assert_eq!(unsafe{libc::close(wake[0])},0);
  assert!(child.0.wait().unwrap().success());controller.guest_exited(aim_storage::posix_control::Owner{process,guest_pid:process.host_pid}).unwrap();controller.shutdown().unwrap();registration.remove(&table).unwrap();
 }
}
