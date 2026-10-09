//! Socket inode allocation shared by native init and the Linux syscall owner.
use std::{fs::{File,OpenOptions},io::{self,Write},os::{fd::{AsRawFd,RawFd},unix::fs::OpenOptionsExt},path::Path,sync::atomic::{AtomicU64,Ordering}};
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Identity{pub inode:u64,pub cookie:u64}
pub fn identity(fd:RawFd)->io::Result<Identity>{
    let mut bytes=[0u8;792];
    let size=unsafe{libc::proc_pidfdinfo(libc::getpid(),fd,3,bytes.as_mut_ptr().cast(),792)};
    if size<0{return Err(io::Error::last_os_error());}
    if size!=792{return Err(io::Error::from_raw_os_error(libc::EIO));}
    let cookie=u64::from_ne_bytes(bytes[160..168].try_into().unwrap());
    if cookie==0{return Err(io::Error::from_raw_os_error(libc::EIO));}
    Ok(Identity{inode:cookie.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(23),cookie})
}
/// A trusted allocation receipt travels with the actual socket capability.
/// The opaque Darwin address cookie alone is never an adoption credential.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Receipt{pub identity:Identity,pub generation:[u8;16]}
impl Receipt{
    pub fn mint(fd:RawFd)->io::Result<Self>{
        let identity=identity(fd)?;let mut generation=[0u8;16];
        loop{unsafe{libc::arc4random_buf(generation.as_mut_ptr().cast(),generation.len());}if generation!=[0;16]{break;}}
        Ok(Self{identity,generation})
    }
    pub fn name(self)->String{self.generation.iter().map(|byte|format!("{byte:02x}")).collect()}
    pub fn validate(self,fd:RawFd)->io::Result<()>{if identity(fd)?!=self.identity{return Err(io::Error::from_raw_os_error(libc::EINVAL));}Ok(())}
    pub fn to_bytes(self)->[u8;40]{let mut bytes=[0;40];bytes[..8].copy_from_slice(b"SOCKRC02");bytes[8..24].copy_from_slice(&self.generation);bytes[24..32].copy_from_slice(&self.identity.cookie.to_le_bytes());bytes[32..40].copy_from_slice(&self.identity.inode.to_le_bytes());bytes}
    pub fn from_bytes(bytes:&[u8])->io::Result<Self>{
        if bytes.len()!=40||&bytes[..8]!=b"SOCKRC02"{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        let generation=bytes[8..24].try_into().unwrap();let cookie=u64::from_le_bytes(bytes[24..32].try_into().unwrap());let inode=u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        if generation==[0;16]||cookie==0||inode!=cookie.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(23){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        Ok(Self{identity:Identity{inode,cookie},generation})
    }
}
#[derive(Clone,Copy)]
pub struct Record{pub uid:u32,pub gid:u32,pub mode:u32}
pub fn encode(receipt:Receipt,record:Record)->Vec<u8>{let mut bytes=b"SOCKIN02".to_vec();bytes.extend(receipt.to_bytes());bytes.extend(record.uid.to_le_bytes());bytes.extend(record.gid.to_le_bytes());bytes.extend(record.mode.to_le_bytes());bytes}
pub fn decode(bytes:&[u8],receipt:Receipt)->io::Result<Record>{
    if bytes.len()!=60||&bytes[..8]!=b"SOCKIN02"||Receipt::from_bytes(&bytes[8..48])?!=receipt{return Err(io::Error::from_raw_os_error(libc::EIO));}
    let mode=u32::from_le_bytes(bytes[56..60].try_into().unwrap());if mode&!0o7777!=0{return Err(io::Error::from_raw_os_error(libc::EIO));}
    Ok(Record{uid:u32::from_le_bytes(bytes[48..52].try_into().unwrap()),gid:u32::from_le_bytes(bytes[52..56].try_into().unwrap()),mode})
}
/// Caller wraps every opened fd in its actual kernel-private descriptor owner.
pub fn publish<H:Write+AsRawFd>(path:&Path,receipt:Receipt,record:Record,wrap:impl Fn(File)->H)->io::Result<()> {
    static NEXT:AtomicU64=AtomicU64::new(0);
    let(name,file)=loop{let name=path.with_extension(format!("{}.{}.new",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));match OpenOptions::new().create_new(true).write(true).custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).mode(0o600).open(&name){Ok(file)=>break(name,wrap(file)),Err(error)if error.kind()==io::ErrorKind::AlreadyExists=>continue,Err(error)=>return Err(error)}};
    let mut cleanup=Temporary(Some(name.clone()));let mut file=file;file.write_all(&encode(receipt,record))?;
    if unsafe{libc::fsync(file.as_raw_fd())}<0{return Err(io::Error::last_os_error());}
    std::fs::rename(name,path)?;cleanup.0=None;let directory=wrap(File::open(path.parent().ok_or_else(||io::Error::from_raw_os_error(libc::EINVAL))?)?);
    if unsafe{libc::fsync(directory.as_raw_fd())}<0{return Err(io::Error::last_os_error());}Ok(())
}
struct Locked(File);
impl Drop for Locked{fn drop(&mut self){unsafe{libc::flock(self.0.as_raw_fd(),libc::LOCK_UN);}}}
struct Temporary(Option<std::path::PathBuf>);
impl Drop for Temporary{fn drop(&mut self){if let Some(path)=self.0.take(){let _=std::fs::remove_file(path);}}}
/// Register only at actual allocation, with its Linux filesystem identity.
/// Binding a pathname with another owner does not change this sockfs inode.
pub fn allocated(runtime:&Path,fd:RawFd,fsuid:u32,fsgid:u32)->io::Result<Receipt>{
    let receipt=Receipt::mint(fd)?;let dir=runtime.join("socket-inodes");std::fs::create_dir_all(&dir)?;
    let path=dir.join(receipt.name());
    let lock=Locked(OpenOptions::new().read(true).write(true).create(true).truncate(false).custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).mode(0o600).open(path.with_extension("lock"))?);
    loop{if unsafe{libc::flock(lock.0.as_raw_fd(),libc::LOCK_EX)}==0{break;}let error=io::Error::last_os_error();if error.kind()!=io::ErrorKind::Interrupted{return Err(error);}}
    publish(&path,receipt,Record{uid:fsuid,gid:fsgid,mode:0o777},|file|file)?;Ok(receipt)
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::os::fd::{FromRawFd,OwnedFd};
    #[test]
    fn actual_allocation_receipt_records_do_not_retain_socket_or_accept_foreign_generation(){
        let mut sockets=[0;2];assert_eq!(unsafe{libc::socketpair(libc::AF_UNIX,libc::SOCK_STREAM,0,sockets.as_mut_ptr())},0);
        let source=unsafe{OwnedFd::from_raw_fd(sockets[0])};let peer=unsafe{OwnedFd::from_raw_fd(sockets[1])};
        static NEXT:AtomicU64=AtomicU64::new(0);
        let root=std::env::temp_dir().join(format!("aim-socket-receipt-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir(&root).unwrap();
        let receipt=allocated(&root,source.as_raw_fd(),unsafe{libc::getuid()},unsafe{libc::getgid()}).unwrap();
        assert_eq!(Receipt::from_bytes(&receipt.to_bytes()).unwrap(),receipt);receipt.validate(source.as_raw_fd()).unwrap();
        assert!(receipt.validate(peer.as_raw_fd()).is_err());
        let bytes=std::fs::read(root.join("socket-inodes").join(receipt.name())).unwrap();
        let record=decode(&bytes,receipt).unwrap();assert_eq!(record.uid,unsafe{libc::getuid()});
        let other=Receipt::mint(peer.as_raw_fd()).unwrap();assert_ne!(receipt.generation,other.generation);assert!(decode(&bytes,other).is_err());
        let mut invalid=receipt.to_bytes();invalid[8..24].fill(0);assert!(Receipt::from_bytes(&invalid).is_err());
        assert!(decode(&[0;36],receipt).is_err());
        drop(source);
        let mut ready=libc::pollfd{fd:peer.as_raw_fd(),events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut ready,1,1000)},1);
        let mut byte=0u8;assert_eq!(unsafe{libc::read(peer.as_raw_fd(),(&mut byte as *mut u8).cast(),1)},0,"allocation metadata must not retain the socket and suppress EOF");
        drop(peer);std::fs::remove_dir_all(root).unwrap();
    }
}
