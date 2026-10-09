//! Linux v6.12 fs-verity query ABI; enabling is owned by the global transition.
use crate::errno::{self,Errno};
use super::{fdtab::{Kind,Pinned},regular_file::Description};
use aim_storage::fsverity::{BuildOptions,Error,Metadata,ENODATA,EMSGSIZE};
use aim_storage::private_fd::PrivateFd;
use std::os::fd::FromRawFd;
use std::os::fd::AsRawFd;

const MEASURE:u64=0xc0046686;
const READ_METADATA:u64=0xc0286687;
const GETFLAGS:u64=0x80086601;
const ENABLE:u64=0x40806685;
pub(super) const VERITY_ATTRIBUTE:u64=0x100000;
const VERITY_FLAG:u32=0x100000;

fn core(error:Error)->Errno{match error{Error::Linux(error)=>error,Error::Io(error)=>errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}}
pub(super) fn read<const N:usize>(address:u64)->Result<[u8;N],Errno>{
 Ok(super::user_memory::read_exact(address,N)?.try_into().unwrap())
}
pub(super) fn write(address:u64,bytes:&[u8])->Result<(),Errno>{
 super::user_memory::write_exact(address,bytes)
}
fn enabled(description:&Description)->Result<Option<Metadata>,Errno>{description.store.lookup(description.identity).map_err(core)}
fn read_bytes(address:u64,length:usize)->Result<Vec<u8>,Errno>{
 super::user_memory::read_exact(address,length)
}
fn enable(pin:&Pinned,arg:u64)->Result<i64,Errno>{
 let client=crate::verity_client().ok_or(errno::ENOTTY)?;
 let argument=read::<128>(arg)?;let word=|at|u32::from_le_bytes(argument[at..at+4].try_into().unwrap());
 if word(0)!=1||word(28)!=0||argument[40..].iter().any(|byte|*byte!=0)||!word(8).is_power_of_two(){return Err(errno::EINVAL);}
 if word(12)>32||word(24)>16128{return Err(EMSGSIZE);}
 let fd=pin.descriptor().as_raw_fd();
 let stat=super::fs::stat_fd_kernel(fd).map_err(|error|(-error)as Errno)?;
 if !super::attrs::permits(&stat,2,&super::cred::current(),super::attrs::FS){return Err(errno::EACCES);}
 let flags=unsafe{libc::fcntl(fd,libc::F_GETFL)};if flags<0{return Err(errno::last());}
 if flags&libc::O_ACCMODE==libc::O_WRONLY||matches!(pin.kind(),Some(Kind::Regular(description))if description.flags&3==3){return Err(errno::EBADF);}
 if stat.st_flags&(libc::UF_APPEND|libc::SF_APPEND)!=0{return Err(errno::EPERM);}
 if stat.st_mode&libc::S_IFMT==libc::S_IFDIR{return Err(errno::EISDIR);}
 if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(errno::EINVAL);}
 if crate::vfs::on_read_only_root(stat.st_dev){return Err(errno::EROFS);}
 let Some(Kind::Regular(description))=pin.kind()else{return Err(errno::ENOTTY)};
 super::binder::drain_regular_scm(&description.identity.to_bytes()).map_err(|error|if error==errno::EAGAIN{26}else{error})?;
 let admission=description.store.lock_inode(&pin.descriptor()).map_err(core)?;
 let exclusion=admission.exclude_writers().map_err(core)?;drop(admission);
 let salt=read_bytes(u64::from_le_bytes(argument[16..24].try_into().unwrap()),word(12)as usize)?;
 let signature=read_bytes(u64::from_le_bytes(argument[32..40].try_into().unwrap()),word(24)as usize)?;
 // The pinned ps16k r07 kernel has builtin signature verification disabled.
 let options=BuildOptions::from_enable_arg(&argument,salt,16384,4096).map_err(core)?;
 let admission=description.store.lock_inode(&pin.descriptor()).map_err(core)?;
 let guard=admission.begin_enable_excluded(exclusion).map_err(core)?;drop(admission);
 let prepared=guard.build(options,&signature,||super::thread::current().is_some_and(super::signal::fatal_pending)).map_err(core)?;
 let view=prepared.metadata_view().map_err(core)?;
 let proof=PrivateFd::allocate(||{
  let fd=unsafe{libc::fcntl(view.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};
  if fd<0{return Err(std::io::Error::last_os_error());}Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})
 }).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
 let transition=client.enable_transition(description.identity.to_bytes(),&proof).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
 match guard.commit(prepared){
  Ok(metadata)=>{transition.published(&metadata).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;Ok(0)},
  Err(error)=>{
   let failure=core(error);
   match enabled(description){
    Ok(Some(metadata))=>{transition.published(&metadata).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;Err(failure)},
    Ok(None)=>{transition.abort().map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;Err(failure)},
    Err(error)=>Err(error),
   }
  },
 }
}
fn query(description:&Description,request:u64,arg:u64)->Result<i64,Errno>{
 let metadata=enabled(description)?;
 if request==GETFLAGS{return write(arg,&if metadata.is_some(){VERITY_FLAG}else{0}.to_le_bytes()).map(|_|0);}
 let metadata=metadata.ok_or(ENODATA)?;
 match request{
  MEASURE=>{
   let capacity=u16::from_le_bytes(read::<2>(arg.checked_add(2).ok_or(errno::EFAULT)?)?) as usize;
   let(algorithm,digest)=metadata.measure(capacity).map_err(core)?;
   let mut header=[0;4];header[..2].copy_from_slice(&algorithm.to_le_bytes());header[2..].copy_from_slice(&(digest.len()as u16).to_le_bytes());
   write(arg,&header)?;write(arg.checked_add(4).ok_or(errno::EFAULT)?,&digest)?;Ok(0)
  },
  READ_METADATA=>{
   let bytes=read::<40>(arg)?;let word=|at|u64::from_le_bytes(bytes[at..at+8].try_into().unwrap());
   let(kind,offset,length,output,reserved)=(word(0),word(8),word(16),word(24),word(32));
   if reserved!=0||offset.checked_add(length).is_none(){return Err(errno::EINVAL);}
   let bytes=metadata.read_metadata(kind,offset,length.min(i32::MAX as u64)as usize).map_err(core)?;
   write(output,&bytes)?;Ok(bytes.len()as i64)
  },
  _=>Err(errno::ENOTTY),
 }
}
pub(super) fn ioctl(pin:&Pinned,request:u64,arg:u64)->Option<i64>{
 if request==ENABLE{
  if !matches!(pin.kind(),Some(Kind::Regular(_)|Kind::Dir(_))){return None;}
  return Some(enable(pin,arg).unwrap_or_else(|error|-(error as i64)));
 }
 if !matches!(request,MEASURE|READ_METADATA|GETFLAGS){return None;}
 let Some(Kind::Regular(description))=pin.kind()else{return Some(-(errno::ENOTTY as i64));};
 if unsafe{libc::fcntl(pin.descriptor().as_raw_fd(),libc::F_GETFL)}<0{return Some(-(errno::last()as i64));}
 Some(query(description,request,arg).unwrap_or_else(|error|-(error as i64)))
}

