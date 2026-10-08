//! Cross-process inode admission and open-description writer leases.
//! Admission is always acquired before the writer lock. Only final descriptor
//! close releases a lease: explicitly unlocking would revoke surviving aliases.
use crate::private_fd::PrivateFd;
use std::{fs::{self,File,OpenOptions},io,os::{fd::{AsFd,AsRawFd,BorrowedFd,FromRawFd,OwnedFd},unix::{ffi::OsStrExt,fs::OpenOptionsExt}},path::{Path,PathBuf},sync::Arc};
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Identity {
    pub dev:u64,
    pub ino:u64,
    pub generation:u32,
    pub birth_seconds:i64,
    pub birth_nanoseconds:i64,
}
impl Identity {
    pub fn from_fd(fd:BorrowedFd<'_>)->io::Result<Self>{
        let mut stat:libc::stat=unsafe{std::mem::zeroed()};
        if unsafe{libc::fstat(fd.as_raw_fd(),&mut stat)}<0{return Err(io::Error::last_os_error());}
        Ok(Self{dev:stat.st_dev as u32 as u64,ino:stat.st_ino,generation:stat.st_gen,birth_seconds:stat.st_birthtime,birth_nanoseconds:stat.st_birthtime_nsec})
    }
    pub fn name(self)->String{format!("{:x}-{:x}-{:x}-{:x}-{:x}",self.dev,self.ino,self.generation,self.birth_seconds,self.birth_nanoseconds)}
    pub fn to_bytes(self)->[u8;36]{let mut bytes=[0;36];bytes[..8].copy_from_slice(&self.dev.to_le_bytes());bytes[8..16].copy_from_slice(&self.ino.to_le_bytes());bytes[16..20].copy_from_slice(&self.generation.to_le_bytes());bytes[20..28].copy_from_slice(&self.birth_seconds.to_le_bytes());bytes[28..36].copy_from_slice(&self.birth_nanoseconds.to_le_bytes());bytes}
    pub fn from_bytes(bytes:&[u8])->io::Result<Self>{if bytes.len()!=36{return Err(io::Error::from_raw_os_error(libc::EINVAL));}let identity=Self{dev:u64::from_le_bytes(bytes[..8].try_into().unwrap()),ino:u64::from_le_bytes(bytes[8..16].try_into().unwrap()),generation:u32::from_le_bytes(bytes[16..20].try_into().unwrap()),birth_seconds:i64::from_le_bytes(bytes[20..28].try_into().unwrap()),birth_nanoseconds:i64::from_le_bytes(bytes[28..36].try_into().unwrap())};if identity.dev>u32::MAX as u64||!(0..1_000_000_000).contains(&identity.birth_nanoseconds){return Err(io::Error::from_raw_os_error(libc::EINVAL));}Ok(identity)}
}
struct Inner {source:PrivateFd,directory:PrivateFd,path:PathBuf,identity:Identity}
#[derive(Clone)]
pub struct Inode(Arc<Inner>);
pub struct Admission {inode:Inode,_metadata:PrivateFd}
pub struct WriterLease {inode:Inode,descriptor:PrivateFd}
pub struct ExclusiveLease {inode:Inode,descriptor:PrivateFd}
pub type EnableSlot = ExclusiveLease;
fn duplicate(fd:BorrowedFd<'_>)->io::Result<PrivateFd>{PrivateFd::allocate(||{let result=unsafe{libc::fcntl(fd.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if result<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(result)})}})}
fn locked(fd:BorrowedFd<'_>,operation:i32)->io::Result<()>{loop{if unsafe{libc::flock(fd.as_raw_fd(),operation)}==0{return Ok(());}let error=io::Error::last_os_error();if error.kind()==io::ErrorKind::Interrupted{continue;}if operation&libc::LOCK_NB!=0&&error.kind()==io::ErrorKind::WouldBlock{return Err(io::Error::from_raw_os_error(libc::EBUSY));}return Err(error);}}
impl Inode {
    pub fn open(directory:&Path,source:BorrowedFd<'_>)->io::Result<Self>{
        let source=duplicate(source)?;
        let mut stat:libc::stat=unsafe{std::mem::zeroed()};
        if unsafe{libc::fstat(source.as_raw_fd(),&mut stat)}<0{return Err(io::Error::last_os_error());}
        if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        let identity=Identity::from_fd(source.as_fd())?;
        fs::create_dir_all(directory)?;
        if fs::symlink_metadata(directory)?.file_type().is_symlink(){return Err(io::Error::from_raw_os_error(libc::ELOOP));}
        let path=fs::canonicalize(directory)?;
        let name=std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_|io::Error::from_raw_os_error(libc::EINVAL))?;
        let directory=PrivateFd::allocate(||{let fd=unsafe{libc::open(name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}})?;
        Ok(Self(Arc::new(Inner{source,directory,path,identity})))
    }
    pub fn identity(&self)->Identity{self.0.identity}
    pub fn source(&self)->BorrowedFd<'_>{self.0.source.as_fd()}
    pub fn directory(&self)->&Path{&self.0.path}
    fn lock_file(&self,suffix:&str)->io::Result<PrivateFd>{
        let name=std::ffi::CString::new(format!("{}.{}",self.identity().name(),suffix)).unwrap();
        let fd=PrivateFd::allocate(||{let fd=unsafe{libc::openat(self.0.directory.as_raw_fd(),name.as_ptr(),libc::O_RDWR|libc::O_CREAT|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}})?;let mut stat:libc::stat=unsafe{std::mem::zeroed()};
        if unsafe{libc::fstat(fd.as_raw_fd(),&mut stat)}<0{return Err(io::Error::last_os_error());}
        if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        Ok(fd)
    }
    pub fn admission(&self)->io::Result<Admission>{
        if Identity::from_fd(self.source())?!=self.identity(){return Err(io::Error::from_raw_os_error(libc::ESTALE));}
        let metadata=self.lock_file("admission")?;locked(metadata.as_fd(),libc::LOCK_EX)?;Ok(Admission{inode:self.clone(),_metadata:metadata})}
    /// Adopt the actual carrier under admission and verify its lock-file inode.
    pub fn adopt_writer(&self,carrier:OwnedFd)->io::Result<WriterLease>{
        self.adopt_private_writer(PrivateFd::adopt(carrier)?)
    }
    pub fn adopt_private_writer(&self,carrier:PrivateFd)->io::Result<WriterLease>{
        let admission=self.admission()?;let expected=self.lock_file("writers")?;
        if Identity::from_fd(carrier.as_fd())?!=Identity::from_fd(expected.as_fd())?{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        locked(carrier.as_fd(),libc::LOCK_SH|libc::LOCK_NB)?;
        Ok(WriterLease{inode:admission.inode.clone(),descriptor:carrier})
    }
}
impl Admission {
    pub fn identity(&self)->Identity{self.inode.identity()}
    pub fn source(&self)->BorrowedFd<'_>{self.inode.source()}
    pub fn writer(&self)->io::Result<WriterLease>{let descriptor=self.inode.lock_file("writers")?;locked(descriptor.as_fd(),libc::LOCK_SH|libc::LOCK_NB)?;Ok(WriterLease{inode:self.inode.clone(),descriptor})}
    pub fn enable_slot(&self)->io::Result<EnableSlot>{let descriptor=self.inode.lock_file("enable")?;locked(descriptor.as_fd(),libc::LOCK_EX|libc::LOCK_NB)?;Ok(ExclusiveLease{inode:self.inode.clone(),descriptor})}
    pub fn exclusive(&self)->io::Result<ExclusiveLease>{let descriptor=self.inode.lock_file("writers")?;locked(descriptor.as_fd(),libc::LOCK_EX|libc::LOCK_NB)?;Ok(ExclusiveLease{inode:self.inode.clone(),descriptor})}
}
impl WriterLease {
    pub fn identity(&self)->Identity{self.inode.identity()}
    pub fn descriptor(&self)->BorrowedFd<'_>{self.descriptor.as_fd()}
    pub fn try_clone(&self)->io::Result<Self>{Ok(Self{inode:self.inode.clone(),descriptor:self.descriptor.try_clone()?})}
    pub fn into_private_fd(self)->PrivateFd{self.descriptor}
    pub fn into_fd(self)->io::Result<OwnedFd>{self.descriptor.into_fd()}
}
impl ExclusiveLease {
    pub fn identity(&self)->Identity{self.inode.identity()}
    pub fn descriptor(&self)->BorrowedFd<'_>{self.descriptor.as_fd()}
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {path:PathBuf,file:File,inode:Inode}
    impl Fixture {fn new()->Self{static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);let path=std::env::temp_dir().join(format!("aim-inode-lease-{}-{}",std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));fs::create_dir(&path).unwrap();let file=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("source")).unwrap();let inode=Inode::open(&path.join("locks"),file.as_fd()).unwrap();Self{path,file,inode}}}
    impl Drop for Fixture {fn drop(&mut self){fs::remove_dir_all(&self.path).unwrap();}}
    fn busy(result:io::Result<ExclusiveLease>){assert_eq!(result.err().unwrap().raw_os_error(),Some(libc::EBUSY));}
    #[test]
    fn actual_identity_round_trip_rejects_invalid_fields_and_aliases_keep_lock(){
        let fixture=Fixture::new();
        let directory=File::open(&fixture.path).unwrap();
        assert_eq!(Inode::open(fixture.inode.directory(),directory.as_fd()).err().unwrap().raw_os_error(),Some(libc::EINVAL));
        let socket=unsafe{OwnedFd::from_raw_fd(libc::socket(libc::AF_UNIX,libc::SOCK_STREAM,0))};
        assert_eq!(Inode::open(fixture.inode.directory(),socket.as_fd()).err().unwrap().raw_os_error(),Some(libc::EINVAL));
        let identity=Identity::from_fd(fixture.file.as_fd()).unwrap();assert_eq!(identity,fixture.inode.identity());assert_eq!(Identity::from_bytes(&identity.to_bytes()).unwrap(),identity);
        let mut synthetic=identity;synthetic.dev=u32::MAX as u64;assert_eq!(Identity::from_bytes(&synthetic.to_bytes()).unwrap().dev,(-1i32 as u32) as u64);
        synthetic.dev=u32::MAX as u64+1;assert!(Identity::from_bytes(&synthetic.to_bytes()).is_err());synthetic=identity;synthetic.birth_nanoseconds=1_000_000_000;assert!(Identity::from_bytes(&synthetic.to_bytes()).is_err());
        let writer=fixture.inode.admission().unwrap().writer().unwrap();let alias=writer.try_clone().unwrap();drop(writer);
        busy(fixture.inode.admission().unwrap().exclusive());let carrier=alias.into_private_fd();let writer=fixture.inode.adopt_private_writer(carrier).unwrap();busy(fixture.inode.admission().unwrap().exclusive());drop(writer);
        assert!(fixture.inode.admission().unwrap().exclusive().is_ok());
        assert!(fixture.inode.adopt_private_writer(duplicate(fixture.file.as_fd()).unwrap()).is_err());
    }
    #[test]
    fn exclusive_enable_build_never_blocks_a_writer_holding_admission(){
        let fixture=Fixture::new();let admission=fixture.inode.admission().unwrap();let exclusive=admission.exclusive().unwrap();let slot=admission.enable_slot().unwrap();drop(admission);
        let admission=fixture.inode.admission().unwrap();assert_eq!(admission.writer().err().unwrap().raw_os_error(),Some(libc::EBUSY));busy(admission.enable_slot());drop(admission);
        let reacquired=fixture.inode.admission().unwrap();drop(reacquired);drop(slot);drop(exclusive);
        assert!(fixture.inode.admission().unwrap().writer().is_ok());
    }
    fn byte(fd:i32,value:u8){assert_eq!(unsafe{libc::write(fd,(&value as *const u8).cast(),1)},1);}
    fn read_byte(fd:i32)->u8{let mut value=0;assert_eq!(unsafe{libc::read(fd,(&mut value as *mut u8).cast(),1)},1);value}
    fn pipe()->[OwnedFd;2]{let mut fds=[0;2];assert_eq!(unsafe{libc::pipe(fds.as_mut_ptr())},0);unsafe{[OwnedFd::from_raw_fd(fds[0]),OwnedFd::from_raw_fd(fds[1])]}}
    #[test]
    fn actual_child_exit_and_crash_release_writer_only_on_final_close(){
        for crash in [false,true]{
            let fixture=Fixture::new();let ready=pipe();let exit=pipe();let child=unsafe{libc::fork()};assert!(child>=0);
            if child==0{
                let writer=match fixture.inode.admission().and_then(|admission|admission.writer()){Ok(writer)=>writer,Err(_)=>unsafe{libc::_exit(31)}};
                let alias=match writer.try_clone(){Ok(alias)=>alias,Err(_)=>unsafe{libc::_exit(32)}};drop(writer);
                unsafe{libc::write(ready[1].as_raw_fd(),b"r".as_ptr().cast(),1);}
                let mut command=0u8;unsafe{libc::read(exit[0].as_raw_fd(),(&mut command as *mut u8).cast(),1);}
                let _live=alias;unsafe{libc::_exit(0)}
            }
            assert_eq!(read_byte(ready[0].as_raw_fd()),b'r');busy(fixture.inode.admission().unwrap().exclusive());
            if crash{assert_eq!(unsafe{libc::kill(child,libc::SIGKILL)},0);}else{byte(exit[1].as_raw_fd(),b'e');}
            let mut status=0;assert_eq!(unsafe{libc::waitpid(child,&mut status,0)},child);
            if crash{assert!(libc::WIFSIGNALED(status));assert_eq!(libc::WTERMSIG(status),libc::SIGKILL);}else{assert!(libc::WIFEXITED(status));assert_eq!(libc::WEXITSTATUS(status),0);}
            assert!(fixture.inode.admission().unwrap().exclusive().is_ok());
        }
    }
    #[test]
    fn scm_carrier_keeps_actual_open_description_after_sender_close(){
        let fixture=Fixture::new();let writer=fixture.inode.admission().unwrap().writer().unwrap();let mut sockets=[0;2];assert_eq!(unsafe{libc::socketpair(libc::AF_UNIX,libc::SOCK_STREAM,0,sockets.as_mut_ptr())},0);
        let sender=unsafe{OwnedFd::from_raw_fd(sockets[0])};let receiver=unsafe{OwnedFd::from_raw_fd(sockets[1])};
        let mut value=1u8;let mut iov=libc::iovec{iov_base:(&mut value as *mut u8).cast(),iov_len:1};let size=unsafe{libc::CMSG_SPACE(4)} as usize;let mut control=vec![0u8;size];let mut message:libc::msghdr=unsafe{std::mem::zeroed()};message.msg_iov=&mut iov;message.msg_iovlen=1;message.msg_control=control.as_mut_ptr().cast();message.msg_controllen=size as _;
        let header=unsafe{libc::CMSG_FIRSTHDR(&message)};unsafe{(*header).cmsg_level=libc::SOL_SOCKET;(*header).cmsg_type=libc::SCM_RIGHTS;(*header).cmsg_len=libc::CMSG_LEN(4);(libc::CMSG_DATA(header) as *mut i32).write_unaligned(writer.descriptor().as_raw_fd());}
        assert_eq!(unsafe{libc::sendmsg(sender.as_raw_fd(),&message,0)},1);drop(writer);busy(fixture.inode.admission().unwrap().exclusive());
        control.fill(0);message.msg_controllen=size as _;assert_eq!(unsafe{libc::recvmsg(receiver.as_raw_fd(),&mut message,0)},1);let fd=unsafe{(libc::CMSG_DATA(libc::CMSG_FIRSTHDR(&message)) as *const i32).read_unaligned()};let lease=fixture.inode.adopt_writer(unsafe{OwnedFd::from_raw_fd(fd)}).unwrap();busy(fixture.inode.admission().unwrap().exclusive());drop(lease);assert!(fixture.inode.admission().unwrap().exclusive().is_ok());
    }
    #[test]
    fn canonical_directory_and_lock_nodes_reject_symlink_redirection(){
        let fixture=Fixture::new();let alias=fixture.path.join("alias");std::os::unix::fs::symlink(fixture.inode.directory(),&alias).unwrap();assert_eq!(Inode::open(&alias,fixture.file.as_fd()).err().unwrap().raw_os_error(),Some(libc::ELOOP));
        let target=fixture.inode.directory().join(format!("{}.writers",fixture.inode.identity().name()));std::os::unix::fs::symlink(fixture.path.join("source"),&target).unwrap();assert!(fixture.inode.admission().unwrap().writer().is_err());
    }
}
