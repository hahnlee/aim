//! The binder device nodes `/dev/binder`, `/dev/hwbinder` and
//! `/dev/vndbinder` (#167), backed by the driver in the binder host daemon
//! (`aim-binder-host`, `linux-run --binder NAME`).
//!
//! A binder fd is a host socket whose peer is the daemon: epoll and poll see
//! it readable while a read would find work, and its last close (including
//! process exit) releases the binder process. ioctl, mmap and poll
//! registration go to the daemon. The fd table (`fdtab`) holds its file, so
//! dup'ed fds resolve and a closed one no longer does.

use std::sync::{Mutex, OnceLock};

use aim_binder_driver::Device;
use aim_binder_host::client::{BinderFile, Client, UserMemory};

use super::fdtab::Kind;
use crate::errno::{EINTR, EINVAL, ENODEV, ENOMEM, EPERM};

/// The daemon's bootstrap name, to reconnect after fork.
static NAME: OnceLock<String> = OnceLock::new();
static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

/// Classification is granted by the native owner's explicit registry.
pub fn file_class(fd: i32) -> Result<u32, i32> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 { return Err(crate::errno::last()); }
    if stat.st_mode & libc::S_IFMT == libc::S_IFSOCK {
        return super::net::classify_socket_scm(fd);
    }
    let client = CLIENT.lock().unwrap();
    match client.as_ref() {
        Some(client) => client.file_class(fd),
        None => Ok(0),
    }
}

pub(super) fn create_path(fd: i32, flags: u32) -> Result<std::os::fd::OwnedFd,i32> {
    let client = CLIENT.lock().unwrap();
    client.as_ref().ok_or(crate::errno::ENODEV)?.create_path(fd,flags)
}

/// The files whose pages the daemon's process shares (`sharedfile`); none
/// without a daemon.
pub fn shared_files() -> Vec<aim_binder_host::wire::SharedFile> {
    CLIENT
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c.shared_files().ok())
        .unwrap_or_default()
}

/// execve in place: the calling thread's client state is the old image's.
pub fn exec_reset() {
    aim_binder_host::client::forget_thread();
}

/// Connect to the daemon serving bootstrap name `name`.
pub fn init(name: &str) -> Result<(), String> {
    let client =
        Client::connect(name).ok_or_else(|| format!("binder host '{name}' is not running"))?;
    *CLIENT.lock().unwrap() = Some(client);
    super::net::reset_socket_scm();
    super::net::init_socket_carriers().map_err(|error| format!("socket carrier initialization: errno {error}"))?;
    let _ = NAME.set(name.to_string());
    super::fdtab::refresh_capabilities()
        .map_err(|e| format!("binder capability registry: errno {e}"))?;
    Ok(())
}

pub(super) fn socket_carriers_configured() -> bool { CLIENT.lock().unwrap().is_some() }

pub(super) fn socket_scm_endpoint() -> Result<aim_binder_host::socket_scm::Endpoint, i32> {
    CLIENT.lock().unwrap().as_ref().ok_or(crate::errno::ENODEV)?.socket_scm_endpoint()
}

pub(super) fn create_regular_scm(backing: i32, writer: Option<i32>, metadata: &aim_binder_host::wire::RegularMetadata) -> Result<aim_binder_host::regular_scm::FilePort, i32> {
    CLIENT.lock().unwrap().as_ref().ok_or(crate::errno::ENODEV)?.create_regular_scm(backing, writer, metadata)
}
pub(super) fn resolve_regular_scm(carrier: i32) -> Result<aim_binder_host::regular_scm::Resolved, i32> {
    CLIENT.lock().unwrap().as_ref().ok_or(crate::errno::ENODEV)?.resolve_regular_scm(carrier)
}
pub(super) fn drain_regular_scm(identity: &[u8; 36]) -> Result<(), i32> {
    CLIENT.lock().unwrap().as_ref().ok_or(crate::errno::ENODEV)?.drain_regular_scm(identity)
}

