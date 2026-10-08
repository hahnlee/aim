//! Linux sock_alloc/sockfs_setattr ownership of an actual socket inode.
use super::*;
use crate::errno::{Errno,EIO};
use super::super::attrs::Attr;
use std::{fs::{File,OpenOptions},io::{Read,Write},os::{fd::{FromRawFd,IntoRawFd,AsRawFd},unix::fs::OpenOptionsExt},path::{Path,PathBuf}};
fn io_error(error:std::io::Error)->Errno{error.raw_os_error().map(errno::from_darwin).unwrap_or(EIO)}
struct Private(Option<File>);
impl Private {
    fn new(file:File)->Self{Self(Some(unsafe{File::from_raw_fd(fdtab::hide(file.into_raw_fd()))}))}
    fn file(&self)->&File{self.0.as_ref().unwrap()}
    fn file_mut(&mut self)->&mut File{self.0.as_mut().unwrap()}
}
impl Drop for Private{fn drop(&mut self){let file=self.0.take().unwrap();let fd=file.as_raw_fd();drop(file);fdtab::unhide(fd);}}
struct Locked(Private);
impl Drop for Locked{fn drop(&mut self){unsafe{libc::flock(self.0.file().as_raw_fd(),libc::LOCK_UN);}}}
impl AsRawFd for Private{fn as_raw_fd(&self)->i32{self.file().as_raw_fd()}}
impl Write for Private {
    fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{self.file_mut().write(bytes)}
    fn flush(&mut self)->std::io::Result<()>{self.file_mut().flush()}
}
fn directory()->PathBuf {
    vfs::runtime_dir().map(|path|path.join("socket-inodes"))
        .unwrap_or_else(||std::env::temp_dir().join(format!("aim-socket-inodes-{}",unsafe{libc::getuid()})))
}
fn lock(cookie:u64)->Result<(Locked,PathBuf),Errno>{
    let dir=directory();std::fs::create_dir_all(&dir).map_err(io_error)?;
    let path=dir.join(format!("{cookie:x}"));
    let file=Private::new(OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).mode(0o600).open(path.with_extension("lock")).map_err(io_error)?);
    loop{if unsafe{libc::flock(file.file().as_raw_fd(),libc::LOCK_EX)}==0{break;}let error=errno::last();if error!=errno::EINTR{return Err(error);}}
    Ok((Locked(file),path))
}
fn read(path:&Path,cookie:u64,inode:u64)->Result<Option<Attr>,Errno>{
    let file=match OpenOptions::new().read(true).custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).open(path){Ok(file)=>file,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(error)=>return Err(io_error(error))};
    let mut file=Private::new(file);let mut bytes=Vec::new();file.file_mut().read_to_end(&mut bytes).map_err(io_error)?;
    let record=aim_storage::socket_inode::decode(&bytes,aim_storage::socket_inode::Identity{inode,cookie}).map_err(io_error)?;
    Ok(Some(Attr{uid:Some(record.uid),gid:Some(record.gid),mode:Some(record.mode)}))
}
fn write(path:&Path,cookie:u64,inode:u64,attributes:Attr)->Result<(),Errno>{
    aim_storage::socket_inode::publish(path,aim_storage::socket_inode::Identity{inode,cookie},
        aim_storage::socket_inode::Record{uid:attributes.uid.ok_or(EIO)?,gid:attributes.gid.ok_or(EIO)?,mode:attributes.mode.ok_or(EIO)?},Private::new).map_err(io_error)
}

