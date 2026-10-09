//! Regular open descriptions retain inode writer admission across aliases (#226).
use crate::{errno::{self,Errno},vfs};
use aim_storage::{fsverity::{Store,Error},inode_lease::{Identity,Inode,WriterLease},private_fd::{PrivateFd,PrivateFile}};
use std::{fs::File,io::Read,os::{fd::{AsRawFd,BorrowedFd,FromRawFd},unix::{ffi::OsStrExt,fs::OpenOptionsExt}},path::{Path,PathBuf},sync::{Arc,OnceLock}};
fn io(error:std::io::Error)->Errno{errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}
fn core(error:Error)->Errno{match error{Error::Linux(error)=>error,Error::Io(error)=>io(error)}}
struct Configuration{store:Arc<Store>,leases:PathBuf}
static CONFIGURATION:OnceLock<Result<Option<Configuration>,Errno>>=OnceLock::new();
fn configuration()->Result<Option<&'static Configuration>,Errno>{
 match CONFIGURATION.get_or_init(||{
  let Some(runtime)=vfs::runtime_dir()else{return Ok(None)};
  load_configuration(runtime)
 }){Ok(configuration)=>Ok(configuration.as_ref()),Err(error)=>Err(*error)}
}
fn load_configuration(runtime:&Path)->Result<Option<Configuration>,Errno>{
  let locator=runtime.join("fs-verity-root");
  match std::fs::symlink_metadata(&locator){
   Ok(_)=>{},
   Err(error)if error.kind()==std::io::ErrorKind::NotFound=>{
    return match std::fs::symlink_metadata(runtime.join("verity-control-owner")){
     Err(error)if error.kind()==std::io::ErrorKind::NotFound=>Ok(None),
     Err(error)=>Err(io(error)),
     Ok(_)=>Err(errno::ENOENT),
    };
   },
   Err(error)=>return Err(io(error)),
  }
  let mut file=PrivateFile::allocate(||File::options().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(locator)).map_err(io)?;
  let mut bytes=Vec::new();file.by_ref().take(4110).read_to_end(&mut bytes).map_err(io)?;
  let path=bytes.strip_prefix(b"AIMVRTROOT01\0").ok_or(errno::EINVAL)?;
  if path.is_empty()||path.contains(&0)||path.len()>4096{return Err(errno::EINVAL);}
  let path=PathBuf::from(std::ffi::OsStr::from_bytes(path));
  if !path.is_absolute()||!std::fs::symlink_metadata(&path).map_err(io)?.is_dir()||std::fs::canonicalize(&path).map_err(io)?!=path{return Err(errno::EINVAL);}
  let leases=runtime.join("fs-verity-leases");let store=Arc::new(Store::new(&path,&leases).map_err(core)?);Ok(Some(Configuration{store,leases}))
}
pub struct Description{pub flags:u64,pub identity:Identity,pub writer:Option<WriterLease>,pub store:Arc<Store>,inode:Inode}
impl Description{
 pub fn new(fd:BorrowedFd<'_>,flags:u64,store:Arc<Store>)->Result<Arc<Self>,Errno>{
  let configuration=configuration()?.ok_or(errno::EOPNOTSUPP)?;
  let inode=Inode::open(&configuration.leases,fd).map_err(io)?;
  let admission=store.lock_inode(&fd).map_err(core)?;let identity=admission.identity();
  let writer=if matches!(flags&3,1|2){Some(admission.writer_lease().map_err(core)?)}else{None};
  Ok(Arc::new(Self{flags:flags&!super::fs::O_CLOEXEC,identity,writer,store,inode}))
 }
 pub fn check_write(&self)->Result<(),Errno>{if !matches!(self.flags&3,1|2){return Err(errno::EBADF);}if self.store.lookup(self.identity).map_err(core)?.is_some(){return Err(errno::EPERM);}Ok(())}

}
pub fn adopt(fd:i32,flags:u64)->Result<Option<Arc<Description>>,Errno>{
 let mut stat:libc::stat=unsafe{std::mem::zeroed()};if unsafe{libc::fstat(fd,&mut stat)}<0{return Err(errno::last());}
 if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Ok(None);}
 let Some(configuration)=configuration()?else{return Ok(None)};
 Description::new(unsafe{BorrowedFd::borrow_raw(fd)},flags,configuration.store.clone()).map(Some)
}
pub(crate) fn adopt_kernel(fd:BorrowedFd<'_>,flags:u64)->Result<Option<Arc<Description>>,Errno>{adopt(fd.as_raw_fd(),flags)}
pub(crate) fn configured_store()->Result<Option<Arc<Store>>,Errno>{configuration().map(|configuration|configuration.map(|configuration|configuration.store.clone()))}
pub(super) fn verity_stat(stat:&libc::stat)->Result<Option<bool>,Errno>{
 if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Ok(None);}
 let Some(configuration)=configuration()?else{return Ok(None)};
 let identity=Identity{dev:stat.st_dev as u32 as u64,ino:stat.st_ino,generation:stat.st_gen,birth_seconds:stat.st_birthtime,birth_nanoseconds:stat.st_birthtime_nsec};
 configuration.store.lookup(identity).map(|metadata|Some(metadata.is_some())).map_err(core)
}
pub struct Export{pub transport:aim_binder_host::regular_file::Export,pub backing_fd:i32,_backing:super::fdtab::Pinned,_description:Arc<Description>}
pub fn export(backing:super::fdtab::Pinned,description:Arc<Description>)->Result<Export,Errno>{
 let fd=backing.descriptor().as_raw_fd();
 let identity=Identity::from_fd(backing.descriptor()).map_err(io)?;if identity!=description.identity{return Err(errno::EBADF);}
 let mut stat:libc::stat=unsafe{std::mem::zeroed()};if unsafe{libc::fstat(fd,&mut stat)}<0{return Err(errno::last());}
 super::attrs::apply(super::attrs::Host::Fd(fd),||crate::xrt::fd_path(fd).and_then(|path|vfs::guest_path_of_host(std::path::Path::new(&path))).unwrap_or_default(),&mut stat);
 let metadata=aim_binder_host::wire::RegularMetadata{flags:description.flags,uid:stat.st_uid,gid:stat.st_gid,identity:identity.to_bytes(),writer:description.writer.is_some()};
 let writer_fd=description.writer.as_ref().map(|writer|writer.descriptor().as_raw_fd());
 Ok(Export{transport:aim_binder_host::regular_file::Export{metadata,writer_fd},backing_fd:fd,_backing:backing,_description:description})
}
pub fn restore(fd:i32,flags:u64,writer_fd:Option<i32>)->Result<Arc<Description>,Errno>{
 let Some(configuration)=configuration()?else{return Err(errno::EOPNOTSUPP)};
 let source=unsafe{BorrowedFd::borrow_raw(fd)};let identity=Identity::from_fd(source).map_err(io)?;
 let inode=Inode::open(&configuration.leases,source).map_err(io)?;
 let writer=if let Some(writer_fd)=writer_fd{
  let carrier=PrivateFd::allocate(||{
   let copy=unsafe{libc::fcntl(writer_fd,libc::F_DUPFD_CLOEXEC,0)};
   if copy<0{return Err(std::io::Error::last_os_error());}Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(copy)})
  }).map_err(io)?;
  Some(inode.adopt_private_writer(carrier).map_err(io)?)
 }else{None};
 Ok(Arc::new(Description{flags,identity,writer,store:configuration.store.clone(),inode}))
}
pub fn import(fd:i32,metadata:&aim_binder_host::wire::RegularMetadata,writer:Option<aim_binder_host::regular_file::WriterPort>)->Result<Arc<Description>,Errno>{
 let Some(configuration)=configuration()?else{return Err(errno::EOPNOTSUPP)};
 let source=unsafe{BorrowedFd::borrow_raw(fd)};let identity=Identity::from_fd(source).map_err(io)?;
 if identity.to_bytes()!=metadata.identity||metadata.flags&super::fs::O_PATH!=0||metadata.writer!=matches!(metadata.flags&3,1|2)||metadata.writer!=writer.is_some(){return Err(71);}
 let actual=unsafe{libc::fcntl(fd,libc::F_GETFL)};if actual<0{return Err(errno::last());}let physical=if metadata.flags&3==3{libc::O_RDWR}else{(metadata.flags&3)as i32};if actual&libc::O_ACCMODE!=physical{return Err(71);}
 let inode=Inode::open(&configuration.leases,source).map_err(io)?;
 let writer=if let Some(writer)=writer{
  let carrier=PrivateFd::allocate(||{
   let fd=aim_binder_host::mach::port_to_fd(writer.as_port()).ok_or_else(||std::io::Error::from_raw_os_error(libc::EIO))?;
   Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})
  }).map_err(io)?;
  let lease=inode.adopt_private_writer(carrier).map_err(io)?;
  if configuration.store.lookup(identity).map_err(core)?.is_some(){return Err(errno::EPERM);}Some(lease)
 }else{None};
 Ok(Arc::new(Description{flags:metadata.flags,identity,writer,store:configuration.store.clone(),inode}))
}

