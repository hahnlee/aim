//! Linux v6.6 FUSE UAPI (7.39) and fs/fuse/dev.c device ownership (#221).
//! Protocol derived from Linux, copyright Miklos Szeredi / kernel contributors.
//! A native broker owns the connection independently of any guest process.
use crate::errno::{self,Errno,EBADF,EINVAL,EIO,ENODEV,ENOTTY};
use std::{fs,path::{Path,PathBuf},io::Read,os::{fd::{AsRawFd,RawFd},unix::{net::UnixStream,fs::PermissionsExt}},time::Duration};
mod broker;
pub const FUSE_DEV_IOC_CLONE:u64=0x8004e500;
pub const DEVICE_MARKER:i32=0x4655;
pub const FILE_MARKER:i32=0x4656;
pub const HEADER_SIZE:usize=40;
pub const OUT_HEADER_SIZE:usize=16;
pub const MAX_MESSAGE:usize=16*1024*1024;
pub const INIT:u32=26;
pub const FORGET:u32=2;
pub const BATCH_FORGET:u32=42;
const IDENTIFY:u32=1;
const DEVICE_READ:u32=2;
const DEVICE_WRITE:u32=3;
const DEVICE_POLL:u32=4;
const REQUEST:u32=5;
const NO_REPLY:u32=6;
const ABORT:u32=7;
const MOUNT:u32=8;
const OPEN_DEVICE:u32=9;
const CLONE:u32=10;
const OPEN_FILE:u32=11;
const FILE_OFFSET:u32=12;
const FILE_IO:u32=13;
const FILE_DESCRIBE:u32=14;
const FILE_FLAGS:u32=15;
const BORROW_FILE:u32=16;
const NOTIFICATIONS:u32=17;
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct SessionKey(PathBuf);
impl SessionKey{
    pub fn transport(&self)->&Path{&self.0}
    pub fn from_transport(path:PathBuf)->Self{Self(path)}
}
pub struct Reply{pub body:Vec<u8>}
#[derive(Clone,Debug)]
pub struct Notification{pub code:i32,pub body:Vec<u8>}
pub struct Client{key:SessionKey}
impl Client{
    pub fn from_key(key:&SessionKey)->Result<Self,Errno>{rpc(key,IDENTIFY,0,&[])?;Ok(Self{key:key.clone()})}
    pub fn request(&self,opcode:u32,nodeid:u64,payload:&[u8],uid:u32,gid:u32,pid:u32)->Result<Reply,Errno>{
        let body=rpc(&self.key,REQUEST,0,&request_body(opcode,nodeid,payload,uid,gid,pid))?;Ok(Reply{body})
    }
    pub fn request_no_reply(&self,opcode:u32,nodeid:u64,payload:&[u8],uid:u32,gid:u32,pid:u32)->Result<(),Errno>{
        if !matches!(opcode,FORGET|BATCH_FORGET){return Err(EINVAL)}
        rpc(&self.key,NO_REPLY,0,&request_body(opcode,nodeid,payload,uid,gid,pid))?;Ok(())
    }
    pub fn take_notifications(&self)->Result<Vec<Notification>,Errno>{
        let body=rpc(&self.key,NOTIFICATIONS,0,&[])?;let mut at=0;let count=read_word(&body,&mut at)?;let mut values=Vec::new();
        for _ in 0..count{let code=read_word(&body,&mut at)? as i32;let length=read_word(&body,&mut at)? as usize;let bytes=body.get(at..at+length).ok_or(EIO)?.to_vec();at+=length;values.push(Notification{code,body:bytes});}
        if at!=body.len(){return Err(EIO)}Ok(values)
    }
    /// Queue INIT without waiting for the userspace daemon that receives fd afterwards.
    pub fn mount(&self)->Result<(),Errno>{rpc(&self.key,MOUNT,0,&[])?;Ok(())}
}
fn request_body(op:u32,node:u64,payload:&[u8],uid:u32,gid:u32,pid:u32)->Vec<u8>{
    let mut out=Vec::with_capacity(24+payload.len());out.extend(op.to_le_bytes());out.extend(node.to_le_bytes());out.extend(uid.to_le_bytes());out.extend(gid.to_le_bytes());out.extend(pid.to_le_bytes());out.extend(payload);out
}
fn io_error(error:std::io::Error)->Errno{error.raw_os_error().map(errno::from_darwin).unwrap_or(EIO)}
fn id()->Result<u64,Errno>{let mut bytes=[0;8];fs::File::open("/dev/urandom").and_then(|mut file|file.read_exact(&mut bytes)).map_err(io_error)?;Ok(u64::from_le_bytes(bytes).max(1))}
fn mark(fd:RawFd,marker:i32)->Result<(),Errno>{let value=libc::linger{l_onoff:0,l_linger:marker};if unsafe{libc::setsockopt(fd,libc::SOL_SOCKET,libc::SO_LINGER,(&value as *const libc::linger).cast(),std::mem::size_of_val(&value) as _)}<0{Err(errno::last())}else{Ok(())}}
fn raw_marker(fd:RawFd)->Option<i32>{let mut value=libc::linger{l_onoff:0,l_linger:0};let mut len=std::mem::size_of_val(&value) as libc::socklen_t;if unsafe{libc::getsockopt(fd,libc::SOL_SOCKET,libc::SO_LINGER,(&mut value as *mut libc::linger).cast(),&mut len)}<0||value.l_onoff!=0{return None}matches!(value.l_linger,DEVICE_MARKER|FILE_MARKER).then_some(value.l_linger)}
fn socket_path(fd:RawFd,peer:bool)->Result<PathBuf,Errno>{
    let mut address:libc::sockaddr_un=unsafe{std::mem::zeroed()};let mut length=std::mem::size_of_val(&address) as libc::socklen_t;
    let result=unsafe{if peer{libc::getpeername(fd,(&mut address as *mut libc::sockaddr_un).cast(),&mut length)}else{libc::getsockname(fd,(&mut address as *mut libc::sockaddr_un).cast(),&mut length)}};
    if result<0{return Err(errno::last())}if address.sun_family as i32!=libc::AF_UNIX{return Err(EBADF)}
    let bytes=address.sun_path.iter().take_while(|byte|**byte!=0).map(|byte|*byte as u8).collect::<Vec<_>>();
    use std::os::unix::ffi::OsStringExt;Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}