fn copied(fd:i32)->Result<Private,Errno>{let copy=unsafe{libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,0)};if copy<0{Err(errno::last())}else{Ok(Private::new(unsafe{File::from_raw_fd(copy)}))}}
pub(super) fn allocated(fd:i32)->Result<(),Errno>{
    let _fds=super::super::fork::spawn::own_fds();let copy=copied(fd)?;
    let(inode,cookie)=super::proc_table::raw_identity(copy.file().as_raw_fd())?;
    let(_lock,path)=lock(cookie)?;let identity=super::super::cred::current();
    write(&path,cookie,inode,Attr{uid:Some(identity.uid[3]),gid:Some(identity.gid[3]),mode:Some(0o777)})
}
pub(crate) fn is_socket(fd:i32)->Result<bool,Errno>{
    if super::hidden_socket(fd)||super::super::fuse_device::is_typed(fd){return Ok(false);}
    let mut host:libc::stat=unsafe{std::mem::zeroed()};
    if unsafe{libc::fstat(fd,&mut host)}<0{return Err(errno::last());}
    Ok(host.st_mode&libc::S_IFMT==libc::S_IFSOCK)
}
pub(crate) fn stat(fd:i32)->Option<Result<libc::stat,Errno>>{
    match is_socket(fd){Ok(false)=>None,Err(error)=>Some(Err(error)),Ok(true)=>Some(with(fd,|stat,_|Ok(*stat)))}
}
pub(crate) fn with<T>(fd:i32,action:impl FnOnce(&libc::stat,&mut Attr)->Result<T,Errno>)->Result<T,Errno>{
    let _fds=super::super::fork::spawn::own_fds();let copy=copied(fd)?;
    let(inode,cookie)=super::proc_table::raw_identity(copy.file().as_raw_fd())?;let(_lock,path)=lock(cookie)?;
    let old=read(&path,cookie,inode)?.ok_or(61)?;
    let mut attributes=old;let mut stat:libc::stat=unsafe{std::mem::zeroed()};
    if unsafe{libc::fstat(copy.file().as_raw_fd(),&mut stat)}<0{return Err(errno::last());}
    stat.st_ino=inode;stat.st_uid=attributes.uid.ok_or(EIO)?;stat.st_gid=attributes.gid.ok_or(EIO)?;stat.st_mode=libc::S_IFSOCK|attributes.mode.ok_or(EIO)? as u16;
    let result=action(&stat,&mut attributes)?;
    if attributes!=old{write(&path,cookie,inode,attributes)?;}
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::OwnedFd;
    fn owner(fd:i32)->libc::stat{stat(fd).unwrap().unwrap()}
    #[test]
    fn actual_udp_resolver_probe_chown_permissions_and_shared_inode_transfers(){
        let(_guard,_root)=vfs::test_view();
        let fd=super::super::socket([2,2|0x80000,17,0,0,0]);assert!(fd>=0,"UDP allocation {fd}");
        let socket=unsafe{OwnedFd::from_raw_fd(fd as i32)};
        let initial=owner(socket.as_raw_fd());
        let mut caller=super::super::super::cred::current();caller.cap_eff=0;caller.cap_perm=0;
        assert_eq!(super::super::super::fsops::fchown_as([fd as u64,initial.st_uid as u64,u32::MAX as u64,0,0,0],&caller),0);
        caller.groups.push(7777);
        assert_eq!(super::super::super::fsops::fchown_as([fd as u64,u32::MAX as u64,7777,0,0,0],&caller),0);
        assert_eq!(owner(socket.as_raw_fd()).st_gid,7777);
        assert_eq!(super::super::super::fsops::fchown_as([fd as u64,u32::MAX as u64,7778,0,0,0],&caller),-(errno::EPERM as i64));
        let mut stranger=caller.clone();stranger.uid=[40001;4];stranger.gid=[40001;4];stranger.groups.clear();
        assert_eq!(super::super::super::fsops::fchown_as([fd as u64,40001,u32::MAX as u64,0,0,0],&stranger),-(errno::EPERM as i64));
        let mut resolver=caller;resolver.cap_eff=1;
        assert_eq!(super::super::super::fsops::fchown_as([fd as u64,1073,u32::MAX as u64,0,0,0],&resolver),0);
        assert_eq!(owner(socket.as_raw_fd()).st_uid,1073);
        let mark=100u32;
        assert_eq!(super::super::setsockopt([fd as u64,1,36,&mark as *const u32 as u64,4,0]),0);
        let mut address=[0u8;16];address[..2].copy_from_slice(&2u16.to_le_bytes());address[4..8].copy_from_slice(&[127,0,0,1]);
        assert_eq!(super::super::connect([fd as u64,address.as_ptr() as u64,16,0,0,0]),0);
        let mut source=[0u8;16];let mut len=16u32;
        assert_eq!(super::super::getsockname([fd as u64,source.as_mut_ptr() as u64,&mut len as *mut u32 as u64,0,0,0]),0);
        assert_ne!(&source[4..8],&[0;4]);
        let duplicate=unsafe{libc::dup(socket.as_raw_fd())};assert!(duplicate>=0);
        let duplicate=unsafe{OwnedFd::from_raw_fd(duplicate)};
        assert_eq!((owner(duplicate.as_raw_fd()).st_ino,owner(duplicate.as_raw_fd()).st_uid),(initial.st_ino,1073));
        // Actual SCM_RIGHTS transfers the same kernel open socket description.
        let mut channel=[0;2];assert_eq!(unsafe{libc::socketpair(libc::AF_UNIX,libc::SOCK_STREAM,0,channel.as_mut_ptr())},0);
        let sender=unsafe{OwnedFd::from_raw_fd(channel[0])};let receiver=unsafe{OwnedFd::from_raw_fd(channel[1])};
        let mut byte=1u8;let mut iov=libc::iovec{iov_base:(&mut byte as *mut u8).cast(),iov_len:1};
        let size=unsafe{libc::CMSG_SPACE(4)} as usize;let mut control=vec![0u8;size];
        let mut message:libc::msghdr=unsafe{std::mem::zeroed()};message.msg_iov=&mut iov;message.msg_iovlen=1;message.msg_control=control.as_mut_ptr().cast();message.msg_controllen=size as _;
        let header=unsafe{libc::CMSG_FIRSTHDR(&message)};unsafe{(*header).cmsg_level=libc::SOL_SOCKET;(*header).cmsg_type=libc::SCM_RIGHTS;(*header).cmsg_len=libc::CMSG_LEN(4);(libc::CMSG_DATA(header) as *mut i32).write_unaligned(socket.as_raw_fd());}
        assert_eq!(unsafe{libc::sendmsg(sender.as_raw_fd(),&message,0)},1);
        control.fill(0);message.msg_controllen=size as _;assert_eq!(unsafe{libc::recvmsg(receiver.as_raw_fd(),&mut message,0)},1);
        let transferred=unsafe{(libc::CMSG_DATA(libc::CMSG_FIRSTHDR(&message)) as *const i32).read_unaligned()};
        let transferred=unsafe{OwnedFd::from_raw_fd(transferred)};
        assert_eq!((owner(transferred.as_raw_fd()).st_ino,owner(transferred.as_raw_fd()).st_uid),(initial.st_ino,1073));
        assert_eq!(super::super::super::fsops::fchown_as([transferred.as_raw_fd() as u64,40001,u32::MAX as u64,0,0,0],&stranger),-(errno::EPERM as i64));
        // The original process can disappear while duplicated/transferred
        // descriptors still retain the exact inode owner.
        drop(socket);assert_eq!(owner(transferred.as_raw_fd()).st_uid,1073);
        fdtab::on_close(fd as i32);
    }
}