unsafe extern "C"{static mach_task_self_:u32;fn mach_vm_write(task:u32,address:u64,data:usize,length:u32)->i32;}
impl Description{
 pub fn seek(&self,fd:BorrowedFd<'_>,offset:i64,whence:i32)->i64{
  let guard=match self.inode.offset_lock(){Ok(guard)=>guard,Err(error)=>return -(io(error)as i64)};
  let result=errno::check(unsafe{libc::lseek(fd.as_raw_fd(),offset,whence)}as i64);drop(guard);result
 }
 pub fn rw(&self,fd:BorrowedFd<'_>,iov:&[libc::iovec],position:Option<i64>,write:bool)->i64{
  match self.rw_result(fd,iov,position,write){Ok(result)=>result,Err(error)=>-(error as i64)}
 }
 fn rw_result(&self,fd:BorrowedFd<'_>,iov:&[libc::iovec],position:Option<i64>,write:bool)->Result<i64,Errno>{
  if position.is_some_and(|position|position<0){return Err(errno::EINVAL);}
  if self.flags&3==3||write&&self.flags&3==0||!write&&self.flags&3==1{return Err(errno::EBADF);}
  if Identity::from_fd(fd).map_err(io)?!=self.identity{return Err(errno::EBADF);}
  let _offset=if position.is_none(){Some(self.inode.offset_lock().map_err(io)?)}else{None};
  let admission=self.store.lock_inode(&fd).map_err(core)?;
  let enabled=admission.enabled().map_err(core)?;
  if write&&enabled.is_some(){return Err(errno::EPERM);}
  let Some(metadata)=enabled else{
   drop(admission);
   let result=unsafe{match(position,write){(Some(position),true)=>libc::pwritev(fd.as_raw_fd(),iov.as_ptr(),iov.len()as i32,position),(Some(position),false)=>libc::preadv(fd.as_raw_fd(),iov.as_ptr(),iov.len()as i32,position),(None,true)=>libc::writev(fd.as_raw_fd(),iov.as_ptr(),iov.len()as i32),(None,false)=>libc::readv(fd.as_raw_fd(),iov.as_ptr(),iov.len()as i32)}};
   return if result<0{Err(errno::last())}else{Ok(result as i64)};
  };
  let original=if let Some(position)=position{position}else{let position=unsafe{libc::lseek(fd.as_raw_fd(),0,libc::SEEK_CUR)};if position<0{return Err(errno::last());}position};
  let mut offset=original as u64;let mut total=0i64;let block=metadata.descriptor().options().block_size as u64;
  for target in iov{let mut copied=0usize;
   while copied<target.iov_len{
    let length=(target.iov_len-copied).min((block-offset%block)as usize);
    let bytes=match metadata.verify_range(&fd,offset,length).map_err(core){Ok(bytes)=>bytes,Err(error)=>return if total>0{Ok(total)}else{Err(error)}};
    if bytes.is_empty(){return Ok(total);}
    let result=unsafe{mach_vm_write(mach_task_self_,(target.iov_base as u64)+copied as u64,bytes.as_ptr()as usize,bytes.len()as u32)};
    if result!=0{return if total>0{Ok(total)}else{Err(14)}};
    total+=bytes.len()as i64;copied+=bytes.len();offset+=bytes.len()as u64;
    if position.is_none()&&unsafe{libc::lseek(fd.as_raw_fd(),offset as i64,libc::SEEK_SET)}<0{return Err(errno::last());}
   }
  }Ok(total)
 }
}