fn lookup(fd: i32) -> Option<BinderFile> {
    match super::fdtab::get(fd)? {
        Kind::Binder(file) => Some(file),
        _ => None,
    }
}

#[derive(Default)]
struct Guest {
    exported: Vec<super::fdtab::ExportedFd>,
    regular: Vec<(i32, super::fdtab::RegularExport)>,
    sockets: Vec<(i32,super::net::BinderSocketExport)>,
}

/// Nothing maps below 4 GiB on macOS arm64 (`__PAGEZERO`), so a pointer
/// there, null included, is EFAULT as copy_from_user would make it. A copy
/// of nothing touches no memory and succeeds whatever the pointer, as on
/// Linux: an empty Parcel (a ping, a void reply) has a null data pointer.
fn check_user(address: u64, len: usize) -> Result<(), i32> {
    if len > 0 && address < 1 << 32 {
        Err(14)
    } else {
        Ok(())
    }
}

impl UserMemory for Guest {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, i32> {
        check_user(address, len)?;
        let mut v = vec![0u8; len];
        // SAFETY: guest memory the guest passed to the ioctl.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, v.as_mut_ptr(), len) };
        Ok(v)
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), i32> {
        check_user(address, data.len())?;
        // SAFETY: as above.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
        Ok(())
    }

    fn installed(&mut self, fd: i32) {
        // A received SEQPACKET or datagram socket (an InputChannel) or
        // memfd keeps its Linux semantics.
        super::fdtab::on_close(fd);
        super::fdtab::adopt_untyped(fd);
    }

    fn install_fileport(&mut self, fd:i32, port:aim_binder_host::mach::Port) -> Result<(),i32> {
        super::fdtab::install_fileport(fd,port)
    }
    fn installed_typed(&mut self, fd: i32, class: u32) {
        self.installed(fd);
        if class == aim_binder_host::proxy_file::CLASS {
            super::fdtab::insert(fd, Kind::ProxyFile);
        }
    }
    fn export_fd(&mut self, fd: i32) -> Result<i32,i32> {
        match super::fdtab::export_scm(fd)?{
            super::fdtab::ScmExport::Regular(regular)=>{let number=regular.backing_fd;self.regular.push((fd,regular));Ok(number)},
            super::fdtab::ScmExport::Socket(source)=>{let socket=super::net::export_binder_socket(source)?;let number=socket.backing();self.sockets.push((fd,socket));Ok(number)},
            super::fdtab::ScmExport::Other(exported)=>{let number=exported.fd;self.exported.push(exported);Ok(number)},
        }
    }
    fn socket_export(&mut self,fd:i32)->Result<Option<aim_storage::socket_inode::Receipt>,i32>{
        Ok(self.sockets.iter().find(|(original,_)|*original==fd).map(|(_,socket)|socket.receipt()))
    }
    fn install_socket(&mut self,fd:i32,receipt:aim_storage::socket_inode::Receipt)->Result<(),i32>{
        self.installed(fd);
        super::net::install_socket_receipt(fd,receipt)?;
        super::fdtab::publish_guest(fd)
    }