#[cfg(test)]
mod creator_tests {
    use super::*;
    use std::os::fd::OwnedFd;
    #[test]
    fn inherited_native_listener_and_unknown_adoption_private_fds_and_error_propagation(){
        let(_guard,_root)=vfs::test_view();
        let socket=unsafe{libc::socket(libc::AF_UNIX,libc::SOCK_STREAM,0)};assert!(socket>=0);
        let socket=unsafe{OwnedFd::from_raw_fd(socket)};
        assert_eq!(stat(socket.as_raw_fd()).unwrap().unwrap_err(),61);
        let runtime=vfs::runtime_dir().unwrap();
        aim_storage::socket_inode::allocated(runtime,socket.as_raw_fd(),0,0).unwrap();
        let inherited=stat(socket.as_raw_fd()).unwrap().unwrap();assert_eq!((inherited.st_uid,inherited.st_gid,inherited.st_mode&0o777),(0,0,0o777));
        let mut receiver=super::super::super::cred::current();receiver.uid=[40001;4];receiver.gid=[40001;4];receiver.cap_eff=0;receiver.cap_perm=0;
        assert_eq!(super::super::super::fsops::fchown_as([socket.as_raw_fd() as u64,40001,u32::MAX as u64,0,0,0],&receiver),-(errno::EPERM as i64));
        let(inode,cookie)=super::super::proc_table::raw_identity(socket.as_raw_fd()).unwrap();let(_lock,path)=lock(cookie).unwrap();
        let hidden=_lock.0.file().as_raw_fd();assert!(fdtab::is_hidden(hidden));
        assert_eq!(super::super::super::fs::close([hidden as u64,0,0,0,0,0]),-(errno::EBADF as i64));drop(_lock);assert!(!fdtab::is_hidden(hidden));
        // Atomic publication failures leave the previous actual owner intact.
        let broken=path.with_extension("owned-directory-error");std::fs::create_dir(&broken).unwrap();
        assert!(write(&broken,cookie,inode,Attr{uid:Some(40001),gid:Some(40001),mode:Some(0o600)}).is_err());
        assert_eq!(stat(socket.as_raw_fd()).unwrap().unwrap().st_uid,0);
        std::fs::remove_dir(broken).unwrap();
        let directory=directory();assert!(!std::fs::read_dir(directory).unwrap().any(|entry|entry.unwrap().path().extension().is_some_and(|value|value=="new")));
        assert!(aim_storage::socket_inode::identity(-1).is_err());
    }
}
