//! Socket allocation receipts travel in one regular kernel lease descriptor.
//! Synchronous retirement releases backing after the last actual lease (#1191).
use crate::errno::{self, Errno};
use super::{socket_inode, super::{binder, fdtab}};
use aim_storage::private_fd::PrivateFd;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(super) struct Export {
    carrier: Option<PrivateFd>,
    _source: socket_inode::Export,
}
impl Export {
    pub(super) fn fd(&self) -> i32 { self.carrier.as_ref().unwrap().as_raw_fd() }
}
impl Drop for Export {
    fn drop(&mut self) {
        drop(self.carrier.take());
        super::super::close_effects::note_socket_close();
    }
}

#[derive(Default)]
struct Channel { endpoint: Option<PrivateFd> }
static CHANNEL: std::sync::Mutex<Channel> = std::sync::Mutex::new(Channel { endpoint: None });

pub(crate) fn reset() { CHANNEL.lock().unwrap().endpoint = None; }

fn io(error: std::io::Error) -> Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}
fn connect(fd: i32, endpoint: &aim_binder_host::socket_scm::Endpoint) -> Result<(), Errno> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 { return Err(errno::last()); }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 { return Err(errno::last()); }
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if endpoint.path.is_empty() || endpoint.path.len() >= address.sun_path.len() || endpoint.path.contains(&0) { return Err(errno::EINVAL); }
    address.sun_family = libc::AF_UNIX as _;
    address.sun_len = (2 + endpoint.path.len() + 1) as u8;
    for (target, source) in address.sun_path.iter_mut().zip(&endpoint.path) { *target = *source as _; }
    let result = unsafe { libc::connect(fd, (&address as *const libc::sockaddr_un).cast(), address.sun_len as _) };
    if result < 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO);
        if error != libc::EINPROGRESS && error != libc::EAGAIN { return Err(errno::from_darwin(error)); }
        let start = std::time::Instant::now();
        loop {
            let remaining = 1000i32 - start.elapsed().as_millis().min(1000) as i32;
            if remaining == 0 { return Err(errno::from_darwin(libc::ETIMEDOUT)); }
            let mut poll = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
            let ready = unsafe { libc::poll(&mut poll, 1, remaining) };
            if ready < 0 { let error = errno::last(); if error == errno::EINTR { continue; } return Err(error); }
            if ready == 0 { continue; }
            let mut error = 0i32; let mut len = 4u32;
            if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, (&mut error as *mut i32).cast(), &mut len) } < 0 { return Err(errno::last()); }
            if error != 0 { return Err(errno::from_darwin(error)); }
            break;
        }
    }
    Ok(())
}

fn wait_ready(fd:i32,event:i16,deadline:std::time::Instant)->Result<(),Errno>{
    loop {
        let remaining=deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero(){return Err(errno::from_darwin(libc::ETIMEDOUT));}
        let mut descriptor=libc::pollfd{fd,events:event,revents:0};
        let ready=unsafe{libc::poll(&mut descriptor,1,remaining.as_millis().clamp(1,1000)as i32)};
        if ready>0{return Ok(());}
        if ready<0{let error=errno::last();if error!=errno::EINTR{return Err(error);}}
    }
}

struct Reply { descriptor: Option<PrivateFd>, receipt: Option<aim_storage::socket_inode::Receipt>, class: u32 }
fn initialize(state: &mut Channel) -> Result<(), Errno> {
    if state.endpoint.is_none() {
        let endpoint = binder::socket_scm_endpoint()?;
        let descriptor = PrivateFd::allocate(|| {
            let descriptor = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
            if descriptor < 0 { return Err(std::io::Error::last_os_error()); }
            let descriptor=unsafe { OwnedFd::from_raw_fd(descriptor) };
            if unsafe { libc::fcntl(descriptor.as_raw_fd(),libc::F_SETFD,libc::FD_CLOEXEC) }<0{return Err(std::io::Error::last_os_error());}
            Ok(descriptor)
        }).map_err(io)?;
        connect(descriptor.as_raw_fd(), &endpoint).map_err(|error| { eprintln!("socket carrier reconnect connect: errno {error}"); error })?;
        aim_binder_host::socket_scm::authenticate(descriptor.as_raw_fd(), &endpoint).map_err(|error| { eprintln!("socket carrier reconnect authenticate: {error}"); io(error) })?;
        state.endpoint = Some(descriptor);
    }
    Ok(())
}
pub(crate) fn init() -> Result<(), Errno> { initialize(&mut CHANNEL.lock().unwrap()) }

