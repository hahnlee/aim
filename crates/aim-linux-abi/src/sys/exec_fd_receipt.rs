//! Kernel-private close-on-exec receipts survive only a successful host exec.
use crate::{errno::{self,Errno},vfs};
use aim_storage::{inode_lease::Identity,private_fd::{PrivateFd,PrivateFile},process_namespace::ProcessIdentity};
use sha2::{Digest,Sha256};
use std::{fs::{self,File},io::Read,os::{fd::{AsFd,AsRawFd,FromRawFd},unix::fs::{OpenOptionsExt,PermissionsExt,MetadataExt}},path::PathBuf};
const HEADER:usize=128;
const MAGIC:&[u8;8]=b"AIMEXCL1";
fn io(error:std::io::Error)->Errno{errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}
pub struct Receipt{writer:PrivateFile,carrier:PrivateFd,path:PathBuf,bytes:Vec<u8>,process:ProcessIdentity,identity:Identity}
impl Drop for Receipt{fn drop(&mut self){if let Err(error)=fs::remove_file(&self.path){if error.kind()!=std::io::ErrorKind::NotFound{crate::diag!("exec receipt cleanup: {error}");}}}}
impl Receipt{
 pub fn prepare()->Result<Option<Self>,Errno>{
  match super::posix_locks::current(){Ok(_)=>{},Err(37)=>return Ok(None),Err(error)=>return Err(error)}
  let runtime=vfs::runtime_dir().ok_or(errno::EINVAL)?;let directory=runtime.join("exec-fd-receipts");
  match fs::create_dir(&directory){Ok(())=>fs::set_permissions(&directory,fs::Permissions::from_mode(0o700)).map_err(io)?,Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(error)=>return Err(io(error))}
  let metadata=fs::symlink_metadata(&directory).map_err(io)?;
  if !metadata.is_dir()||metadata.mode()&0o777!=0o700||metadata.uid()!=unsafe{libc::geteuid()}{return Err(errno::EPERM);}
  let directory=fs::canonicalize(directory).map_err(io)?;
  let mut nonce=[0u8;16];if unsafe{libc::getentropy(nonce.as_mut_ptr().cast(),nonce.len())}<0{return Err(errno::last());}
  let path=directory.join(nonce.iter().map(|byte|format!("{byte:02x}")).collect::<String>());
  let writer=PrivateFile::allocate(||File::options().create_new(true).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path)).map_err(io)?;
  let result=(||{
   let mut limit:libc::rlimit=unsafe{std::mem::zeroed()};if unsafe{libc::getrlimit(libc::RLIMIT_NOFILE,&mut limit)}<0{return Err(errno::last());}
   let size=HEADER+36*limit.rlim_cur.min(1<<20)as usize;
   let mut bytes=Vec::new();bytes.try_reserve_exact(size).map_err(|_|errno::ENOMEM)?;bytes.resize(size,0);
   if unsafe{libc::ftruncate(writer.as_raw_fd(),size as i64)}<0{return Err(errno::last());}
   let carrier=PrivateFd::allocate(||Ok(File::options().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path)?.into())).map_err(io)?;
   if unsafe{libc::fcntl(carrier.as_raw_fd(),libc::F_SETFD,0)}<0{return Err(errno::last());}
   let process=ProcessIdentity::running(unsafe{libc::getpid()}).map_err(io)?;let identity=Identity::from_fd(carrier.as_fd()).map_err(io)?;
   Ok((carrier,bytes,process,identity))
  })();
  match result{Ok((carrier,bytes,process,identity))=>Ok(Some(Self{writer,carrier,path,bytes,process,identity})),Err(error)=>{fs::remove_file(&path).map_err(io)?;Err(error)}}
 }
 pub fn fd(&self)->i32{self.carrier.as_raw_fd()}
 /// Caller holds the lifecycle gate through capture and the actual exec syscall.
 pub fn capture(&mut self)->Result<(),Errno>{
  let mut count=0usize;
  for fd in super::fd_visibility::visible(){
   let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{return Err(errno::last());}if flags&libc::FD_CLOEXEC==0{continue;}
   let identity=Identity::from_fd(unsafe{std::os::fd::BorrowedFd::borrow_raw(fd)}).map_err(io)?;
   let at=HEADER+count*36;if at+36>self.bytes.len(){return Err(errno::ENOMEM);}
   self.bytes[at..at+36].copy_from_slice(&identity.to_bytes());count+=1;
  }
  self.bytes[..8].copy_from_slice(MAGIC);self.bytes[8..12].copy_from_slice(&self.process.host_pid.to_le_bytes());
  self.bytes[16..24].copy_from_slice(&self.process.start_seconds.to_le_bytes());self.bytes[24..32].copy_from_slice(&self.process.start_microseconds.to_le_bytes());
  self.bytes[32..68].copy_from_slice(&self.identity.to_bytes());self.bytes[68..72].copy_from_slice(&(count as u32).to_le_bytes());
  let length=HEADER+count*36;let digest=Sha256::digest(&self.bytes[HEADER..length]);self.bytes[80..112].copy_from_slice(&digest);
  let mut written=0;while written<length{let n=unsafe{libc::pwrite(self.writer.as_raw_fd(),self.bytes[written..length].as_ptr().cast(),length-written,written as i64)};if n<0{return Err(errno::last());}if n==0{return Err(errno::EIO);}written+=n as usize;}Ok(())
 }
}
pub fn consume(fd:i32)->Result<(),Errno>{
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
 let count=u32::from_le_bytes(header[68..72].try_into().unwrap())as usize;if count>1<<20{return Err(errno::EINVAL);}
 let mut bytes=vec![0;count*36];file.read_exact(&mut bytes).map_err(io)?;
 if Sha256::digest(&bytes).as_slice()!=&header[80..112]{return Err(errno::EIO);}
 let client=super::posix_locks::current()?;
 for bytes in bytes.chunks_exact(36){client.close_inode(Identity::from_bytes(bytes).map_err(io)?)?;}
 fs::remove_file(path).map_err(io)?;Ok(())
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
  let second=option("PHASE=")=="second";
  if !second{let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();assert_eq!(line.trim(),"admitted");}
  vfs::init(&root.join("root"),Some(&root.join("run/path-map"))).unwrap();super::super::fdtab::install_storage_registrar().unwrap();
  super::super::cred::init(super::super::cred::Identity::default(),Some(root.join("run/identity/by-pid")));
  super::super::posix_locks::start(Some(&locator)).unwrap();
  if second{
   consume(option("RECEIPT=").parse().unwrap()).unwrap();println!("SUCCESS_EXEC_OK");return;
  }
  let name=std::ffi::CString::new("/data/receipt-lock").unwrap();
  let fd=super::super::fs::openat([vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,2,0,0,0]);assert!(fd>=0);
  let lock=super::super::posix_locks::LinuxFlock{kind:1,whence:0,pad:0,start:0,len:0,pid:0,pad2:0};
  assert_eq!(super::super::fs::fcntl([fd as u64,6,(&lock as*const _)as u64,0,0,0]),0);
  assert_eq!(super::super::fs::fcntl([fd as u64,2,libc::FD_CLOEXEC as u64,0,0,0]),0);
  let mut receipt=Receipt::prepare().unwrap().unwrap();
  let guard=super::super::fdtab::lifecycle();receipt.capture().unwrap();
  let missing=std::ffi::CString::new(root.join("missing-executable").to_str().unwrap()).unwrap();let args=[missing.as_ptr(),std::ptr::null()];let env=[std::ptr::null()];
  assert_eq!(unsafe{libc::execve(missing.as_ptr(),args.as_ptr(),env.as_ptr())},-1);assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::ENOENT));
  drop(guard);drop(receipt);println!("FAILED_EXEC_OK");
  let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();assert_eq!(line.trim(),"exec");
  let mut receipt=Receipt::prepare().unwrap().unwrap();
  let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact",CHILD,"--ignored","--nocapture","--skip",&format!("ROOT={}",root.display()),"--skip","PHASE=second","--skip",&format!("RECEIPT={}",receipt.fd())]);
  let _guard=super::super::fdtab::lifecycle();receipt.capture().unwrap();
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
 fn actual_host_exec_releases_only_committed_cloexec_locks(){
  let(_view,root)=vfs::test_view();fs::write(root.join("data/receipt-lock"),b"lock bytes").unwrap();
  let actual=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let table=root.join("run/identity/by-pid");
  let namespace=aim_storage::mount_namespace::Namespace::open(vfs::runtime_dir().unwrap(),&format!("receipt-{}",actual.host_pid)).unwrap();
  namespace.initialize(&fs::read_to_string(root.join("run/path-map")).unwrap()).unwrap();
  let registration=aim_storage::process_namespace::InitRegistration::register(&table,actual,namespace.id()).unwrap();
  let holder=std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("aim-lock-holder");assert!(holder.is_file(),"required native holder was not built");
  let controller=aim_storage::posix_broker::Controller::start(aim_storage::posix_broker::Config{endpoint:format!("com.aim.exec-close.{}",actual.host_pid),holder,startup_timeout:Duration::from_secs(5)}).unwrap();
  controller.bind_namespace(&table,&registration).unwrap();controller.owner_config().unwrap().write(&root.join("run/posix-owner")).unwrap();
  let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact",CHILD,"--ignored","--nocapture","--skip",&format!("ROOT={}",root.display()),"--skip","PHASE=first"]).stdin(Stdio::piped()).stdout(Stdio::piped());
  let mut child=Child(command.spawn().unwrap());let process=ProcessIdentity::running(child.0.id()as i32).unwrap();
  aim_storage::process_namespace::register_mount_namespace(&table,process,&registration.mount_namespace).unwrap();
  controller.register_guest(aim_storage::posix_control::Owner{process,guest_pid:process.host_pid}).unwrap();
  writeln!(child.0.stdin.as_mut().unwrap(),"admitted").unwrap();
  let mut reader=std::io::BufReader::new(child.0.stdout.take().unwrap());marker(&mut reader,"FAILED_EXEC_OK");
  let file=File::options().read(true).write(true).open(root.join("data/receipt-lock")).unwrap();assert!(!probe(&file),"failed exec must preserve the actual native lock");
  writeln!(child.0.stdin.as_mut().unwrap(),"exec").unwrap();marker(&mut reader,"SUCCESS_EXEC_OK");assert!(probe(&file),"successful CLOEXEC close must release the actual native lock");
  assert!(child.0.wait().unwrap().success());controller.guest_exited(aim_storage::posix_control::Owner{process,guest_pid:process.host_pid}).unwrap();controller.shutdown().unwrap();registration.remove(&table).unwrap();
 }
}