    fn regular_export(&mut self, fd:i32) -> Result<Option<aim_binder_host::regular_file::Export>,i32> {
        Ok(self.regular.iter().find(|(original,_)| *original==fd).map(|(_,regular)| aim_binder_host::regular_file::Export {
            metadata:regular.transport.metadata.clone(), writer_fd:regular.transport.writer_fd,
        }))
    }
    fn install_regular(&mut self, fd:i32, metadata:&aim_binder_host::wire::RegularMetadata, writer:Option<aim_binder_host::regular_file::WriterPort>) -> Result<(),i32> {
        super::fdtab::on_close(fd);
        super::fdtab::install_regular(fd,metadata,writer)?;
        super::fdtab::publish_guest(fd)
    }
    fn install_typed(&mut self, fd:i32, class:u32) -> Result<(),i32> {
        if class == aim_binder_host::socket_scm::CLASS{return Err(71);}
        if class == aim_binder_host::path_file::CLASS { super::fdtab::install_path(fd)?; }
        else if class == aim_binder_host::pty_file::CLASS { super::fdtab::install_pty(fd)?; }
        else { self.installed_typed(fd,class); }
        super::fdtab::publish_guest(fd)
    }
    fn file_class(&mut self, fd: i32) -> u32 {
        if self.sockets.iter().any(|(original,_)|*original==fd){aim_binder_host::socket_scm::CLASS}
        else if self.regular.iter().any(|(original,_)| *original==fd) {
            aim_binder_host::regular_file::CLASS
        } else if matches!(super::fdtab::get(fd),Some(Kind::Path(_))) {
            aim_binder_host::path_file::CLASS
        } else if matches!(super::fdtab::get(fd),Some(Kind::Pty(_))) {
            aim_binder_host::pty_file::CLASS
        } else if super::proxy_file::is_proxy(fd) {
            aim_binder_host::proxy_file::CLASS
        } else {
            0
        }
    }
    fn close_file(&mut self, fd:i32) -> Result<(),i32> {
        super::fdtab::close_owned_guest(fd,true)
    }
    fn closed(&mut self, fd: i32) {
        super::fdtab::on_close(fd);
    }
}

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;

/// The device a guest path names: `/dev/binder` and the others, or their
/// binderfs nodes `/dev/binderfs/<name>`. init mounts binderfs there and
/// symlinks the former to them; this layer plays that mount.
fn device(guest_path: &str) -> Option<Device> {
    match guest_path.strip_prefix("/dev/binderfs/") {
        Some(name) => Device::from_path(&format!("/dev/{name}")),
        None => Device::from_path(guest_path),
    }
}

/// Whether `guest_path` is a binder device node that exists.
pub fn is_device(guest_path: &str) -> bool {
    CLIENT.lock().unwrap().is_some() && device(guest_path).is_some()
}

/// `openat` of a binder device node; None for any other path.
pub fn open(guest_path: &str, flags: u64) -> Option<i64> {
    let device = device(guest_path)?;
    let client = CLIENT.lock().unwrap();
    let Some(client) = client.as_ref() else {
        return Some(-(ENODEV as i64));
    };
    let euid = super::cred::getuid(175) as u32;
    let file = client.open(
        device,
        flags & O_NONBLOCK != 0,
        flags & O_CLOEXEC != 0,
        euid,
        &super::cred::seclabel(),
    );
    Some(match file {
        Ok(f) => {
            super::fdtab::insert(f.fd, Kind::Binder(f));
            f.fd as i64
        }
        Err(e) => -(e as i64),
    })
}

/// Binder ioctls are `_IOC(dir, 'b', nr, size)`.
pub fn ioctl(fd: i32, cmd: u64, arg: u64) -> Option<i64> {
    if (cmd >> 8) & 0xff != u64::from(b'b') {
        return None;
    }
    let file = lookup(fd)?;
    let tid = super::process::gettid() as i32;
    // A guest signal for this thread interrupts a read parked in the
    // daemon, as `binder_wait_for_work` returns on a pending signal; the
    // syscall layer restarts the ioctl when no handler is to run.
    let interrupt = move || {
        let _ = file.interrupt(tid);
    };
    let r =
        super::signal::interruptible(&interrupt, || file.ioctl(tid, cmd as u32, arg, &mut Guest::default()));
    Some(match r {
        None => -(EINTR as i64),
        Some(Ok(())) => 0,
        Some(Err(e)) => -(e as i64),
    })
}

const PROT_WRITE: u64 = 2;
const MAP_FIXED: u64 = 0x10;
const PAGE: u64 = 16384;