fn exchange(request: impl FnMut(i32) -> std::io::Result<()>) -> Result<Reply, Errno> {
    let mut state = CHANNEL.lock().unwrap();
    initialize(&mut state)?;
    exchange_initialized(&mut state, request)
}
fn exchange_initialized(state: &mut Channel, mut request: impl FnMut(i32) -> std::io::Result<()>) -> Result<Reply, Errno> {
    let channel = state.endpoint.as_ref().unwrap().as_raw_fd();
    let result = (|| {
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(1);
        loop {
            match request(channel) {
                Ok(())=>break,
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>wait_ready(channel,libc::POLLOUT,deadline)?,
                Err(error) if error.kind()==std::io::ErrorKind::Interrupted=>continue,
                Err(error)=>return Err(io(error)),
            }
        }
        // Readiness waits stay outside admission; an EAGAIN receive did not
        // consume the reply and must not cause this request to be sent twice.
        let ((receipt, class), mut descriptors)=loop {
            if std::time::Instant::now()>=deadline{return Err(errno::from_darwin(libc::ETIMEDOUT));}
            aim_binder_host::socket_scm::wait_reply(channel).map_err(io)?;
            match aim_storage::private_fd::receive_allocations(|| {
                let reply=aim_binder_host::socket_scm::receive_reply(channel)?;
                if let Some(descriptor)=&reply.descriptor {
                    let flags=unsafe{libc::fcntl(descriptor.as_raw_fd(),libc::F_GETFD)};
                    if flags<0{return Err(std::io::Error::last_os_error());}
                    if unsafe{libc::fcntl(descriptor.as_raw_fd(),libc::F_SETFD,flags|libc::FD_CLOEXEC)}<0{return Err(std::io::Error::last_os_error());}
                }
                Ok(((reply.receipt,reply.class),reply.descriptor.into_iter().collect()))
            }) {
                Ok(reply)=>break reply,
                Err(error) if matches!(error.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::Interrupted)=>continue,
                Err(error)=>return Err(io(error)),
            }
        };
        Ok(Reply { descriptor: descriptors.pop(), receipt, class })
    })();
    if result.is_err() { state.endpoint = None; }
    result
}
pub(crate) fn drain() -> Result<(), Errno> {
    let mut state = CHANNEL.lock().unwrap();
    if state.endpoint.is_none() {
        if !binder::socket_carriers_configured() { return Ok(()); }
        initialize(&mut state)?;
    }
    let reply = exchange_initialized(&mut state, aim_binder_host::socket_scm::request_drain)?;
    if reply.class != 0 || reply.descriptor.is_some() || reply.receipt.is_some() { return Err(71); }
    Ok(())
}

