//! Host-call module [`aim_hostcall::module::BLUETOOTH`]: a virtual HCI
//! controller over CoreBluetooth for the guest `android.hardware.bluetooth`
//! service (`docs/bluetooth.md`).
//!
//! The original Android Bluetooth stack drives it with HCI as it would a
//! chip. [`controller`] answers as an LE-only controller, [`att`] plays each
//! connected device's ATT server over its GATT tree, and [`corebluetooth`]
//! is the radio. Controller-to-host packets wait in a queue; a pipe the
//! guest polls says when it is not empty.

mod adv;
mod att;
mod backend;
mod controller;
#[cfg(target_os = "macos")]
mod corebluetooth;
mod gatt;
mod hci;
mod l2cap;

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use aim_storage::private_fd::{PrivateFd, receive_allocations};
use std::sync::atomic::{AtomicU64, Ordering};

use aim_hostcall::bluetooth::{
    FN_CLOSE, FN_OPEN, FN_RECV, FN_SEND, MAX_PACKET, Packet, VERSION, kind,
};
use aim_hostcall::{HostModule, args_mut, errno, module};

pub use adv::Addr;
pub use backend::{Backend, Event, AdvertiseRequest as BackendAdvertisement, PeerId as BackendPeerId};
pub use att::Op as BackendOperation;
pub use gatt::Target as BackendTarget;
pub use controller::Controller;

pub static MODULE: HostModule = HostModule {
    id: module::BLUETOOTH,
    name: "bluetooth",
    version: VERSION,
    call,
};

fn neg(e: i32) -> i64 {
    -(e as i64)
}

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        FN_OPEN => open(),
        FN_CLOSE => {
            close();
            0
        }
        FN_SEND | FN_RECV => {
            // SAFETY: the registry passes the guest's argument block.
            let p = match unsafe { args_mut::<Packet>(args, len) } {
                Ok(p) => p,
                Err(e) => return e,
            };
            if p.data == 0 {
                return neg(errno::EINVAL);
            }
            if func == FN_SEND {
                if p.len as usize > MAX_PACKET {
                    return neg(errno::EINVAL);
                }
                // SAFETY: the guest's buffer of `len` bytes, for this call.
                let data =
                    unsafe { std::slice::from_raw_parts(p.data as *const u8, p.len as usize) };
                send(p.kind, data)
            } else {
                // SAFETY: the guest's buffer of `capacity` bytes.
                let buf = unsafe {
                    std::slice::from_raw_parts_mut(p.data as *mut u8, p.capacity as usize)
                };
                match recv(buf) {
                    Ok((k, n)) => {
                        p.kind = k;
                        p.len = n as u32;
                        0
                    }
                    Err(e) => neg(e),
                }
            }
        }
        _ => neg(errno::ENOSYS),
    }
}

struct Session {
    controller: Controller,
    queue: VecDeque<(u32, Vec<u8>)>,
    /// The pipe's write end: one byte in the pipe while `queue` is not
    /// empty.
    wake: PrivateFd,
    /// Our own copy of the read end, the guest's being the guest's.
    wake_read: PrivateFd,
    generation: u64,
}

