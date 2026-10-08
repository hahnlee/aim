//! Socket inode allocation shared by native init and the Linux syscall owner.
use std::{fs::{File,OpenOptions},io::{self,Write},os::{fd::{AsRawFd,RawFd},unix::fs::OpenOptionsExt},path::Path,sync::atomic::{AtomicU64,Ordering}};
#[derive(Clone,Copy)]
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
#[derive(Clone,Copy)]
pub struct Record{pub uid:u32,pub gid:u32,pub mode:u32}
pub fn encode(identity:Identity,record:Record)->Vec<u8>{let mut bytes=b"SOCKIN01".to_vec();bytes.extend(identity.cookie.to_le_bytes());bytes.extend(identity.inode.to_le_bytes());bytes.extend(record.uid.to_le_bytes());bytes.extend(record.gid.to_le_bytes());bytes.extend(record.mode.to_le_bytes());bytes}
pub fn decode(bytes:&[u8],identity:Identity)->io::Result<Record>{
    if bytes.len()!=36||&bytes[..8]!=b"SOCKIN01"||u64::from_le_bytes(bytes[8..16].try_into().unwrap())!=identity.cookie||u64::from_le_bytes(bytes[16..24].try_into().unwrap())!=identity.inode{return Err(io::Error::from_raw_os_error(libc::EIO));}
    Ok(Record{uid:u32::from_le_bytes(bytes[24..28].try_into().unwrap()),gid:u32::from_le_bytes(bytes[28..32].try_into().unwrap()),mode:u32::from_le_bytes(bytes[32..36].try_into().unwrap())})
}
/// Caller wraps every opened fd in its actual kernel-private descriptor owner.
pub fn publish<H:Write+AsRawFd>(path:&Path,identity:Identity,record:Record,wrap:impl Fn(File)->H)->io::Result<()> {
    static NEXT:AtomicU64=AtomicU64::new(0);
    let(name,file)=loop{let name=path.with_extension(format!("{}.{}.new",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));match OpenOptions::new().create_new(true).write(true).custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).mode(0o600).open(&name){Ok(file)=>break(name,wrap(file)),Err(error)if error.kind()==io::ErrorKind::AlreadyExists=>continue,Err(error)=>return Err(error)}};
    let mut cleanup=Temporary(Some(name.clone()));let mut file=file;file.write_all(&encode(identity,record))?;
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
pub fn allocated(runtime:&Path,fd:RawFd,fsuid:u32,fsgid:u32)->io::Result<Identity>{
    let identity=identity(fd)?;let dir=runtime.join("socket-inodes");std::fs::create_dir_all(&dir)?;
    let path=dir.join(format!("{:x}",identity.cookie));
    let lock=Locked(OpenOptions::new().read(true).write(true).create(true).truncate(false).custom_flags(libc::O_CLOEXEC|libc::O_NOFOLLOW).mode(0o600).open(path.with_extension("lock"))?);
    loop{if unsafe{libc::flock(lock.0.as_raw_fd(),libc::LOCK_EX)}==0{break;}let error=io::Error::last_os_error();if error.kind()!=io::ErrorKind::Interrupted{return Err(error);}}
    publish(&path,identity,Record{uid:fsuid,gid:fsgid,mode:0o777},|file|file)?;Ok(identity)
}