#[cfg(test)]
mod offset_tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn scratch_without_verity_owner_is_optional_but_declared_stores_are_strict(){
        let root=std::env::temp_dir().join(format!("aim-verity-config-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        struct Directory(PathBuf);impl Drop for Directory{fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}
        let _directory=Directory(root.clone());
        assert!(load_configuration(&root).unwrap().is_none());
        let owner=root.join("verity-control-owner");std::fs::write(&owner,b"declared owner").unwrap();
        assert!(matches!(load_configuration(&root),Err(errno::ENOENT)));std::fs::remove_file(owner).unwrap();
        let locator=root.join("fs-verity-root");std::os::unix::fs::symlink(root.join("missing"),&locator).unwrap();
        assert!(load_configuration(&root).is_err());std::fs::remove_file(&locator).unwrap();
        std::fs::write(&locator,b"corrupt").unwrap();assert!(matches!(load_configuration(&root),Err(errno::EINVAL)));
        let proof=root.join("proof");std::fs::create_dir(&proof).unwrap();let proof=std::fs::canonicalize(proof).unwrap();
        let mut bytes=b"AIMVRTROOT01\0".to_vec();bytes.extend_from_slice(proof.as_os_str().as_bytes());std::fs::write(&locator,bytes).unwrap();
        assert!(load_configuration(&root).unwrap().is_some());
    }

    #[test]
    fn verified_duplicate_reads_are_disjoint_and_pread_does_not_move_position() {
        let (_view, root) = vfs::test_view();
        let path = root.join("data/verified-offsets");
        let expected = (0..128u64).flat_map(u64::to_le_bytes).collect::<Vec<_>>();
        std::fs::write(&path, &expected).unwrap();
        let source = File::open(&path).unwrap();
        let store = Arc::new(Store::new(&root.join("offset-proofs"), &root.join("offset-locks")).unwrap());
        let admission = store.lock_inode(&source).unwrap();
        let enable = admission.begin_enable().unwrap(); drop(admission);
        let prepared = enable.build(aim_storage::fsverity::BuildOptions::new(1, 4096, vec![], 4096, 4096).unwrap(), &[], || false).unwrap();
        enable.commit(prepared).unwrap();
        let description = Description::new(source.as_fd(), 0, store).unwrap();
        let mut workers = Vec::new();
        for _ in 0..4 {
            let fd = source.try_clone().unwrap(); let description = description.clone();
            workers.push(std::thread::spawn(move || {
                let mut values = Vec::new();
                loop {
                    let mut bytes = [0u8; 8];
                    let iov = libc::iovec { iov_base: bytes.as_mut_ptr().cast(), iov_len: bytes.len() };
                    let count = description.rw(fd.as_fd(), &[iov], None, false);
                    if count == 0 { break; } assert_eq!(count, 8);
                    values.push(u64::from_le_bytes(bytes));
                }
                values
            }));
        }
        let results=workers.into_iter().map(|worker|worker.join()).collect::<Vec<_>>();
        let mut values=results.into_iter().flat_map(Result::unwrap).collect::<Vec<_>>();
        values.sort_unstable(); assert_eq!(values, (0..128).collect::<Vec<_>>());
        assert_eq!(description.seek(source.as_fd(), 16, libc::SEEK_SET), 16);
        let mut bytes = [0u8; 8]; let iov = libc::iovec { iov_base: bytes.as_mut_ptr().cast(), iov_len: 8 };
        assert_eq!(description.rw(source.as_fd(), &[iov], Some(0), false), 8);
        assert_eq!(u64::from_le_bytes(bytes), 0);
        assert_eq!(description.seek(source.as_fd(), 0, libc::SEEK_CUR), 16);
        assert_eq!(description.rw(source.as_fd(), &[iov], None, false), 8);
        assert_eq!(u64::from_le_bytes(bytes), 2);
    }
}