pub fn marker(fd:RawFd)->Option<i32>{let value=raw_marker(fd)?;let(key,number)=descriptor(fd).ok()?;if value==FILE_MARKER{rpc(&key,FILE_DESCRIBE,number,&[]).ok()?;}else{rpc(&key,IDENTIFY,number,&[]).ok()?;}Some(value)}
fn descriptor(fd:RawFd)->Result<(SessionKey,u64),Errno>{
    if raw_marker(fd).is_none(){return Err(EBADF)}let key=SessionKey(socket_path(fd,true)?);
    let local=socket_path(fd,false)?;let text=local.extension().and_then(|part|part.to_str()).ok_or(EBADF)?;
    let number=u64::from_str_radix(text.strip_prefix('d').ok_or(EBADF)?,16).map_err(|_|EBADF)?;Ok((key,number))
}
pub fn session_for_fd(fd:RawFd)->Result<SessionKey,Errno>{let(key,number)=descriptor(fd)?;let body=rpc(&key,IDENTIFY,number,&[])?;let text=String::from_utf8(body).map_err(|_|EIO)?;Ok(SessionKey(PathBuf::from(text)))}
/// Allocate transport independently of the possibly long guest runtime path.
/// The local descriptor address has the longest suffix; validate it as well.
fn transport_directory()->Result<PathBuf,Errno>{
    use std::os::unix::ffi::OsStrExt;
    let capacity=unsafe{std::mem::zeroed::<libc::sockaddr_un>()}.sun_path.len();
    let candidates=[std::env::temp_dir(),PathBuf::from("/tmp")];
    for base in candidates {
        for _ in 0..16 {
            let directory=base.join(format!("af-{:016x}",id()?));
            let local=directory.join("c.dffffffffffffffff");
            if local.as_os_str().as_bytes().len()>=capacity{break;}
            match fs::create_dir(&directory){
                Ok(())=>{if let Err(error)=fs::set_permissions(&directory,fs::Permissions::from_mode(0o700)){let _=fs::remove_dir(&directory);return Err(io_error(error));}return Ok(directory);},
                Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>continue,
                Err(error)=>return Err(io_error(error)),
            }
        }
    }
    Err(36)
}
struct TransportAllocation{directory:PathBuf,entry:Option<PathBuf>,identity:(u64,u64),connections:PathBuf}
impl Drop for TransportAllocation{fn drop(&mut self){
    use std::os::unix::fs::MetadataExt;
    if let Some(entry)=&self.entry{let _=fs::remove_file(entry);}
    if let Some(number)=self.directory.file_name().and_then(|name|name.to_str()).and_then(|name|name.strip_prefix("af-")).and_then(|name|u64::from_str_radix(name,16).ok()){
        let record=self.connections.join(number.to_string());let expected=self.directory.join("c.sock");
        if fs::read(&record).is_ok_and(|bytes|bytes==expected.as_os_str().as_encoded_bytes()){let _=fs::remove_file(record);}
    }
    if fs::symlink_metadata(&self.directory).is_ok_and(|metadata|metadata.is_dir()&&(metadata.dev(),metadata.ino())==self.identity){
        if let Err(error)=fs::remove_dir_all(&self.directory){if error.kind()!=std::io::ErrorKind::NotFound{eprintln!("owned FUSE transport cleanup: {error}");}}
    }
}}
pub fn open_device(runtime_dir:&Path,flags:i32)->Result<RawFd,Errno>{
    let fd=open_device_with_helper(runtime_dir,flags,&std::env::current_exe().map_err(io_error)?)?;
    if let Err(error)=super::fdtab::publish_typed_guest(fd){unsafe{libc::close(fd);}return Err(error);}
    Ok(fd)
}
fn open_device_with_helper(runtime_dir:&Path,flags:i32,helper:&Path)->Result<RawFd,Errno>{
    fs::create_dir_all(runtime_dir).map_err(io_error)?;
    let directory=transport_directory()?;
    use std::os::unix::fs::MetadataExt;let metadata=fs::symlink_metadata(&directory).map_err(io_error)?;
    let mut allocation=TransportAllocation{directory:directory.clone(),entry:None,identity:(metadata.dev(),metadata.ino()),connections:runtime_dir.join("fuse-connections")};
    let registry=runtime_dir.join("fuse-sessions");fs::create_dir_all(&registry).map_err(io_error)?;
    let entry=registry.join(directory.file_name().ok_or(EINVAL)?);allocation.entry=Some(entry.clone());
    use std::os::unix::fs::OpenOptionsExt;
    let mut record=fs::OpenOptions::new().create_new(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&entry).map_err(io_error)?;
    std::io::Write::write_all(&mut record,directory.as_os_str().as_encoded_bytes()).map_err(io_error)?;record.sync_all().map_err(io_error)?;
    let key=SessionKey(directory.join("c.sock"));
    // The explicit helper CLI starts before guest loader/global state initialization.
    let mut child=std::process::Command::new(helper)
        .arg("--fuse-broker").arg(&key.0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn().map_err(io_error)?;
    let started=std::time::Instant::now();
    loop{
        if key.0.exists(){break}
        if child.try_wait().map_err(io_error)?.is_some(){return Err(ENODEV)}
        if started.elapsed()>Duration::from_secs(5){child.kill().map_err(io_error)?;child.wait().map_err(io_error)?;return Err(ENODEV)}
        std::thread::sleep(Duration::from_millis(5));
    }
    let result=open_on_key(&key,flags,OPEN_DEVICE,&[]);
    if result.is_err(){let _=rpc(&key,ABORT,0,&[]);}
    // Reap only this broker; it exits when its final device/open description closes.
    std::thread::spawn(move||{let _allocation=allocation;if let Err(error)=child.wait(){eprintln!("FUSE broker reap: {error}");}});
    result
}
fn open_on_key(key:&SessionKey,flags:i32,op:u32,payload:&[u8])->Result<RawFd,Errno>{
    use std::os::unix::ffi::OsStrExt;
    let number=id()?;let local=key.0.with_extension(format!("d{number:x}"));
    let fd=unsafe{libc::socket(libc::AF_UNIX,libc::SOCK_STREAM,0)};if fd<0{return Err(errno::last())}
    let address=|path:&Path|->Result<libc::sockaddr_un,Errno>{let bytes=path.as_os_str().as_bytes();let mut out:libc::sockaddr_un=unsafe{std::mem::zeroed()};if bytes.len()>=out.sun_path.len(){return Err(36)}out.sun_family=libc::AF_UNIX as _;out.sun_len=std::mem::size_of::<libc::sockaddr_un>() as u8;for (dst,src) in out.sun_path.iter_mut().zip(bytes){*dst=*src as _;}Ok(out)};
    no_sigpipe(fd);
    let result=(||{let local_address=address(&local)?;let remote=address(&key.0)?;
        if unsafe{libc::bind(fd,(&local_address as *const libc::sockaddr_un).cast(),std::mem::size_of_val(&local_address) as _)}<0{return Err(errno::last())}
        if unsafe{libc::connect(fd,(&remote as *const libc::sockaddr_un).cast(),std::mem::size_of_val(&remote) as _)}<0{return Err(errno::last())}
        send_frame(fd,op,number,payload)?;read_reply(fd)?;mark(fd,if matches!(op,OPEN_FILE|BORROW_FILE){FILE_MARKER}else{DEVICE_MARKER})?;
        if unsafe{libc::fcntl(fd,libc::F_SETFD,if flags&0x80000!=0{libc::FD_CLOEXEC}else{0})}<0{return Err(errno::last())}
        if flags&0x800!=0&&unsafe{libc::fcntl(fd,libc::F_SETFL,libc::O_NONBLOCK)}<0{return Err(errno::last())}
        Ok(fd)})();
    if result.is_err(){unsafe{libc::close(fd);}let _=fs::remove_file(local);}result
}
pub fn read_device(fd:RawFd,buffer:&mut[u8],nonblock:bool)->Result<usize,Errno>{
    if buffer.len()<HEADER_SIZE{return Err(EINVAL)}let(key,number)=descriptor(fd)?;let mut payload=Vec::new();payload.extend((buffer.len() as u64).to_le_bytes());payload.push(nonblock as u8);
    let stream=connect(&key)?;send_frame(stream.as_raw_fd(),DEVICE_READ,number,&payload)?;let body=read_reply(stream.as_raw_fd())?;
    if body.len()>buffer.len(){return Err(EIO)}buffer[..body.len()].copy_from_slice(&body);
    // The broker re-arms level readiness only after this acknowledgement.
    drain_marker(fd);write_all_fd(stream.as_raw_fd(),&[1])?;Ok(body.len())
}
pub fn write_device(fd:RawFd,buffer:&[u8])->Result<usize,Errno>{let(key,number)=descriptor(fd)?;let body=rpc(&key,DEVICE_WRITE,number,buffer)?;if body.len()!=8{return Err(EIO)}Ok(u64::from_le_bytes(body.try_into().unwrap()) as usize)}
pub fn poll_device(fd:RawFd)->Result<u16,Errno>{let(key,number)=descriptor(fd)?;let body=rpc(&key,DEVICE_POLL,number,&[])?;if body.len()!=2{return Err(EIO)}Ok(u16::from_le_bytes(body.try_into().unwrap()))}
pub fn ioctl_clone(destination:RawFd,source:RawFd)->Result<(),Errno>{
    if marker(destination)!=Some(DEVICE_MARKER)||marker(source)!=Some(DEVICE_MARKER){return Err(EINVAL)}
    let(dest,number)=descriptor(destination)?;let key=session_for_fd(source)?;
    rpc(&dest,CLONE,number,key.0.to_str().ok_or(EINVAL)?.as_bytes())?;Ok(())
}
pub fn ioctl(fd:RawFd,request:u64,source_fd:i32)->Result<(),Errno>{if request!=FUSE_DEV_IOC_CLONE{return Err(ENOTTY)}ioctl_clone(fd,source_fd)}
pub fn abort(key:&SessionKey)->Result<(),Errno>{rpc(key,ABORT,0,&[])?;Ok(())}
pub fn serve_broker(path:&Path)->Result<(),Errno>{broker::serve(path)}
fn no_sigpipe(fd:RawFd){let one=1i32;unsafe{libc::setsockopt(fd,libc::SOL_SOCKET,libc::SO_NOSIGPIPE,(&one as *const i32).cast(),4);}}
fn connect(key:&SessionKey)->Result<UnixStream,Errno>{let stream=UnixStream::connect(&key.0).map_err(io_error)?;no_sigpipe(stream.as_raw_fd());Ok(stream)}
fn rpc(key:&SessionKey,op:u32,number:u64,payload:&[u8])->Result<Vec<u8>,Errno>{let stream=connect(key)?;send_frame(stream.as_raw_fd(),op,number,payload)?;read_reply(stream.as_raw_fd())}
fn send_frame(fd:RawFd,op:u32,number:u64,payload:&[u8])->Result<(),Errno>{if payload.len()>MAX_MESSAGE{return Err(EINVAL)}let mut header=Vec::new();header.extend(b"AIMFUSE1");header.extend(op.to_le_bytes());header.extend(number.to_le_bytes());header.extend((payload.len() as u32).to_le_bytes());write_all_fd(fd,&header)?;write_all_fd(fd,payload)}
fn write_all_fd(fd:RawFd,mut bytes:&[u8])->Result<(),Errno>{while !bytes.is_empty(){let count=unsafe{libc::send(fd,bytes.as_ptr().cast(),bytes.len(),0)};if count<0{return Err(errno::last())}if count==0{return Err(EIO)}bytes=&bytes[count as usize..];}Ok(())}
fn read_exact_fd(fd:RawFd,mut bytes:&mut[u8])->Result<(),Errno>{while !bytes.is_empty(){let count=unsafe{libc::read(fd,bytes.as_mut_ptr().cast(),bytes.len())};if count<0{return Err(errno::last())}if count==0{return Err(107)}bytes=&mut bytes[count as usize..];}Ok(())}
fn read_reply(fd:RawFd)->Result<Vec<u8>,Errno>{let mut head=[0;8];read_exact_fd(fd,&mut head)?;let error=i32::from_le_bytes(head[..4].try_into().unwrap());let length=u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;if length>MAX_MESSAGE{return Err(EIO)}let mut body=vec![0;length];read_exact_fd(fd,&mut body)?;if error!=0{Err(error)}else{Ok(body)}}
fn reply(fd:RawFd,value:Result<&[u8],Errno>)->Result<(),Errno>{let(error,body)=match value{Ok(body)=>(0,body),Err(error)=>(error,&[][..])};let mut header=Vec::new();header.extend(error.to_le_bytes());header.extend((body.len() as u32).to_le_bytes());write_all_fd(fd,&header)?;write_all_fd(fd,body)}
fn drain_marker(fd:RawFd){let mut bytes=[0;64];while unsafe{libc::recv(fd,bytes.as_mut_ptr().cast(),bytes.len(),libc::MSG_DONTWAIT)}>0{}}

#[derive(Clone,Debug)]
pub struct MountPolicy{pub uid:u32,pub gid:u32,pub allow_other:bool,pub default_permissions:bool,pub read_only:bool}
#[derive(Clone,Debug)]
pub struct Description{pub key:SessionKey,pub node:u64,pub fh:u64,pub flags:u32,pub directory:bool,pub relative:String,pub guest:String,pub open_flags:u32,pub policy:MountPolicy}
pub struct IoReply{pub body:Vec<u8>,pub offset:u64}
fn text(out:&mut Vec<u8>,value:&str){out.extend((value.len() as u32).to_le_bytes());out.extend(value.as_bytes());}
fn read_text(body:&[u8],at:&mut usize)->Result<String,Errno>{let length=u32::from_le_bytes(body.get(*at..*at+4).ok_or(EIO)?.try_into().unwrap()) as usize;*at+=4;let value=String::from_utf8(body.get(*at..*at+length).ok_or(EIO)?.to_vec()).map_err(|_|EIO)?;*at+=length;Ok(value)}
/// A real open-description capability. Offset and RELEASE lifetime are broker-owned.
pub fn open_description(key:&SessionKey,nodeid:u64,fh:u64,flags:u32,directory:bool,relative:&str,guest:&str,open_flags:u32,policy:&MountPolicy,uid:u32,gid:u32,pid:u32)->Result<RawFd,Errno>{
    let mut body=Vec::new();body.extend(nodeid.to_le_bytes());body.extend(fh.to_le_bytes());body.extend(flags.to_le_bytes());body.push(directory as u8);body.extend(uid.to_le_bytes());body.extend(gid.to_le_bytes());body.extend(pid.to_le_bytes());body.extend(open_flags.to_le_bytes());body.extend(policy.uid.to_le_bytes());body.extend(policy.gid.to_le_bytes());body.push(policy.allow_other as u8);body.push(policy.default_permissions as u8);body.push(policy.read_only as u8);text(&mut body,relative);text(&mut body,guest);open_on_key(key,flags as i32,OPEN_FILE,&body)
}
pub fn description(fd:RawFd)->Result<Description,Errno>{let(key,number)=descriptor(fd)?;let body=rpc(&key,FILE_DESCRIBE,number,&[])?;if body.len()<44{return Err(EIO)}let mut at=36;
    let policy=MountPolicy{uid:u32::from_le_bytes(body[25..29].try_into().unwrap()),gid:u32::from_le_bytes(body[29..33].try_into().unwrap()),allow_other:body[33]!=0,default_permissions:body[34]!=0,read_only:body[35]!=0};
    let relative=read_text(&body,&mut at)?;let guest=read_text(&body,&mut at)?;if at!=body.len(){return Err(EIO)}
    Ok(Description{key,node:u64::from_le_bytes(body[..8].try_into().unwrap()),fh:u64::from_le_bytes(body[8..16].try_into().unwrap()),flags:u32::from_le_bytes(body[16..20].try_into().unwrap()),directory:body[20]!=0,open_flags:u32::from_le_bytes(body[21..25].try_into().unwrap()),policy,relative,guest})}
pub fn offset(fd:RawFd,next:Option<u64>)->Result<u64,Errno>{let(key,number)=descriptor(fd)?;let body=next.map(|value|value.to_le_bytes().to_vec()).unwrap_or_default();let value=rpc(&key,FILE_OFFSET,number,&body)?;if value.len()!=8{return Err(EIO)}Ok(u64::from_le_bytes(value.try_into().unwrap()))}
pub fn description_io_at(fd:RawFd,write:bool,position:Option<u64>,data:&[u8],size:usize)->Result<IoReply,Errno>{let(key,number)=descriptor(fd)?;let mut body=Vec::new();body.push(write as u8);body.push(position.is_some() as u8);body.extend(position.unwrap_or(0).to_le_bytes());body.extend((size as u32).to_le_bytes());let credentials=super::cred::current();body.extend(credentials.uid[3].to_le_bytes());body.extend(credentials.gid[3].to_le_bytes());body.extend((super::process::getpid() as u32).to_le_bytes());body.extend(data);let reply=rpc(&key,FILE_IO,number,&body)?;if reply.len()<8{return Err(EIO)}Ok(IoReply{offset:u64::from_le_bytes(reply[..8].try_into().unwrap()),body:reply[8..].to_vec()})}
pub fn description_io(fd:RawFd,write:bool,position:Option<u64>,data:&[u8],size:usize)->Result<Vec<u8>,Errno>{description_io_at(fd,write,position,data,size).map(|reply|reply.body)}

pub fn description_flags(fd:RawFd,next:Option<u32>)->Result<u32,Errno>{let(key,number)=descriptor(fd)?;let body=next.map(|flags|flags.to_le_bytes().to_vec()).unwrap_or_default();let reply=rpc(&key,FILE_FLAGS,number,&body)?;if reply.len()!=4{return Err(EIO)}Ok(u32::from_le_bytes(reply.try_into().unwrap()))}

#[cfg(test)]
mod tests;

fn read_word(bytes:&[u8],at:&mut usize)->Result<u32,Errno>{let value=u32::from_le_bytes(bytes.get(*at..*at+4).ok_or(EIO)?.try_into().unwrap());*at+=4;Ok(value)}
pub fn borrow_inode_descriptor(key:&SessionKey,node:u64)->Result<std::os::fd::OwnedFd,Errno>{
    use std::os::fd::FromRawFd;let fd=open_on_key(key,0,BORROW_FILE,&node.to_le_bytes())?;Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})
}