pub(crate) fn classify(fd: i32) -> Result<u32, Errno> {
    let reply = exchange(|channel| aim_binder_host::socket_scm::request_classify(channel, fd))?;
    if reply.descriptor.is_some() { return Err(71); }
    Ok(reply.class)
}
pub(super) fn export(source: socket_inode::Export) -> Result<Export, Errno> {
    let mut reply = exchange(|channel| aim_binder_host::socket_scm::request_create(channel, source.backing_fd, source.receipt))?;
    if reply.class != aim_binder_host::socket_scm::CLASS || reply.receipt != Some(source.receipt) { return Err(71); }
    let carrier = reply.descriptor.take().ok_or(71)?;
    #[cfg(test)] assert_ne!(unsafe{libc::fcntl(carrier.as_raw_fd(),libc::F_GETFD)}&libc::FD_CLOEXEC,0,"internal C carrier must not escape an unrelated exec");
    Ok(Export { carrier: Some(carrier), _source: source })
}
fn install_backing(fd: i32, backing: &PrivateFd) -> Result<(), Errno> {
    let guard = fdtab::lifecycle();
    if fdtab::is_hidden(fd) || fdtab::visible(fd) { return Err(errno::EBADF); }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 { return Err(errno::last()); }
    let retired = fdtab::get(fd);
    fdtab::on_close(fd);
    let result = unsafe { libc::dup2(backing.as_raw_fd(), fd) };
    if result >= 0 { super::super::close_effects::note_socket_close(); }
    let error = if result < 0 { Some(errno::last()) }
        else if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 { Some(errno::last()) } else { None };
    drop(guard); drop(retired);
    match error { Some(error) => Err(error), None => Ok(()) }
}
pub(super) fn adopt(fd: i32) -> Result<(), Errno> {
    let mut reply = exchange(|channel| aim_binder_host::socket_scm::request_resolve(channel, fd))?;
    if reply.class != aim_binder_host::socket_scm::CLASS { return Err(71); }
    let receipt = reply.receipt.ok_or(71)?;
    let backing = reply.descriptor.take().ok_or(71)?;
    install_backing(fd, &backing)?;
    super::adopt(fd);
    socket_inode::install(fd, receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;

    #[test]
    fn socket_scm_actual_receipt_preserves_owner_flags_payload_and_final_close() {
        if fdtab::isolated_kernel_test("sys::net::socket_scm::tests::socket_scm_actual_receipt_preserves_owner_flags_payload_and_final_close") { return; }
        let (_view, _directory) = crate::vfs::test_view();
        let name = format!("dev.aim.test.socket-scm.{}", std::process::id());
        let _server = aim_binder_host::server::Server::start(&name).unwrap();
        binder::init(&name).unwrap();
        let pair = |flags: u64| {
            let mut pair = [-1i32; 2];
            assert_eq!(socketpair([L_AF_UNIX as u64,L_SOCK_STREAM|flags,0,pair.as_mut_ptr() as u64,0,0]),0);
            pair
        };
        let close = |fd:i32| assert_eq!(super::super::super::fs::close([fd as u64,0,0,0,0,0]),0);
        let subject = pair(L_SOCK_NONBLOCK);
        let mut resolver = super::super::super::cred::current(); resolver.cap_eff |= 1;
        assert_eq!(super::super::super::fsops::fchown_as([subject[0] as u64,1073,u32::MAX as u64,0,0,0],&resolver),0);
        let fdtab::Kind::Sock(owner) = fdtab::get(subject[0]).unwrap() else { panic!("real socket creator did not install state") };
        let receipt = owner.inode_allocation.lock().unwrap().unwrap(); drop(owner);
        let original = socket_inode::stat(subject[0]).unwrap().unwrap();
        let transfer = |channel:i32,fd:i32| {
            let mut control = [0u8;24];
            control[..8].copy_from_slice(&20u64.to_ne_bytes());control[8..12].copy_from_slice(&L_SOL_SOCKET.to_ne_bytes());
            control[12..16].copy_from_slice(&L_SCM_RIGHTS.to_ne_bytes());control[16..20].copy_from_slice(&fd.to_ne_bytes());
            let mut byte=[b'q'];let iov=libc::iovec{iov_base:byte.as_mut_ptr().cast(),iov_len:1};
            let message=LinuxMsghdr{name:0,namelen:0,_pad:0,iov:&iov as *const _ as u64,iovlen:1,control:control.as_mut_ptr() as u64,controllen:24,flags:0,_pad2:0};
            assert_eq!(sendmsg([channel as u64,&message as *const _ as u64,0,0,0,0]),1);
        };
        let receive = |channel:i32,flags:u64| {
            let mut control=[0u8;24];let mut byte=[0u8];let iov=libc::iovec{iov_base:byte.as_mut_ptr().cast(),iov_len:1};
            let mut message=LinuxMsghdr{name:0,namelen:0,_pad:0,iov:&iov as *const _ as u64,iovlen:1,control:control.as_mut_ptr() as u64,controllen:24,flags:0,_pad2:0};
            assert_eq!(recvmsg([channel as u64,&mut message as *mut _ as u64,flags,0,0,0]),1);
            assert_eq!(byte,[b'q']);assert_eq!(message.controllen,24);assert_eq!(message.flags&L_MSG_CTRUNC,0);
            let fd=i32::from_ne_bytes(control[16..20].try_into().unwrap());
            let fdtab::Kind::Sock(owner)=fdtab::get(fd).unwrap() else {panic!("opaque carrier escaped as the guest socket")};
            assert_eq!(*owner.inode_allocation.lock().unwrap(),Some(receipt));
            let imported=socket_inode::stat(fd).unwrap().unwrap();
            assert_eq!((imported.st_ino,imported.st_uid,imported.st_gid),(original.st_ino,1073,original.st_gid));
            assert_ne!(unsafe{libc::fcntl(fd,libc::F_GETFL)}&libc::O_NONBLOCK,0);
            assert_eq!(unsafe{libc::fcntl(fd,libc::F_GETFD)}&libc::FD_CLOEXEC,if flags&L_MSG_CMSG_CLOEXEC!=0{libc::FD_CLOEXEC}else{0});
            fd
        };
        let live = |peer:i32, stage:&str| {
            let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer,(&mut byte as *mut u8).cast(),1,libc::MSG_DONTWAIT)},-1,"{stage}");
            assert_eq!(errno::last(),errno::EAGAIN);
        };
        let first=pair(0);transfer(first[0],subject[0]);
        assert_ne!(unsafe{libc::fcntl(subject[0],libc::F_GETFL)}&libc::O_NONBLOCK,0);
        close(subject[0]);live(subject[1],"queued before init refresh");
        // Real init/refresh must not Mach-classify any live socket while the
        // queued carrier is the sole owner of the original backing endpoint.
        let refresh = (0..4).map(|_| {
            let name = name.clone();
            std::thread::spawn(move || { for _ in 0..4 { binder::init(&name).unwrap(); } })
        }).collect::<Vec<_>>();
        for thread in refresh { thread.join().unwrap(); }
        live(subject[1],"queued after parallel init refresh");let received=receive(first[1],L_MSG_CMSG_CLOEXEC);
        assert_eq!(sendto([received as u64,b"real".as_ptr() as u64,4,0,0,0]),4);
        let mut bytes=[0u8;4];assert_eq!(unsafe{libc::recv(subject[1],bytes.as_mut_ptr().cast(),4,libc::MSG_DONTWAIT)},4);assert_eq!(&bytes,b"real");
        let second=pair(0);transfer(second[0],received);close(received);live(subject[1],"queued after reexport");let received=receive(second[1],0);
        let discard=pair(0);transfer(discard[0],received);close(received);live(subject[1],"queued before discard");
        // Retiring a broken control channel must not turn configured drain
        // into a no-op while this actual queued carrier keeps B alive.
        assert!(classify(-1).is_err());
        close(discard[1]);
        let mut byte=0u8;
        let eof=unsafe{libc::recv(subject[1],(&mut byte as *mut u8).cast(),1,libc::MSG_DONTWAIT)};
        if eof != 0 {
            for fd in fdtab::open_fds() {
                if receipt.validate(fd).is_ok() {
                    let kind=match fdtab::get(fd){Some(fdtab::Kind::Sock(_))=>"Sock",Some(_)=>"other",None=>"none"};
                    eprintln!("EOF live backing fd={fd} hidden={} visible={} kind={kind}",fdtab::is_hidden(fd),fdtab::visible(fd));
                }
                if let Some(path)=crate::xrt::fd_path(fd) {
                    if path.as_os_str().to_string_lossy().contains("aim-socket-lease-") { eprintln!("EOF live carrier fd={fd} hidden={} visible={} path={}",fdtab::is_hidden(fd),fdtab::visible(fd),path.display()); }
                }
            }
        }
        assert_eq!(eof,0,"no native registry backing may defer real peer EOF");
        for fd in [subject[1],first[0],first[1],second[0],second[1],discard[0]] {close(fd);}
    }
}