impl Session {
    /// Queue what the controller produced.
    fn flush(&mut self) {
        let was_empty = self.queue.is_empty();
        self.queue.extend(self.controller.drain());
        if was_empty && !self.queue.is_empty() {
            // SAFETY: a byte into our non-blocking pipe; a full pipe
            // already wakes the reader.
            unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }
}

/// The ABI duplicates and publishes this borrowed native reader under its
/// descriptor lifecycle; module-private pipe endpoints never become guest fds.
pub struct Hooks { pub publish: fn(BorrowedFd<'_>) -> Result<i32, i32> }
static HOOKS: OnceLock<Hooks> = OnceLock::new();
pub fn set_hooks(hooks: Hooks) -> Result<(), Hooks> { HOOKS.set(hooks) }

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Run `f` on the open session of `generation`.
fn with_session(generation: u64, f: impl FnOnce(&mut Session)) {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = s.as_mut().filter(|s| s.generation == generation) {
        f(s);
        s.flush();
    }
}

fn pipe_error(error:std::io::Error)->i32 {
    match error.raw_os_error() {
        Some(libc::EMFILE)=>24, Some(libc::ENFILE)=>23, Some(libc::ENOMEM)=>errno::ENOMEM,
        Some(libc::EBADF)=>errno::EBADF, Some(libc::EINVAL)=>errno::EINVAL,
        _=>5,
    }
}
fn wake_pipe()->Result<(PrivateFd,PrivateFd),i32>{
    let(_,mut descriptors)=receive_allocations(|| {
        let mut raw=[-1i32;2];
        if unsafe{libc::pipe(raw.as_mut_ptr())}<0{return Err(std::io::Error::last_os_error());}
        let descriptors=raw.into_iter().map(|fd|unsafe{OwnedFd::from_raw_fd(fd)}).collect::<Vec<_>>();
        for descriptor in &descriptors {
            let fd=descriptor.as_raw_fd();let flags=unsafe{libc::fcntl(fd,libc::F_GETFL)};
            if flags<0{return Err(std::io::Error::last_os_error());}
            if unsafe{libc::fcntl(fd,libc::F_SETFD,libc::FD_CLOEXEC)}<0||unsafe{libc::fcntl(fd,libc::F_SETFL,flags|libc::O_NONBLOCK)}<0{return Err(std::io::Error::last_os_error());}
        }
        Ok(((),descriptors))
    }).map_err(pipe_error)?;
    let writer=descriptors.pop().unwrap();let reader=descriptors.pop().unwrap();Ok((reader,writer))
}
fn open_with(factory:impl FnOnce(u64)->Option<Box<dyn Backend>>,address:Addr,publish:fn(BorrowedFd<'_>)->Result<i32,i32>)->Result<i32,i32>{
    let mut slot=SESSION.lock().unwrap_or_else(|error|error.into_inner());
    if slot.is_some(){return Err(errno::EBUSY);}
    let(reader,writer)=wake_pipe()?;
    let generation=GENERATION.fetch_add(1,Ordering::Relaxed)+1;
    let backend=factory(generation).ok_or(errno::ENODEV)?;
    let session=Session{controller:Controller::new(backend,address),queue:VecDeque::new(),wake:writer,wake_read:reader,generation};
    let guest=publish(session.wake_read.as_fd())?;
    *slot=Some(session);Ok(guest)
}
/// Explicit backend dependency injection uses the same descriptor producer.
/// The default hostcall still requires the actual CoreBluetooth backend.
pub fn open_controller(backend:Box<dyn Backend>,address:Addr)->Result<i32,i32>{
    let hook=HOOKS.get().ok_or(errno::ENODEV)?;
    open_with(|_|Some(backend),address,hook.publish)
}
fn open()->i64 {
    let Some(hook)=HOOKS.get()else{return neg(errno::ENODEV)};
    open_with(new_backend,local_address(),hook.publish).map(|fd|fd as i64).unwrap_or_else(neg)
}

#[cfg(target_os = "macos")]
fn new_backend(generation: u64) -> Option<Box<dyn Backend>> {
    let sink = move |e: Event| with_session(generation, |s| s.controller.on_event(e));
    corebluetooth::CoreBluetooth::new(Box::new(sink)).map(|b| Box::new(b) as Box<dyn Backend>)
}

#[cfg(not(target_os = "macos"))]
fn new_backend(_generation: u64) -> Option<Box<dyn Backend>> {
    None
}

fn close() {
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mut s) = session {
        s.controller.reset();
    }
}

fn send(k: u32, data: &[u8]) -> i64 {
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = slot.as_mut() else {
        return neg(errno::ENODEV);
    };
    match k {
        kind::COMMAND => s.controller.command(data),
        kind::ACL => s.controller.acl(data),
        // No synchronous or isochronous link is ever set up.
        kind::SCO | kind::ISO => {}
        _ => return neg(errno::EINVAL),
    }
    s.flush();
    0
}

fn recv(buf: &mut [u8]) -> Result<(u32, usize), i32> {
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let s = slot.as_mut().ok_or(errno::ENODEV)?;
    let Some((k, packet)) = s.queue.front() else {
        return Err(errno::EAGAIN);
    };
    if packet.len() > buf.len() {
        return Err(errno::EMSGSIZE);
    }
    let (k, n) = (*k, packet.len());
    buf[..n].copy_from_slice(packet);
    s.queue.pop_front();
    if s.queue.is_empty() {
        let mut sink = [0u8; 64];
        // SAFETY: draining our non-blocking pipe into a local buffer.
        while unsafe { libc::read(s.wake_read.as_raw_fd(), sink.as_mut_ptr().cast(), sink.len()) } > 0 {}
    }
    Ok((k, n))
}

/// The controller's public address: derived from the Mac's hardware UUID,
/// so it is stable per Mac and never a real device's address.
fn local_address() -> Addr {
    let mut uuid = [0u8; 16];
    let timeout = libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    // SAFETY: a 16-byte buffer, as gethostuuid requires.
    unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &timeout) };
    Addr::derive(&uuid, false)
}

pub(crate) fn random(buf: &mut [u8]) {
    // SAFETY: fills our buffer.
    unsafe { libc::arc4random_buf(buf.as_mut_ptr().cast(), buf.len()) };
}

#[cfg(test)]
mod wake_owner_tests {
    use super::*;
    use std::sync::atomic::AtomicI32;
    static FAILED_READER:AtomicI32=AtomicI32::new(-1);
    static FAILED_OBSERVER:AtomicI32=AtomicI32::new(-1);
    fn publish(reader:BorrowedFd<'_>)->Result<i32,i32>{
        let duplicate=unsafe{libc::fcntl(reader.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};
        if duplicate<0{return Err(5);}Ok(duplicate)
    }
    fn fail(reader:BorrowedFd<'_>)->Result<i32,i32>{
        FAILED_READER.store(reader.as_raw_fd(),Ordering::Relaxed);
        let observer=publish(reader)?;FAILED_OBSERVER.store(observer,Ordering::Relaxed);Err(errno::ENOMEM)
    }
    #[test]
    fn injected_radio_wake_pipe_owner_publishes_only_reader_and_rolls_back_failure(){
        // This exercises FD ownership with the existing radio test backend;
        // it does not claim a real CoreBluetooth adapter is available.
        let address=Addr([1,2,3,4,5,0x02]);
        let guest=open_with(|_|Some(Box::new(controller::tests::Fake::default())),address,publish).unwrap();
        let slot=SESSION.lock().unwrap();let session=slot.as_ref().unwrap();
        let native_reader=session.wake_read.as_raw_fd();let native_writer=session.wake.as_raw_fd();
        assert_ne!(guest,native_reader);assert_ne!(guest,native_writer);
        for descriptor in [native_reader,native_writer]{assert_ne!(unsafe{libc::fcntl(descriptor,libc::F_GETFD)}&libc::FD_CLOEXEC,0);assert_ne!(unsafe{libc::fcntl(descriptor,libc::F_GETFL)}&libc::O_NONBLOCK,0);}
        let mut native_stat:libc::stat=unsafe{std::mem::zeroed()};let mut guest_stat:libc::stat=unsafe{std::mem::zeroed()};
        assert_eq!(unsafe{libc::fstat(native_reader,&mut native_stat)},0);assert_eq!(unsafe{libc::fstat(guest,&mut guest_stat)},0);
        assert_eq!(guest_stat.st_mode&libc::S_IFMT,libc::S_IFIFO);assert_eq!((guest_stat.st_dev,guest_stat.st_ino),(native_stat.st_dev,native_stat.st_ino));
        drop(slot);
        let mut poll=libc::pollfd{fd:guest,events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut poll,1,0)},0);
        let opcode=hci::cmd::READ_LOCAL_VERSION_INFORMATION;let command=[opcode as u8,(opcode>>8)as u8,0];
        assert_eq!(send(kind::COMMAND,&command),0);assert_eq!(unsafe{libc::poll(&mut poll,1,0)},1);
        let mut wake=0u8;assert_eq!(unsafe{libc::read(guest,(&mut wake as*mut u8).cast(),1)},1);assert_eq!(wake,1);
        assert_eq!(unsafe{libc::write(guest,(&wake as*const u8).cast(),1)},-1);assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::EBADF));
        let mut event=[0u8;64];let(event_kind,length)=recv(&mut event).unwrap();assert_eq!(event_kind,kind::EVENT);assert_eq!(event[0],hci::ev::COMMAND_COMPLETE);assert!(length>3);
        assert_eq!(recv(&mut event),Err(errno::EAGAIN));
        close();assert_eq!(unsafe{libc::fcntl(native_reader,libc::F_GETFD)},-1);assert_eq!(unsafe{libc::fcntl(native_writer,libc::F_GETFD)},-1);
        assert_eq!(unsafe{libc::read(guest,(&mut wake as*mut u8).cast(),1)},0);assert_eq!(unsafe{libc::close(guest)},0);
        assert_eq!(open_with(|_|Some(Box::new(controller::tests::Fake::default())),address,fail),Err(errno::ENOMEM));
        assert!(SESSION.lock().unwrap().is_none());assert_eq!(unsafe{libc::fcntl(FAILED_READER.load(Ordering::Relaxed),libc::F_GETFD)},-1);
        let observer=FAILED_OBSERVER.swap(-1,Ordering::Relaxed);assert!(observer>=0);assert_eq!(unsafe{libc::read(observer,(&mut wake as*mut u8).cast(),1)},0,"failed publication dropped the internal writer");assert_eq!(unsafe{libc::close(observer)},0);
        assert_eq!(open_with(|_|None,address,publish),Err(errno::ENODEV));assert!(SESSION.lock().unwrap().is_none());
    }
}