/// `mmap` of a binder fd: the read-only receive buffer.
pub fn mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: i32) -> Option<i64> {
    let file = lookup(fd)?;
    if prot & PROT_WRITE != 0 {
        return Some(-(EPERM as i64));
    }
    let len = (len + PAGE - 1) & !(PAGE - 1);
    if len == 0 {
        return Some(-(EINVAL as i64));
    }
    let hflags = libc::MAP_PRIVATE | libc::MAP_ANON;
    let base = if flags & MAP_FIXED != 0 {
        // SAFETY: reserving the range the receive buffer will replace.
        let b = unsafe {
            libc::mmap(
                addr as *mut _,
                len as usize,
                libc::PROT_NONE,
                hflags | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if b == libc::MAP_FAILED {
            return Some(-(ENOMEM as i64));
        }
        b as u64
    } else {
        match super::arena::map(
            super::arena::hint(addr, len),
            len,
            libc::PROT_NONE,
            hflags,
            -1,
            0,
        ) {
            Ok(b) => b,
            Err(_) => return Some(-(ENOMEM as i64)),
        }
    };
    Some(match file.mmap(base, len) {
        Ok(()) => base as i64,
        Err(e) => {
            // SAFETY: undoing our reservation.
            unsafe { libc::munmap(base as *mut _, len as usize) };
            -(e as i64)
        }
    })
}

/// The calling thread starts polling `fd` (epoll_ctl, poll), as Linux calls
/// `binder_poll` from those paths.
pub fn poll(fd: i32) {
    if let Some(file) = lookup(fd) {
        let _ = file.poll(super::process::gettid() as i32);
    }
}

#[cfg(test)]
mod socket_receipt_tests {
    use super::*;
    use aim_binder_driver::uapi::*;
    use std::{io::{Read,Write},process::{Child,Command,Stdio},time::{Duration,Instant}};
    fn ioctl(file:&BinderFile,tid:i32,code:u32,arg:&mut[u8]){file.ioctl(tid,code,arg.as_mut_ptr()as u64,&mut Guest::default()).unwrap();}
    fn command(out:&mut Vec<u8>,code:u32,bytes:&[u8]){out.extend(code.to_le_bytes());out.extend(bytes);}
    fn exchange(file:&BinderFile,tid:i32,write:&[u8],read:bool)->Vec<u8>{
        let mut bytes=vec![0u8;if read{512}else{0}];let mut request=WriteRead{write_size:write.len()as u64,write_buffer:write.as_ptr()as u64,read_size:bytes.len()as u64,read_buffer:bytes.as_mut_ptr()as u64,..Default::default()}.encode();
        ioctl(file,tid,BINDER_WRITE_READ,&mut request);let done=WriteRead::decode(&request);assert_eq!(done.write_consumed,write.len()as u64);bytes.truncate(done.read_consumed as usize);bytes
    }
    fn transaction(read:&[u8],wanted:u32)->Option<TransactionData>{let mut at=0;while at+4<=read.len(){let code=u32::from_le_bytes(read[at..at+4].try_into().unwrap());at+=4;if code==wanted{return Some(TransactionData::decode(&read[at..]));}at+=ioc_size(code);}None}
    fn wait_transaction(file:&BinderFile,tid:i32,mut read:Vec<u8>,code:u32)->TransactionData{loop{if let Some(tr)=transaction(&read,code){return tr;}read=exchange(file,tid,&[],true);}}
    fn status(fd:i32)->(u32,u32,u32,u64){let mut bytes=[0u8;128];assert_eq!(super::super::fs::fstat([fd as u64,bytes.as_mut_ptr()as u64,0,0,0,0]),0);(u32::from_ne_bytes(bytes[24..28].try_into().unwrap()),u32::from_ne_bytes(bytes[28..32].try_into().unwrap()),u32::from_ne_bytes(bytes[16..20].try_into().unwrap()),u64::from_ne_bytes(bytes[8..16].try_into().unwrap()))}
    fn dup(fd:i32)->i32{let copy=super::super::fs::dup([fd as u64,0,0,0,0,0]);assert!(copy>=0);copy as i32}
    fn close(fd:i32){assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);}
    fn payload(fd:i32,stat:(u32,u32,u32,u64))->Vec<u8>{let mut data=vec![0;64];data[0..4].copy_from_slice(&0x1235u32.to_le_bytes());data[8..12].copy_from_slice(&BINDER_TYPE_FD.to_le_bytes());data[16..20].copy_from_slice(&fd.to_le_bytes());data[32..36].copy_from_slice(&stat.0.to_le_bytes());data[36..40].copy_from_slice(&stat.1.to_le_bytes());data[40..44].copy_from_slice(&stat.2.to_le_bytes());data[48..56].copy_from_slice(&stat.3.to_le_bytes());data}
    fn received(tr:&TransactionData)->(i32,(u32,u32,u32,u64)){assert_eq!(tr.data_size,64);let bytes=unsafe{std::slice::from_raw_parts(tr.buffer as*const u8,64)};assert_eq!(&bytes[..4],&0x1235u32.to_le_bytes());let fd=i32::from_ne_bytes(bytes[16..20].try_into().unwrap());let expected=(u32::from_ne_bytes(bytes[32..36].try_into().unwrap()),u32::from_ne_bytes(bytes[36..40].try_into().unwrap()),u32::from_ne_bytes(bytes[40..44].try_into().unwrap()),u64::from_ne_bytes(bytes[48..56].try_into().unwrap()));assert_eq!(status(fd),expected);(fd,expected)}
    fn free(file:&BinderFile,tid:i32,buffer:u64){let mut out=vec![];command(&mut out,BC_FREE_BUFFER,&buffer.to_le_bytes());exchange(file,tid,&out,false);}
    struct OwnedChild(Child);
    impl Drop for OwnedChild{fn drop(&mut self){if self.0.try_wait().unwrap().is_none(){self.0.kill().unwrap();}self.0.wait().unwrap();}}
    fn child(role:&str,root:&std::path::Path,name:&str)->OwnedChild{let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::binder::socket_receipt_tests::binder_socket_controlled_child","--ignored","--nocapture"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();writeln!(child.stdin.take().unwrap(),"{role}\n{}\n{name}",root.display()).unwrap();OwnedChild(child)}
    fn finish(child:&mut OwnedChild){let deadline=Instant::now()+Duration::from_secs(20);while child.0.try_wait().unwrap().is_none(){if Instant::now()>=deadline{child.0.kill().unwrap();let mut err=String::new();child.0.stderr.take().unwrap().read_to_string(&mut err).unwrap();panic!("owned Binder child exceeded deadline: {err}");}std::thread::sleep(Duration::from_millis(10));}let mut out=String::new();let mut err=String::new();child.0.stdout.take().unwrap().read_to_string(&mut out).unwrap();child.0.stderr.take().unwrap().read_to_string(&mut err).unwrap();assert!(child.0.wait().unwrap().success(),"{out}\n{err}");assert!(out.contains("BINDER_SOCKET_CHILD_EXECUTED"),"{out}");}
    #[test]
    fn two_process_binder_socket_receipt_preserves_identity_payload_duplicates_and_final_eof(){
        let(_view,root)=crate::vfs::test_view();let name=format!("dev.aim.test.binder-socket.{}",std::process::id());let _server=aim_binder_host::server::Server::start(&name).unwrap();let ready=root.join("binder-socket-ready");let _=std::fs::remove_file(&ready);
        let mut receiver=child("receiver",root,&name);let deadline=Instant::now()+Duration::from_secs(10);while !ready.exists(){assert!(receiver.0.try_wait().unwrap().is_none());assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(10));}
        let mut sender=child("sender",root,&name);finish(&mut sender);finish(&mut receiver);std::fs::remove_file(ready).unwrap();
    }
    #[test]
    #[ignore="controlled process body, executed by its owning two-process test"]
    fn binder_socket_controlled_child(){
        let mut input=String::new();std::io::stdin().read_to_string(&mut input).unwrap();let mut lines=input.lines();let role=lines.next().unwrap();let root=std::path::PathBuf::from(lines.next().unwrap());let name=lines.next().unwrap();
        crate::vfs::init(&root.join("root"),Some(&root.join("run/path-map"))).unwrap();let mut id=super::super::cred::Identity::default();id.uid=[if role=="sender"{1073}else{10151};4];id.gid=[1234;4];super::super::cred::init(id,None);init(name).unwrap();let client=Client::connect(name).unwrap();let file=client.open(Device::Binder,false,true,super::super::cred::current().uid[1],"u:r:shell:s0").unwrap();
        let map=unsafe{libc::mmap(std::ptr::null_mut(),1<<20,libc::PROT_NONE,libc::MAP_PRIVATE|libc::MAP_ANON,-1,0)};assert_ne!(map,libc::MAP_FAILED);file.mmap(map as u64,1<<20).unwrap();let tid=std::process::id()as i32;
        if role=="receiver"{
            let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:FLAT_BINDER_FLAG_ACCEPTS_FDS,binder:0x1235,cookie:0x5678}.encode();ioctl(&file,tid,BINDER_SET_CONTEXT_MGR_EXT,&mut object);
            let mut enter=vec![];command(&mut enter,BC_ENTER_LOOPER,&[]);exchange(&file,tid,&enter,false);std::fs::write(root.join("binder-socket-ready"),b"ready").unwrap();
            let tr=wait_transaction(&file,tid,exchange(&file,tid,&[],true),BR_TRANSACTION);let(fd,expected)=received(&tr);assert_eq!((expected.0,expected.1,expected.2&0o777),(1073,1234,0o777));let duplicate=dup(fd);assert_eq!(status(duplicate),expected);
            let data=payload(duplicate,expected);let offsets=8u64.to_le_bytes();let reply=TransactionData{data_size:data.len()as u64,offsets_size:8,buffer:data.as_ptr()as u64,offsets:offsets.as_ptr()as u64,..Default::default()};let mut write=vec![];command(&mut write,BC_REPLY,&reply.encode());exchange(&file,tid,&write,false);free(&file,tid,tr.buffer);close(fd);close(duplicate);
        }else{
            let mut pair=[-1;2];assert_eq!(super::super::net::socketpair([1,1,0,pair.as_mut_ptr()as u64,0,0]),0);let expected=status(pair[0]);let data=payload(pair[0],expected);let offsets=8u64.to_le_bytes();let tr=TransactionData{flags:TF_ACCEPT_FDS,data_size:data.len()as u64,offsets_size:8,buffer:data.as_ptr()as u64,offsets:offsets.as_ptr()as u64,..Default::default()};let mut write=vec![];command(&mut write,BC_TRANSACTION,&tr.encode());let reply=wait_transaction(&file,tid,exchange(&file,tid,&write,true),BR_REPLY);let(fd,got)=received(&reply);assert_eq!(got,expected);let duplicate=dup(fd);free(&file,tid,reply.buffer);close(fd);close(pair[0]);
            assert_eq!(super::super::fs::write([pair[1]as u64,b"payload".as_ptr()as u64,7,0,0,0]),7);let mut bytes=[0u8;7];assert_eq!(super::super::fs::read([duplicate as u64,bytes.as_mut_ptr()as u64,7,0,0,0]),7);assert_eq!(&bytes,b"payload");close(duplicate);super::super::net::drain_socket_carriers().unwrap();let mut byte=0u8;assert_eq!(super::super::fs::read([pair[1]as u64,&mut byte as*mut u8 as u64,1,0,0,0]),0);close(pair[1]);
        }
        assert_eq!(unsafe{libc::close(file.fd)},0);println!("BINDER_SOCKET_CHILD_EXECUTED {role}");
    }
}