#[cfg(test)]
mod tests{
 use super::*;
 use aim_storage::fsverity::EOVERFLOW;
 use std::{fs::File,os::fd::AsRawFd};
 #[test]
 #[ignore="actual native daemon run by issuer_child"]
 fn binder_child(){let name=std::env::args().find_map(|arg|arg.strip_prefix("BINDER=").map(String::from)).unwrap();aim_binder_host::server::Server::start(&name).unwrap();loop{std::thread::park();}}
 struct Child(std::process::Child);
 impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
 #[test]
 #[ignore="actual isolated issuer exercised by enable_uses_real_native_prepare_publication_and_queries"]
 fn issuer_child(){
  let(_view,root)=crate::vfs::test_view();let name=std::ffi::CString::new("/data/native-enable-issuer").unwrap();
  std::fs::write(root.join("data/native-enable-issuer"),vec![39;8193]).unwrap();
  let service=format!("com.aim.verity-issuer.{}",std::process::id());
  let mut binder=Child(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::fsverity_ioctl::tests::binder_child","--ignored","--nocapture","--skip",&format!("BINDER={service}")]).stdout(std::process::Stdio::null()).spawn().unwrap());
  let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
  while aim_binder_host::client::Client::connect(&service).is_none(){assert!(std::time::Instant::now()<deadline);assert!(binder.0.try_wait().unwrap().is_none());std::thread::sleep(std::time::Duration::from_millis(10));}
  super::super::binder::init(&service).unwrap();
  let fd=super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,0,0,0,0]);assert!(fd>=0);
  let store=super::super::regular_file::configured_store().unwrap().unwrap();
  let running=aim_storage::verity_control::RunningServer::start_with_store(&root.join("run/verity-control"),std::time::Duration::from_secs(2),store.clone()).unwrap();
  aim_storage::verity_control::OwnerConfig{endpoint:root.join("run/verity-control"),process:running.process}.write(&root.join("run/verity-control-owner")).unwrap();
  crate::start_verity_runtime().unwrap();
  let ioctl_only=super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,3,0,0,0]);assert!(ioctl_only>=0);
  assert_eq!(super::super::fs::fcntl([ioctl_only as u64,3,0,0,0,0])&3,3);
  let mut byte=0u8;
  assert_eq!(super::super::fs::read([ioctl_only as u64,(&mut byte as*mut u8)as u64,1,0,0,0]),-(errno::EBADF as i64));
  assert_eq!(super::super::fs::write([ioctl_only as u64,(&byte as*const u8)as u64,1,0,0,0]),-(errno::EBADF as i64));
  assert_eq!(super::super::fsops::ftruncate([ioctl_only as u64,0,0,0,0,0]),-(errno::EINVAL as i64));
  assert_eq!(super::super::fsops::fallocate([ioctl_only as u64,0,0,4096,0,0]),-(errno::EBADF as i64));
  assert_eq!(super::super::mem::mmap([0,16384,1,2,ioctl_only as u64,0]),-(errno::EACCES as i64));
  assert_eq!(super::super::fs::ioctl([fd as u64,ENABLE,1,0,0,0]),-(errno::EFAULT as i64));
  let mut argument=[0u8;128];argument[..4].copy_from_slice(&1u32.to_le_bytes());argument[4..8].copy_from_slice(&1u32.to_le_bytes());argument[8..12].copy_from_slice(&4096u32.to_le_bytes());
  argument[12..16].copy_from_slice(&33u32.to_le_bytes());assert_eq!(super::super::fs::ioctl([fd as u64,ENABLE,argument.as_ptr()as u64,0,0,0]),-(EMSGSIZE as i64));argument[12..16].fill(0);
  assert_eq!(super::super::fs::ioctl([fd as u64,ENABLE,argument.as_ptr()as u64,0,0,0]),0);
  let mut flags=0u32;assert_eq!(super::super::fs::ioctl([ioctl_only as u64,GETFLAGS,(&mut flags as*mut u32)as u64,0,0,0]),0);assert_eq!(flags,VERITY_FLAG);
  assert_eq!(super::super::fs::close([ioctl_only as u64,0,0,0,0,0]),0);
  assert_eq!(super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,2,0,0,0]),-(errno::EPERM as i64));
  assert_eq!(super::super::fs::ioctl([fd as u64,ENABLE,argument.as_ptr()as u64,0,0,0]),-17);
  let mut digest=[0u8;68];digest[2..4].copy_from_slice(&64u16.to_le_bytes());assert_eq!(super::super::fs::ioctl([fd as u64,MEASURE,digest.as_mut_ptr()as u64,0,0,0]),0);assert_eq!(&digest[..4],&[1,0,32,0]);
  let mut bytes=[0u8;8193];assert_eq!(super::super::fs::read([fd as u64,bytes.as_mut_ptr()as u64,bytes.len()as u64,0,0,0]),8193);assert_eq!(bytes,[39;8193]);
  assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
  let runtime=crate::VERITY_RUNTIME.lock().unwrap().take().unwrap();runtime.shutdown().unwrap();running.shutdown().unwrap();println!("NATIVE_ENABLE_ISSUER_EXECUTED");
 }
 #[test]
 fn enable_uses_real_native_prepare_publication_and_queries(){
  let mut child=Child(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::fsverity_ioctl::tests::issuer_child","--ignored","--nocapture"]).stdout(std::process::Stdio::piped()).spawn().unwrap());
  let deadline=std::time::Instant::now()+std::time::Duration::from_secs(20);
  let status=loop{if let Some(status)=child.0.try_wait().unwrap(){break status;}assert!(std::time::Instant::now()<deadline,"native issuer child timed out");std::thread::sleep(std::time::Duration::from_millis(10));};
  use std::io::Read;let mut output=String::new();child.0.stdout.take().unwrap().read_to_string(&mut output).unwrap();assert!(status.success(),"{output}");assert!(output.contains("NATIVE_ENABLE_ISSUER_EXECUTED"));
 }
 #[test]
 fn queries_preserve_linux_errno_and_authenticated_metadata(){
  let(_view,root)=crate::vfs::test_view();
  let path=root.join("data/query-verity");std::fs::write(&path,b"actual verity bytes").unwrap();
  let file=File::open(path).unwrap();
  let description=super::super::regular_file::adopt(file.as_raw_fd(),0).unwrap().unwrap();
  let store=description.store.clone();
  assert_eq!(query(&description,MEASURE,1),Err(ENODATA));
  assert_eq!(query(&description,READ_METADATA,1),Err(ENODATA));
  let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);
  let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();
  let metadata=enable.commit(prepared).unwrap();
  assert_eq!(query(&description,MEASURE,1),Err(errno::EFAULT));
  let mut digest=[0u8;68];digest[2..4].copy_from_slice(&31u16.to_le_bytes());
  assert_eq!(query(&description,MEASURE,digest.as_mut_ptr()as u64),Err(EOVERFLOW));
  digest[2..4].copy_from_slice(&64u16.to_le_bytes());
  assert_eq!(query(&description,MEASURE,digest.as_mut_ptr()as u64),Ok(0));
  assert_eq!(&digest[..4],&[1,0,32,0]);assert_eq!(&digest[4..36],metadata.measure(64).unwrap().1);
  let mut output=[0u8;256];let mut argument=[2u64,0,256,output.as_mut_ptr()as u64,0];
  assert_eq!(query(&description,READ_METADATA,argument.as_mut_ptr()as u64),Ok(256));
  assert_eq!(&output,metadata.descriptor().bytes());
  argument[4]=1;assert_eq!(query(&description,READ_METADATA,argument.as_ptr()as u64),Err(errno::EINVAL));
  argument[4]=0;argument[1]=u64::MAX;argument[2]=1;assert_eq!(query(&description,READ_METADATA,argument.as_ptr()as u64),Err(errno::EINVAL));
  argument[1]=256;argument[2]=1;argument[3]=1;assert_eq!(query(&description,READ_METADATA,argument.as_ptr()as u64),Ok(0));
  argument[1]=0;argument[3]=1;assert_eq!(query(&description,READ_METADATA,argument.as_ptr()as u64),Err(errno::EFAULT));
  let mut flags=0u32;assert_eq!(query(&description,GETFLAGS,(&mut flags as*mut u32)as u64),Ok(0));assert_eq!(flags,VERITY_FLAG);
  let name=std::ffi::CString::new("/data/query-verity").unwrap();
  let fd=super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,0,0,0,0]);assert!(fd>=0);
  digest[2..4].copy_from_slice(&64u16.to_le_bytes());
  assert_eq!(super::super::fs::ioctl([fd as u64,MEASURE,digest.as_mut_ptr()as u64,0,0,0]),0);
  assert_eq!(&digest[4..36],metadata.measure(64).unwrap().1);
  let mut statx=[0u64;32];
  assert_eq!(super::super::fs::statx([fd as u64,b"\0".as_ptr()as u64,0x1000,0x7ff,statx.as_mut_ptr()as u64,0]),0);
  assert_eq!(statx[1]&VERITY_ATTRIBUTE,VERITY_ATTRIBUTE);assert_eq!(statx[7]&VERITY_ATTRIBUTE,VERITY_ATTRIBUTE);
  assert_eq!(super::super::fs::statx([fd as u64,b"\0".as_ptr()as u64,0x1000,0x7ff,1,0]),-(errno::EFAULT as i64));
  assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
 }
}
