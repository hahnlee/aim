//! The daemon side: one [`Driver`] for every guest process of a profile.
//!
//! - The **service port** (a bootstrap name) takes `OPEN`. Each open file
//!   gets a **file port** (`THREAD`, `MMAP`, `POLL`) and a readiness socket
//!   whose other end is the guest's binder fd.
//! - The service port also answers `FILES`: the files whose pages this
//!   process shares as memory objects ([`Server::share_file`]).
//! - Each guest thread that issues binder ioctls gets a **thread port**. A
//!   small pool of daemon threads receives on the set of all thread ports
//!   and runs the ioctls; none blocks in the driver ([`Worker`]).
//! - **Release** is the last close of the guest's binder fd, which includes
//!   process death: the daemon's end of the readiness socket reads EOF.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use aim_binder_driver::{
    Credentials, Device, Driver, Errno, File, GuestProcess, ProcHandle, ReceiveMemory, Tid, errno,
};

use crate::mach::{self, Buffer, Msg, Port, Received};
use crate::wire::{self, Ioctl, IoctlReply, Reader, SharedFile, Writer};

/// A fileport as the driver's opaque `File`.
struct FilePort(Port, u32, Option<RegularPorts>, Option<aim_storage::socket_inode::Receipt>);
struct RegularPorts { metadata: wire::RegularMetadata, writer: Option<Port> }
impl Drop for RegularPorts { fn drop(&mut self) { if let Some(port) = self.writer { mach::release_send(port); } } }


impl Drop for FilePort {
    fn drop(&mut self) {
        mach::release_send(self.0);
    }
}

/// Retain a host file descriptor as a Binder file without taking the caller's
/// fd. The returned fileport keeps the file alive while it crosses Binder.
pub fn file_from_fd(fd: std::os::fd::BorrowedFd<'_>) -> Option<File> {
    use std::os::fd::AsRawFd;
    let class = registered_descriptor_class(fd.as_raw_fd()).ok()?;
    if class==crate::socket_scm::CLASS{
        let(backing,receipt)=crate::socket_scm::binder_from_carrier(fd).ok()?;
        return mach::fd_to_port(backing.as_raw_fd()).map(|port|Arc::new(FilePort(port,class,None,Some(receipt)))as File);
    }
    mach::fd_to_port(fd.as_raw_fd()).map(|port| Arc::new(FilePort(port, class, None,None)) as File)
}

/// Export a native proxy capability with an explicit versioned Binder class.
pub fn proxy_file_from_fd(fd: std::os::fd::BorrowedFd<'_>) -> Option<File> {
    use std::os::fd::AsRawFd;
    if crate::proxy_file::registered_class(fd.as_raw_fd()) != crate::proxy_file::CLASS {
        return None;
    }
    mach::fd_to_port(fd.as_raw_fd())
        .map(|port| Arc::new(FilePort(port, crate::proxy_file::CLASS, None,None)) as File)
}

fn path_creation_errno(error: std::io::Error) -> Errno {
    match error.raw_os_error() {
        Some(libc::EPROTO) => wire::EPROTO,
        Some(libc::EAGAIN) => 11,
        Some(libc::ENOTSOCK) => 88,
        Some(libc::ECONNRESET) => 104,
        Some(value)
            if matches!(
                value,
                libc::EPERM
                    | libc::ENOENT
                    | libc::EINTR
                    | libc::EIO
                    | libc::EBADF
                    | libc::ENOMEM
                    | libc::EACCES
                    | libc::EINVAL
                    | libc::ENFILE
                    | libc::EMFILE
                    | libc::EPIPE
            ) =>
        {
            value
        }
        _ => 5,
    }
}

fn create_path_fileport(port: Port, flags: u32) -> Result<(Vec<(Port, u32)>, Vec<u8>), Errno> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let fd = mach::port_to_fd(port).ok_or(errno::EBADF)?;
    let capability = crate::path_file::create(unsafe { OwnedFd::from_raw_fd(fd) }, flags)
        .map_err(path_creation_errno)?;
    let port = mach::fd_to_port(capability.carrier.as_raw_fd()).ok_or(errno::EBADF)?;
    let mut out = Writer::default();
    out.u32(crate::path_file::CLASS);
    Ok((vec![(port, mach::MOVE_SEND)], out.0))
}

fn scm_control(req:&mach::Received)->Result<crate::regular_scm::ReplyGuard,Errno>{
    use std::os::fd::{AsFd,FromRawFd,OwnedFd};
    let convert=|port|mach::port_to_fd(port).map(|fd|unsafe{OwnedFd::from_raw_fd(fd)}).ok_or(errno::EBADF);
    match req.id {
        wire::CREATE_REGULAR_SCM=>{
            let mut reader=Reader::new(&req.data);let metadata=wire::RegularMetadata::decode(&mut reader)?;
            if reader.remaining()!=0||req.ports.len()!=1+metadata.writer as usize{return Err(wire::EPROTO);}
            let backing=convert(req.ports[0])?;let writer=if metadata.writer{Some(convert(req.ports[1])?)}else{None};
            crate::regular_scm::create(backing,writer,metadata).map_err(path_creation_errno)
        }
        wire::RESOLVE_REGULAR_SCM=>{
            if req.ports.len()!=1||!req.data.is_empty(){return Err(wire::EPROTO);}
            let carrier=convert(req.ports[0])?;crate::regular_scm::resolve(carrier.as_fd()).map_err(path_creation_errno)
        }
        _=>Err(wire::EPROTO),
    }
}

fn serve_regular_control(req:&mach::Received,buffer:&mut Buffer){
    let mut guard=None;
    let result=if req.id==wire::DRAIN_REGULAR_SCM{
        if !req.ports.is_empty()||req.data.len()!=36{Err(wire::EPROTO)}else{
            crate::regular_scm::drain(req.data[..].try_into().unwrap()).map(|_|(Vec::new(),Vec::new())).map_err(path_creation_errno)
        }
    }else{scm_control(req).map(|reply|{let ports=reply.ports();let data=reply.data().to_vec();guard=Some(reply);(ports,data)})};
    for port in &req.ports{mach::release_send(*port);}
    let mut data=Writer::default();let (ports,extra)=match result{Ok(result)=>{data.i32(0);result},Err(error)=>{data.i32(error);(vec![],vec![])}};
    data.0.extend_from_slice(&extra);
    let message=Msg{id:wire::REPLY,ports,data:data.0};
    if let Err(error)=mach::reply_bounded(buffer,req.reply,&message,1000){mach::release_send(req.reply);eprintln!("regular SCM reply failed: Mach {error}");}
    drop(guard);
}

pub(crate) fn socket_scm_errno(error:std::io::Error)->i32{path_creation_errno(error)}
pub(crate) fn socket_scm_darwin_errno(errno:i32)->i32{match errno{71=>libc::EPROTO,11=>libc::EAGAIN,88=>libc::ENOTSOCK,104=>libc::ECONNRESET,other=>other}}
pub(crate) fn registered_descriptor_class(fd: i32) -> std::io::Result<u32> {
    let proxy = crate::proxy_file::registered_class_result(fd)?;
    if proxy != 0 {
        return Ok(proxy);
    }
    let path=crate::path_file::registered_class_result(fd)?;
    if path!=0{return Ok(path);}
    let regular=crate::regular_scm::registered_class(fd)?;if regular!=0{return Ok(regular);}
    crate::socket_scm::registered_class(fd)
}

pub fn path_file_from_fd(fd: std::os::fd::BorrowedFd<'_>) -> Option<File> {
    use std::os::fd::AsRawFd;
    if registered_descriptor_class(fd.as_raw_fd()).ok() != Some(crate::path_file::CLASS) {
        return None;
    }
    mach::fd_to_port(fd.as_raw_fd())
        .map(|port| Arc::new(FilePort(port, crate::path_file::CLASS, None,None)) as File)
}

/// Capture the actual regular backing and writer descriptions together. They
/// remain owned by the driver's File Arc through queued and delivered buffers.
pub fn regular_file_from_fd(backing: std::os::fd::BorrowedFd<'_>, writer: Option<std::os::fd::BorrowedFd<'_>>, metadata: wire::RegularMetadata) -> Result<File, Errno> {
    use std::os::fd::AsRawFd;
    crate::regular_file::validate(backing, writer, &metadata).map_err(path_creation_errno)?;
    let backing = mach::fd_to_port(backing.as_raw_fd()).ok_or(errno::EBADF)?;
    let writer = match writer {
        Some(writer) => match mach::fd_to_port(writer.as_raw_fd()) { Some(port) => Some(port), None => { mach::release_send(backing); return Err(errno::EBADF); } },
        None => None,
    };
    Ok(Arc::new(FilePort(backing, crate::regular_file::CLASS, Some(RegularPorts { metadata, writer }),None)) as File)
}

pub fn file_class(file: &File) -> Option<u32> {
    file.downcast_ref::<FilePort>().map(|file| file.1)
}

/// A native descriptor retains the actual driver owner through its final close.
/// Extracting a plain OwnedFd is allowed only for capabilities without regular
/// metadata; a regular writer lease cannot be separated from its backing.
pub struct RetainedFd { backing: std::fs::File, owner: File }
impl RetainedFd {
    fn access(&self,write:bool)->std::io::Result<()>{
        if let Some(metadata)=self.owner.downcast_ref::<FilePort>().and_then(|file|file.2.as_ref()).map(|regular|&regular.metadata){
            let mode=metadata.flags&3;
            if !(if write{matches!(mode,1|2)}else{matches!(mode,0|2)}){return Err(std::io::Error::from_raw_os_error(libc::EBADF));}
        }
        Ok(())
    }
    pub fn into_file_owner(self) -> File { self.owner }
    pub fn try_clone(&self) -> std::io::Result<Self> { Ok(Self { backing:self.backing.try_clone()?, owner:self.owner.clone() }) }
    pub fn metadata(&self) -> std::io::Result<std::fs::Metadata> { self.backing.metadata() }
    pub fn set_len(&self, length:u64) -> std::io::Result<()> { self.access(true)?;self.backing.set_len(length) }
    pub fn sync_all(&self) -> std::io::Result<()> { self.backing.sync_all() }
    pub fn into_owned_fd(self) -> std::io::Result<std::os::fd::OwnedFd> {
        if self.owner.downcast_ref::<FilePort>().is_some_and(|file| file.1==crate::regular_file::CLASS) {
            return Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP));
        }
        Ok(self.backing.into())
    }
}
impl std::os::fd::AsRawFd for RetainedFd { fn as_raw_fd(&self)->i32 { std::os::fd::AsRawFd::as_raw_fd(&self.backing) } }
impl std::os::fd::AsFd for RetainedFd { fn as_fd(&self)->std::os::fd::BorrowedFd<'_> { std::os::fd::AsFd::as_fd(&self.backing) } }
impl std::io::Read for RetainedFd { fn read(&mut self, bytes:&mut[u8])->std::io::Result<usize> { self.access(false)?;std::io::Read::read(&mut self.backing,bytes) } }
impl std::io::Write for RetainedFd {
    fn write(&mut self, bytes:&[u8])->std::io::Result<usize> { self.access(true)?;std::io::Write::write(&mut self.backing,bytes) }
    fn flush(&mut self)->std::io::Result<()> { std::io::Write::flush(&mut self.backing) }
}
impl std::io::Seek for RetainedFd { fn seek(&mut self, position:std::io::SeekFrom)->std::io::Result<u64> { std::io::Seek::seek(&mut self.backing,position) } }
impl std::os::unix::fs::FileExt for RetainedFd {
    fn read_at(&self, bytes:&mut[u8], offset:u64)->std::io::Result<usize> { self.access(false)?;std::os::unix::fs::FileExt::read_at(&self.backing,bytes,offset) }
    fn write_at(&self, bytes:&[u8], offset:u64)->std::io::Result<usize> { self.access(true)?;std::os::unix::fs::FileExt::write_at(&self.backing,bytes,offset) }
}

pub fn file_fd(file: &File) -> Option<RetainedFd> {
    let port = file.downcast_ref::<FilePort>()?.0;
    use std::os::fd::FromRawFd;
    mach::port_to_fd(port).map(|fd| RetainedFd { backing:unsafe { std::fs::File::from_raw_fd(fd) }, owner:file.clone() })
}

/// A receive buffer: shared memory mapped read-write here and read-only in
/// the guest.
struct SharedReceive {
    addr: u64,
    len: usize,
}

impl ReceiveMemory for SharedReceive {
    fn write(&self, offset: usize, data: &[u8]) {
        assert!(
            offset
                .checked_add(data.len())
                .is_some_and(|e| e <= self.len)
        );
        // SAFETY: bounds checked; the mapping lives as long as `self`.
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                (self.addr as *mut u8).add(offset),
                data.len(),
            )
        };
    }
}

impl Drop for SharedReceive {
    fn drop(&mut self) {
        mach::unmap(self.addr, self.len as u64);
    }
}

/// Level-triggered readiness of a binder fd: one byte in the socket while a
/// read by the polling thread would find work (`binder_poll`).
struct Readiness {
    socket: i32,
    ready: bool,
    poll_tid: Option<Tid>,
}

impl Readiness {
    fn raise(&mut self) {
        if !self.ready {
            self.ready = true;
            // SAFETY: writing one byte to our end of the socket.
            unsafe { libc::write(self.socket, [1u8].as_ptr().cast(), 1) };
        }
    }

    /// Returns the number of bytes the guest must drain.
    fn set(&mut self, ready: bool) -> u32 {
        if ready {
            self.raise();
            0
        } else if std::mem::take(&mut self.ready) {
            1
        } else {
            0
        }
    }
}

struct OpenFile {
    handle: ProcHandle,
    readiness: Mutex<Readiness>,
    /// Thread ports served for this file, destroyed at release.
    threads: Mutex<Vec<Port>>,
}

impl OpenFile {
    /// Re-evaluate readiness for the polling thread, with the readiness
    /// lock held across the driver's poll so a concurrent wake is not lost.
    fn refresh(&self, driver: &Driver) -> u32 {
        let mut r = self.readiness.lock().unwrap();
        match r.poll_tid {
            Some(tid) => {
                let ready = driver.poll(self.handle, tid).unwrap_or(true);
                r.set(ready)
            }
            None => 0,
        }
    }
}

/// Ports to send back, and payload after the status word.
type ControlReply = (Vec<(Port, u32)>, Vec<u8>);

pub struct Server {
    socket_service:crate::socket_scm::Service,
    driver: Arc<Driver>,
    service: Port,
    set: Port,
    files: Mutex<HashMap<Port, Arc<OpenFile>>>,
    kq: i32,
    shared: Mutex<Vec<SharedFile>>,
    pool: Arc<Pool>,
}

/// The thread ports, served by the workers.
struct Pool {
    driver: Arc<Driver>,
    /// The port set of every thread port.
    set: Port,
    threads: RwLock<HashMap<Port, (Arc<OpenFile>, Tid)>>,
}

impl Pool {
    /// Stop serving a thread port, unless release already destroyed it.
    fn forget(&self, port: Port) {
        if let Some((file, _)) = self.threads.write().unwrap().remove(&port) {
            file.threads.lock().unwrap().retain(|p| *p != port);
            mach::destroy_receive(port);
        }
    }
}

impl Server {
    /// Check in `name` and start serving on background threads.
    pub fn start(name: &str) -> Result<Arc<Self>, String> {
        let service =
            mach::check_in(name).map_err(|kr| format!("bootstrap_check_in {name}: {kr:#x}"))?;
        let set = mach::new_port_set().map_err(|kr| format!("port set: {kr:#x}"))?;
        mach::move_member(service, set).map_err(|kr| format!("port set: {kr:#x}"))?;
        let driver = Driver::new();
        let pool = Arc::new(Pool {
            driver: driver.clone(),
            set: mach::new_port_set().map_err(|kr| format!("port set: {kr:#x}"))?,
            threads: RwLock::new(HashMap::new()),
        });
        // SAFETY: plain kqueue.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err("kqueue failed".into());
        }
        let server = Arc::new(Self {
            socket_service:crate::socket_scm::Service::start().map_err(|error|format!("socket SCM listener: {error}"))?,
            driver,
            service,
            set,
            files: Mutex::new(HashMap::new()),
            kq,
            shared: Mutex::new(Vec::new()),
            pool: pool.clone(),
        });
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()) {
            let worker = Worker(pool.clone());
            std::thread::Builder::new()
                .name("binder-worker".into())
                .spawn(move || worker.run())
                .map_err(|e| e.to_string())?;
        }
        let s = server.clone();
        std::thread::Builder::new()
            .name("binder-control".into())
            .spawn(move || s.control_loop())
            .map_err(|e| e.to_string())?;
        let s = server.clone();
        std::thread::Builder::new()
            .name("binder-release".into())
            .spawn(move || s.release_loop())
            .map_err(|e| e.to_string())?;
        Ok(server)
    }

    fn control_loop(self: Arc<Self>) {
        let mut buf = Buffer::default();
        loop {
            let Ok(req) = mach::receive(&mut buf, self.set) else {
                continue;
            };
            if req.local==self.service&&matches!(req.id,wire::CREATE_REGULAR_SCM|wire::RESOLVE_REGULAR_SCM|wire::DRAIN_REGULAR_SCM){
                serve_regular_control(&req,&mut buf);continue;
            }
            let result = if req.local==self.service&&req.id==wire::SOCKET_SCM_CHANNEL {
                if !req.ports.is_empty()||!req.data.is_empty(){Err(wire::EPROTO)}else{Ok((Vec::new(),self.socket_service.endpoint().encode()))}
            } else if req.local == self.service && req.id == wire::CREATE_PATH {
                if req.ports.len() != 1 || req.data.len() != 4 {
                    Err(wire::EPROTO)
                } else {
                    create_path_fileport(
                        req.ports[0],
                        u32::from_le_bytes(req.data[..4].try_into().unwrap()),
                    )
                }
            } else if req.local == self.service && req.id == wire::FILE_CLASS {
                if req.ports.len() != 1 || !req.data.is_empty() {
                    Err(wire::EPROTO)
                } else if let Some(fd) = mach::port_to_fd(req.ports[0]) {
                    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
                    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                    let mut out = Writer::default();
                    match registered_descriptor_class(fd.as_raw_fd()) {
                        Ok(class) => {
                            out.u32(class);
                            Ok((Vec::new(), out.0))
                        }
                        Err(_) => Err(5),
                    }
                } else {
                    Err(errno::EBADF)
                }
            } else if req.local == self.service && req.id == wire::FILES {
                self.shared_files(&req)
            } else if req.local == self.service {
                self.open(&req)
            } else {
                let file = self.files.lock().unwrap().get(&req.local).cloned();
                match file {
                    Some(f) => self.file_request(&f, &req),
                    None => Err(errno::EBADF),
                }
            };
            for p in &req.ports {
                mach::release_send(*p);
            }
            let mut w = Writer::default();
            let (ports, extra) = match result {
                Ok(r) => {
                    w.i32(0);
                    r
                }
                Err(e) => {
                    w.i32(e);
                    (Vec::new(), Vec::new())
                }
            };
            w.0.extend_from_slice(&extra);
            let msg = Msg {
                id: wire::REPLY,
                ports,
                data: w.0,
            };
            mach::reply(&mut buf, req.reply, &msg);
        }
    }

    /// The driver every guest process of this server opens.
    pub fn driver(&self) -> &Arc<Driver> {
        &self.driver
    }

    /// Share the pages of the file `file.dev`/`file.ino` with every guest:
    /// `file.entry` is a read-only memory entry of this process's shared
    /// mapping of it ([`mach::share_read_only`]), which the server keeps.
    pub fn share_file(&self, file: SharedFile) {
        self.shared.lock().unwrap().push(file);
    }

    fn shared_files(&self, req: &Received) -> Result<ControlReply, Errno> {
        let first = Reader::new(&req.data).u32()? as usize;
        let shared = self.shared.lock().unwrap();
        let mut w = Writer::default();
        w.u32(shared.len() as u32);
        let mut ports = Vec::new();
        for f in shared.iter().skip(first).take(mach::MAX_PORTS) {
            w.u64(f.dev).u64(f.ino).u64(f.size);
            ports.push((f.entry, mach::COPY_SEND));
        }
        Ok((ports, w.0))
    }

    fn open(&self, req: &Received) -> Result<ControlReply, Errno> {
        let parse = || -> Result<(Device, bool, u32, Option<String>), Errno> {
            let mut r = Reader::new(&req.data);
            let device = match r.u32()? {
                0 => Device::Binder,
                1 => Device::HwBinder,
                2 => Device::VndBinder,
                _ => return Err(errno::EINVAL),
            };
            let nonblocking = r.u32()? != 0;
            let euid = r.u32()?;
            let ctx = r.bytes()?;
            let ctx = (!ctx.is_empty()).then(|| String::from_utf8_lossy(ctx).into_owned());
            Ok((device, nonblocking, euid, ctx))
        };
        let (device, nonblocking, euid, security_context) = parse()?;
        let socket = req
            .ports
            .first()
            .and_then(|p| mach::port_to_fd(*p))
            .ok_or(errno::EBADF)?;
        let Ok(port) = mach::new_port(false) else {
            // SAFETY: closing the fd we just made.
            unsafe { libc::close(socket) };
            return Err(errno::ENOMEM);
        };
        let creds = Credentials {
            pid: req.pid,
            euid,
            security_context,
        };
        let handle = self.driver.open(device, creds);
        self.driver.set_nonblocking(handle, nonblocking);
        let file = Arc::new(OpenFile {
            handle,
            readiness: Mutex::new(Readiness {
                socket,
                ready: false,
                poll_tid: None,
            }),
            threads: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&file);
        self.driver.set_notifier(
            handle,
            Arc::new(move |_| {
                if let Some(f) = weak.upgrade() {
                    f.readiness.lock().unwrap().raise();
                }
            }),
        );
        self.files.lock().unwrap().insert(port, file);
        let _ = mach::move_member(port, self.set);
        let ev = libc::kevent {
            ident: socket as usize,
            filter: libc::EVFILT_READ,
            flags: libc::EV_ADD,
            fflags: 0,
            data: 0,
            udata: port as usize as *mut _,
        };
        // SAFETY: registering our socket with our kqueue.
        unsafe { libc::kevent(self.kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
        Ok((vec![(port, mach::MAKE_SEND)], Vec::new()))
    }

    fn file_request(&self, file: &Arc<OpenFile>, req: &Received) -> Result<ControlReply, Errno> {
        let mut r = Reader::new(&req.data);
        match req.id {
            wire::THREAD => {
                let tid = r.i32()?;
                let port = mach::new_port(false).map_err(|_| errno::ENOMEM)?;
                file.threads.lock().unwrap().push(port);
                let thread = (file.clone(), tid);
                self.pool.threads.write().unwrap().insert(port, thread);
                if mach::move_member(port, self.pool.set).is_err() {
                    self.pool.forget(port);
                    return Err(errno::ENOMEM);
                }
                Ok((vec![(port, mach::MAKE_SEND)], Vec::new()))
            }
            wire::MMAP => {
                let (vm_start, len) = (r.u64()?, r.u64()?);
                let len = len.min(aim_binder_driver::MAX_MAPPING as u64);
                let (entry, addr) = mach::new_shared_memory(len).map_err(|_| errno::ENOMEM)?;
                let memory = Arc::new(SharedReceive {
                    addr,
                    len: len as usize,
                });
                match self
                    .driver
                    .mmap(file.handle, vm_start, len as usize, false, memory)
                {
                    Ok(()) => Ok((vec![(entry, mach::MOVE_SEND)], Vec::new())),
                    Err(e) => {
                        mach::release_send(entry);
                        Err(e)
                    }
                }
            }
            wire::INTERRUPT => {
                self.driver.interrupt(file.handle, r.i32()?);
                Ok((Vec::new(), Vec::new()))
            }
            wire::POLL => {
                let tid = r.i32()?;
                file.readiness.lock().unwrap().poll_tid = Some(tid);
                let mut w = Writer::default();
                w.u32(file.refresh(&self.driver));
                Ok((Vec::new(), w.0))
            }
            _ => Err(errno::EINVAL),
        }
    }

    /// Release files whose guest side is closed.
    fn release_loop(self: Arc<Self>) {
        loop {
            // SAFETY: an event buffer on our stack.
            let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
            let n =
                unsafe { libc::kevent(self.kq, std::ptr::null(), 0, &mut ev, 1, std::ptr::null()) };
            if n != 1 {
                continue;
            }
            let fd = ev.ident as i32;
            let mut sink = [0u8; 64];
            // SAFETY: draining our end of the socket (the guest never
            // writes to it; this only notices EOF).
            let got = unsafe { libc::read(fd, sink.as_mut_ptr().cast(), sink.len()) };
            if ev.flags & libc::EV_EOF == 0 && got != 0 {
                continue;
            }
            let port = ev.udata as usize as Port;
            let Some(file) = self.files.lock().unwrap().remove(&port) else {
                continue;
            };
            self.driver.release(file.handle);
            mach::destroy_receive(port);
            let threads = std::mem::take(&mut *file.threads.lock().unwrap());
            for t in threads {
                self.pool.forget(t);
            }
            // SAFETY: closing our end; kqueue drops the registration.
            unsafe { libc::close(fd) };
        }
    }
}

fn valid_exported_files(io: &Ioctl, ports: &[Port]) -> bool {
    if io.validate().is_err() { return false; }
    let count = io.fds.len() + io.regular.iter().flatten().filter(|metadata| metadata.writer).count();
    if count != ports.len() { return false; }
    let mut ports = ports.iter().copied();
    io.file_classes.iter().zip(&io.regular).zip(&io.sockets).all(|((class, regular),socket)| {
        use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
        let Some(backing) = ports.next().and_then(mach::port_to_fd) else { return false; };
        let backing = unsafe { OwnedFd::from_raw_fd(backing) };
        if let Some(metadata) = regular {
            let writer = if metadata.writer {
                let Some(fd) = ports.next().and_then(mach::port_to_fd) else { return false; };
                Some(unsafe { OwnedFd::from_raw_fd(fd) })
            } else { None };
            *class == crate::regular_file::CLASS && crate::regular_file::validate(backing.as_fd(), writer.as_ref().map(AsFd::as_fd), metadata).is_ok()
        } else if let Some(receipt)=socket{
            *class==crate::socket_scm::CLASS&&crate::socket_scm::validate_binder_backing(backing.as_fd(),*receipt).is_ok()
        } else {
            registered_descriptor_class(backing.as_raw_fd()).is_ok_and(|actual| actual == *class)
        }
    })
}

/// Serves the ioctls of any guest thread: a guest thread has one ioctl in
/// flight at a time, and a worker never blocks, so one worker per CPU
/// serves them all. A read that would wait parks instead
/// ([`Driver::ioctl_or_park`]), and the thread that brings its work
/// answers it.
///
/// A worker sends the answers of the calls it resumed, and its own, when it
/// is done with the request. One goes out in the `mach_msg` that waits for
/// the next request, so the thread it wakes can take this CPU as the worker
/// blocks, as Linux wakes the target of a binder call synchronously: a
/// resumed one, whose thread waits for a transaction or a reply, before the
/// worker's own.

struct Worker(Arc<Pool>);

impl Worker {
    fn run(self) {
        OUTBOX.with(|o| *o.borrow_mut() = Some(Vec::new()));
        let mut buf = Buffer::default();
        // Replies without retained files combine send and receive.
        let mut answer: Option<Answer> = None;
        loop {
            let next = match answer.take() {
                Some(a) if !a.files.is_empty() => {
                    // COPY_SEND takes its queue reference during send. Release
                    // the owner before waiting for an unrelated next request.
                    mach::reply(&mut buf,a.to,&a.msg);
                    drop(a);
                    mach::receive(&mut buf,self.0.set)
                }
                Some(a) => mach::reply_and_receive(&mut buf,a.to,&a.msg,self.0.set),
                None => mach::receive(&mut buf, self.0.set),
            };
            let Ok(req) = next else { continue };
            let port = req.local;
            let own = match self.call(req) {
                Ok(call) => call.run(),
                Err(a) => Some(a),
            };
            let mut resumed = OUTBOX.with(|o| std::mem::take(o.borrow_mut().as_mut().unwrap()));
            if own.as_ref().is_some_and(|a| a.exit) {
                resumed.into_iter().chain(own).for_each(Answer::send);
                self.0.forget(port);
                continue;
            }
            answer = match resumed.pop() {
                Some(last) => {
                    resumed.into_iter().chain(own).for_each(Answer::send);
                    Some(last)
                }
                None => own,
            };
        }
    }

    fn call(&self, req: Received) -> Result<Call, Answer> {
        let thread = self.0.threads.read().unwrap().get(&req.local).cloned();
        if req.id == wire::REJECT_DELIVERY {
            let result = (|| {
                if !req.ports.is_empty() || req.data.len() != 16 {
                    return Err(wire::EPROTO);
                }
                let (file, tid) = thread.as_ref().ok_or(errno::EBADF)?;
                let mut reader = Reader::new(&req.data);
                self.0
                    .driver
                    .reject_delivery(file.handle, *tid, reader.u64()?, reader.u64()?)
            })();
            for port in &req.ports {
                mach::release_send(*port);
            }
            return Err(Answer::new(
                req.reply,
                IoctlReply {
                    status: result.err().unwrap_or(0),
                    ..Default::default()
                },
                Vec::new(),
                false,
            ));
        }

        // A thread port of a file being released is gone.
        let status = if thread.is_some() {
            wire::EPROTO
        } else {
            errno::EBADF
        };
        let io = Ioctl::decode(&req.data).ok().filter(|io| valid_exported_files(io, &req.ports));
        let (Some(io), Some((file, tid))) = (io, thread) else {
            for p in &req.ports {
                mach::release_send(*p);
            }
            let reply = IoctlReply {
                status,
                ..Default::default()
            };
            return Err(Answer::new(req.reply, reply, Vec::new(), false));
        };
        let mut ports = req.ports.iter().copied();
        let files = io.fds.iter().enumerate().map(|(index, fd)| {
            let backing = ports.next().unwrap();
            let regular = io.regular[index].clone().map(|metadata| {
                let writer = if metadata.writer { Some(ports.next().unwrap()) } else { None };
                RegularPorts { metadata, writer }
            });
            let file=Arc::new(FilePort(backing, io.file_classes[index], regular,io.sockets[index])) as File;
            (*fd,file)
        }).collect();
        let guest = Gathered {
            segments: io.segments,
            files,
            reserved: io.reserved,
            grow: io.grow,
            reply: IoctlReply::default(),
            installed: Vec::new(),
        };
        Ok(Call {
            driver: self.0.driver.clone(),
            file,
            tid,
            to: req.reply,
            cmd: io.cmd,
            arg: io.arg,
            guest,
        })
    }
}

thread_local! {
    /// On a worker thread: the answers of calls resumed on it, which it
    /// sends.
    static OUTBOX: std::cell::RefCell<Option<Vec<Answer>>> = const { std::cell::RefCell::new(None) };
}

/// One guest ioctl, from its request until its answer.
struct Call {
    driver: Arc<Driver>,
    file: Arc<OpenFile>,
    tid: Tid,
    /// The requester's send-once right.
    to: Port,
    cmd: u32,
    arg: Vec<u8>,
    guest: Gathered,
}

/// Where a parked call waits for its resume.
enum Slot {
    Running,
    /// Resumed before the call was stored: it retries at once.
    Woken,
    Parked(Box<Call>),
}

impl Call {
    /// Issue the ioctl: its answer, or `None` if its read parked, in which
    /// case the thread that resumes it sends the answer.
    fn run(mut self) -> Option<Answer> {
        loop {
            let slot = Arc::new(Mutex::new(Slot::Running));
            let waker = slot.clone();
            let resume = Box::new(move || {
                let parked = std::mem::replace(&mut *waker.lock().unwrap(), Slot::Woken);
                if let Slot::Parked(call) = parked
                    && let Some(answer) = call.run()
                {
                    answer.deliver();
                }
            });
            let handle = self.file.handle;
            let outcome = self.driver.ioctl_or_park(
                handle,
                self.tid,
                self.cmd,
                &mut self.arg,
                &mut self.guest,
                resume,
            );
            match outcome {
                Some(result) => return Some(self.finish(result)),
                None => {
                    let mut slot = slot.lock().unwrap();
                    if !matches!(*slot, Slot::Woken) {
                        *slot = Slot::Parked(Box::new(self));
                        return None;
                    }
                }
            }
        }
    }

    fn finish(self, result: Result<(), Errno>) -> Answer {
        let mut reply = self.guest.reply;
        reply.status = result.err().unwrap_or(0);
        reply.arg = self.arg;
        reply.drain = self.file.refresh(&self.driver);
        let exit = reply.status == 0 && self.cmd == aim_binder_driver::uapi::BINDER_THREAD_EXIT;
        Answer::new(self.to, reply, self.guest.installed, exit)
    }
}

/// An ioctl's answer, and the files it installs, kept until it is sent.
struct Answer {
    to: Port,
    msg: Msg,
    files: Vec<File>,
    /// The thread left the driver (`BINDER_THREAD_EXIT`).
    exit: bool,
}

impl Answer {
    fn new(to: Port, reply: IoctlReply, files: Vec<File>, exit: bool) -> Self {
        let ports = files
            .iter()
            .flat_map(|f| {
                let file = f.downcast_ref::<FilePort>().unwrap();
                std::iter::once((file.0, mach::COPY_SEND)).chain(file.2.as_ref().and_then(|regular| regular.writer).map(|port| (port, mach::COPY_SEND)))
            })
            .collect();
        let msg = Msg {
            id: wire::REPLY,
            ports,
            data: reply.encode(),
        };
        Self {
            to,
            msg,
            files,
            exit,
        }
    }

    /// Send it, or leave it to the worker this runs on.
    fn deliver(self) {
        let this = OUTBOX.with(|o| match o.borrow_mut().as_mut() {
            Some(outbox) => {
                outbox.push(self);
                None
            }
            None => Some(self),
        });
        if let Some(answer) = this {
            answer.send();
        }
    }

    fn send(self) {
        thread_local! {
            static BUF: std::cell::RefCell<Buffer> = std::cell::RefCell::new(Buffer::default());
        }
        BUF.with(|b| mach::reply(&mut b.borrow_mut(), self.to, &self.msg));
    }
}

/// The calling guest process as the driver sees it during one ioctl: the
/// memory the syscall layer gathered, and the fds it passed.
struct Gathered {
    segments: Vec<(u64, Vec<u8>)>,
    /// The files the caller passed, by its fd (a few at most).
    files: Vec<(u32, File)>,
    reserved: Vec<u32>,
    grow: bool,
    reply: IoctlReply,
    installed: Vec<File>,
}

impl GuestProcess for Gathered {
    fn copy_from_user(&mut self, address: u64, out: &mut [u8]) -> Result<(), Errno> {
        // Copying nothing succeeds, as copy_from_user does: a
        // BINDER_WRITE_READ restarted after a signal has all of its write
        // buffer consumed already, and nothing of it was gathered.
        if out.is_empty() {
            return Ok(());
        }
        let end = address.checked_add(out.len() as u64).ok_or(errno::EFAULT)?;
        for (base, bytes) in &self.segments {
            if address >= *base && end <= base + bytes.len() as u64 {
                let at = (address - base) as usize;
                out.copy_from_slice(&bytes[at..at + out.len()]);
                return Ok(());
            }
        }
        Err(errno::EFAULT)
    }

    fn copy_to_user(&mut self, address: u64, data: &[u8]) -> Result<(), Errno> {
        self.reply.writes.push((address, data.to_vec()));
        Ok(())
    }

    fn get_file(&mut self, fd: u32) -> Result<File, Errno> {
        self.files
            .iter()
            .find(|(f, _)| *f == fd)
            .map(|(_, file)| file.clone())
            .ok_or(errno::EBADF)
    }

    fn install_file(&mut self, file: File) -> Result<u32, Errno> {
        if !file.is::<FilePort>() || self.reserved.is_empty() {
            // EMFILE: the reader has no fd for it.
            return Err(24);
        }
        let next = file.downcast_ref::<FilePort>().unwrap();
        if (next.1 == crate::regular_file::CLASS) != next.2.is_some() || (next.1==crate::socket_scm::CLASS)!=next.3.is_some(){return Err(wire::EPROTO);}
        let ports: usize = self.installed.iter().map(|file| 1 + file.downcast_ref::<FilePort>().unwrap().2.as_ref().is_some_and(|regular| regular.writer.is_some()) as usize).sum();
        let next = file.downcast_ref::<FilePort>().unwrap();
        if ports + 1 + next.2.as_ref().is_some_and(|regular| regular.writer.is_some()) as usize > mach::MAX_PORTS { return Err(24); }
        let fd = self.reserved.remove(0);
        self.reply.installs.push(fd);
        self.reply
            .file_classes
            .push(file.downcast_ref::<FilePort>().unwrap().1);
        self.reply.regular.push(file.downcast_ref::<FilePort>().unwrap().2.as_ref().map(|regular| regular.metadata.clone()));
        self.reply.sockets.push(file.downcast_ref::<FilePort>().unwrap().3);
        self.installed.push(file);
        Ok(fd)
    }

    fn delivered(&mut self, buffer: u64, transaction: u64) {
        self.reply.deliveries.push((buffer, transaction));
    }

    fn can_install(&mut self, count: usize) -> bool {
        // The installed files ride back as the reply's ports.
        if count <= self.reserved.len() || !self.grow || count > mach::MAX_PORTS {
            return true;
        }
        self.reply.want_fds = count as u32;
        false
    }

    fn close_fd(&mut self, fd: u32) {
        self.reply.closes.push(fd);
    }
}

#[cfg(test)]
mod path_carrier_tests {
    use super::*;
    use std::{
        ffi::CString,
        os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    };
    struct FiniteScmService {port:Port,thread:Option<std::thread::JoinHandle<()>>}
    impl FiniteScmService {
        fn join(&mut self){self.thread.take().unwrap().join().unwrap();}
    }
    impl Drop for FiniteScmService {
        fn drop(&mut self){
            mach::destroy_receive(self.port);mach::release_send(self.port);
            if let Some(thread)=self.thread.take(){if thread.join().is_err(){eprintln!("SCM fixture service unwound");}}
        }
    }
    #[test]
    fn binder_socket_authority_retains_actual_backing_and_rejects_foreign_receipts(){
        const MARKER:&str="BINDER_SOCKET_AUTHORITY_EXECUTED";
        if crate::socket_scm::tests::isolated_exec_fixture("server::path_carrier_tests::binder_socket_authority_retains_actual_backing_and_rejects_foreign_receipts",MARKER){return;}
        let service=crate::socket_scm::Service::start().unwrap();
        use std::os::unix::ffi::OsStrExt;
        let channel=std::os::unix::net::UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&service.endpoint().path))).unwrap();
        let queue=aim_storage::socket_queue_root::SocketQueueRoot::new(channel.as_fd()).unwrap();
        crate::socket_scm::authenticate(channel.as_raw_fd(),service.endpoint()).unwrap();
        let(backing,peer)=std::os::unix::net::UnixStream::pair().unwrap();let receipt=aim_storage::socket_inode::Receipt::mint(backing.as_raw_fd()).unwrap();
        crate::socket_scm::request_create(channel.as_raw_fd(),backing.as_raw_fd(),receipt).unwrap();crate::socket_scm::wait_reply(channel.as_raw_fd()).unwrap();
        let carrier=crate::socket_scm::receive_reply(channel.as_raw_fd()).unwrap().descriptor.unwrap();
        let port=mach::fd_to_port(backing.as_raw_fd()).unwrap();
        let mut io=Ioctl{fds:vec![8],file_classes:vec![crate::socket_scm::CLASS],regular:vec![None],sockets:vec![Some(receipt)],..Default::default()};
        assert!(valid_exported_files(&io,&[port]));
        io.sockets[0].as_mut().unwrap().generation[0]^=1;assert!(!valid_exported_files(&io,&[port]));
        io.sockets[0]=Some(receipt);let foreign=mach::fd_to_port(peer.as_raw_fd()).unwrap();assert!(!valid_exported_files(&io,&[foreign]));mach::release_send(foreign);
        io.sockets[0]=None;assert!(!valid_exported_files(&io,&[port]));io.sockets[0]=Some(receipt);io.file_classes[0]=0;assert!(!valid_exported_files(&io,&[port]));mach::release_send(port);
        let file=file_from_fd(carrier.as_fd()).unwrap();assert_eq!(file_class(&file),Some(crate::socket_scm::CLASS));
        let queued=file.clone();drop(file);drop(backing);drop(carrier);
        crate::socket_scm::request_drain(channel.as_raw_fd()).unwrap();crate::socket_scm::wait_reply(channel.as_raw_fd()).unwrap();drop(crate::socket_scm::receive_reply(channel.as_raw_fd()).unwrap());
        let received=file_fd(&queued).unwrap();receipt.validate(received.as_raw_fd()).unwrap();
        assert_eq!(queued.downcast_ref::<FilePort>().unwrap().3,Some(receipt));
        let mut duplicate=received.try_clone().unwrap();drop(queued);drop(received);
        use std::io::{Read,Write};let mut peer=peer;peer.write_all(b"queued payload").unwrap();let mut bytes=[0;14];duplicate.read_exact(&mut bytes).unwrap();assert_eq!(&bytes,b"queued payload");
        let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},-1);
        drop(duplicate);assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},0,"last B/fileport reference releases peer EOF");
        queue.prepare_last_close(channel.as_fd()).unwrap();drop(channel);drop(queue);drop(service);println!("{MARKER}");
    }
    #[test]
    fn actual_client_socket_scm_rpc_repeated_resolve_preserves_receipt_and_exact_eof(){
        use std::os::unix::ffi::OsStrExt;
        let(backing,peer)=std::os::unix::net::UnixStream::pair().unwrap();let receipt=aim_storage::socket_inode::Receipt::mint(backing.as_raw_fd()).unwrap();let socket_service=crate::socket_scm::Service::start().unwrap();
        let service=mach::new_port(true).unwrap();let descriptor=socket_service.endpoint().clone();let worker=std::thread::spawn(move||{
            let mut buffer=Buffer::default();let request=mach::receive(&mut buffer,service).unwrap();assert_eq!(request.id,wire::SOCKET_SCM_CHANNEL);assert!(request.ports.is_empty());
            let mut data=0i32.to_le_bytes().to_vec();data.extend(descriptor.encode());let message=Msg{id:wire::REPLY,ports:vec![],data};mach::reply_bounded(&mut buffer,request.reply,&message,1000).unwrap();
        });let mut worker=FiniteScmService{port:service,thread:Some(worker)};let client=crate::client::Client::from_service_port(service);
        let endpoint=client.socket_scm_endpoint().unwrap();let channel=std::os::unix::net::UnixStream::connect(std::path::Path::new(std::ffi::OsStr::from_bytes(&endpoint.path))).unwrap();let _channel_root=aim_storage::socket_queue_root::SocketQueueRoot::new(channel.as_fd()).unwrap();crate::socket_scm::authenticate(channel.as_raw_fd(),&endpoint).unwrap();worker.join();drop(worker);
        crate::socket_scm::request_create(channel.as_raw_fd(),backing.as_raw_fd(),receipt).unwrap();crate::socket_scm::wait_reply(channel.as_raw_fd()).unwrap();let reply=crate::socket_scm::receive_reply(channel.as_raw_fd()).unwrap();let carrier=reply.descriptor.unwrap();assert_eq!(reply.receipt,Some(receipt));drop(backing);
        for _ in 0..2{crate::socket_scm::request_resolve(channel.as_raw_fd(),carrier.as_raw_fd()).unwrap();crate::socket_scm::wait_reply(channel.as_raw_fd()).unwrap();let reply=crate::socket_scm::receive_reply(channel.as_raw_fd()).unwrap();assert_eq!(reply.receipt,Some(receipt));let imported=reply.descriptor.unwrap();receipt.validate(imported.as_raw_fd()).unwrap();drop(imported);}
        drop(carrier);crate::socket_scm::request_drain(channel.as_raw_fd()).unwrap();crate::socket_scm::wait_reply(channel.as_raw_fd()).unwrap();let drained=crate::socket_scm::receive_reply(channel.as_raw_fd()).unwrap();assert_eq!(drained.class,0);assert!(drained.descriptor.is_none());assert!(drained.receipt.is_none());_channel_root.prepare_last_close(channel.as_fd()).unwrap();drop(channel);drop(_channel_root);drop(socket_service);let mut byte=0u8;assert_eq!(unsafe{libc::recv(peer.as_raw_fd(),(&mut byte as*mut u8).cast(),1,libc::MSG_DONTWAIT)},0);
    }
    #[test]
    fn actual_client_regular_scm_control_rpc_preserves_identity_writer_and_drains_eof(){
        const MARKER:&str="REGULAR_SCM_RPC_LIFETIME_FIXTURE_EXECUTED";
        if crate::socket_scm::tests::isolated_exec_fixture("server::path_carrier_tests::actual_client_regular_scm_control_rpc_preserves_identity_writer_and_drains_eof",MARKER){return;}
        use std::fs::OpenOptions;
        let path=std::env::temp_dir().join(format!("aim-scm-control-{}",std::process::id()));std::fs::create_dir(&path).unwrap();
        let backing=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("source")).unwrap();
        let writer=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("writers")).unwrap();
        let contender=OpenOptions::new().read(true).write(true).open(path.join("writers")).unwrap();assert_eq!(unsafe{libc::flock(writer.as_raw_fd(),libc::LOCK_SH)},0);
        let metadata=wire::RegularMetadata{flags:2,uid:1000,gid:1001,identity:crate::regular_file::identity(backing.as_fd()).unwrap(),writer:true};
        let service=mach::new_port(true).unwrap();
        let owner=std::thread::spawn(move||{
            let mut buffer=Buffer::default();
            for _ in 0..3{
                let Ok(request)=mach::receive(&mut buffer,service)else{return;};
                if let Err(panic)=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||serve_regular_control(&request,&mut buffer))){
                    for port in &request.ports{mach::release_send(*port);}mach::release_send(request.reply);
                    std::panic::resume_unwind(panic);
                }
            }
        });
        let mut owner=FiniteScmService{port:service,thread:Some(owner)};
        let client=crate::client::Client::from_service_port(service);
        let port=client.create_regular_scm(backing.as_raw_fd(),Some(writer.as_raw_fd()),&metadata).unwrap();
        let carrier=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(port.as_port()).unwrap())};drop(port);drop(backing);drop(writer);
        let resolved=client.resolve_regular_scm(carrier.as_raw_fd()).unwrap();assert_eq!(resolved.metadata,metadata);
        let data=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(resolved.backing.as_port()).unwrap())};
        let lease=unsafe{OwnedFd::from_raw_fd(mach::port_to_fd(resolved.writer.as_ref().unwrap().as_port()).unwrap())};
        assert_eq!(crate::regular_file::identity(data.as_fd()).unwrap(),metadata.identity);
        drop(resolved);drop(carrier);drop(data);drop(lease);
        client.drain_regular_scm(&metadata.identity).unwrap();owner.join();
        assert_eq!(unsafe{libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
        drop(owner);std::fs::remove_dir_all(path).unwrap();println!("{MARKER}");
    }
    #[test]
    fn regular_sidecar_count_and_backing_incarnation_are_checked_before_driver_adoption() {
        use std::fs::OpenOptions;
        let path=std::env::temp_dir().join(format!("aim-binder-regular-validation-{}",std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let source=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("source")).unwrap();
        let writer=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("writers")).unwrap();
        let backing=mach::fd_to_port(source.as_raw_fd()).unwrap();let lock=mach::fd_to_port(writer.as_raw_fd()).unwrap();
        let metadata=wire::RegularMetadata {flags:2,uid:1000,gid:1001,identity:crate::regular_file::identity(source.as_fd()).unwrap(),writer:true};
        let mut io=Ioctl {fds:vec![9],file_classes:vec![crate::regular_file::CLASS],regular:vec![Some(metadata)],sockets:vec![None],..Default::default()};
        assert!(valid_exported_files(&io,&[backing,lock]));
        assert!(!valid_exported_files(&io,&[backing]));
        assert!(!valid_exported_files(&io,&[backing,lock,lock]));
        io.regular[0].as_mut().unwrap().identity[8]^=1;
        assert!(!valid_exported_files(&io,&[backing,lock]));
        mach::release_send(backing);mach::release_send(lock);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn regular_writer_sidecar_survives_real_binder_reply_buffer_and_sender_close() {
        use crate::{local::{LocalProcess,Service,Call,Reply},parcel::{Binder,Parcel}};
        use std::{fs::OpenOptions,sync::Weak};
        struct Echo(Weak<LocalProcess>);
        impl Service for Echo {
            fn descriptor(&self)->&str { "test.RegularLease" }
            fn accepts_fds(&self)->bool { true }
            fn transact(&self, call:&mut Call<'_>)->Reply {
                let fd=call.data.read_fd()?;
                let mut reply=Parcel::new();
                reply.write_file(self.0.upgrade().unwrap().file(fd).unwrap());
                Ok(reply)
            }
        }
        let path=std::env::temp_dir().join(format!("aim-binder-regular-roundtrip-{}",std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let source=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("source")).unwrap();
        let writer=OpenOptions::new().read(true).write(true).create_new(true).open(path.join("writers")).unwrap();
        let contender=OpenOptions::new().read(true).write(true).open(path.join("writers")).unwrap();
        assert_eq!(unsafe {libc::flock(writer.as_raw_fd(),libc::LOCK_SH)},0);
        let metadata=wire::RegularMetadata {flags:2,uid:1000,gid:1001,identity:crate::regular_file::identity(source.as_fd()).unwrap(),writer:true};
        let file=regular_file_from_fd(source.as_fd(),Some(writer.as_fd()),metadata.clone()).unwrap();
        let driver=Driver::new();
        let open=|pid| LocalProcess::open(&driver,Device::Binder,Credentials {pid,euid:1000,security_context:None});
        let owner=open(701);let caller=open(702);
        let Binder::Local(ptr)=owner.add_service(Arc::new(Echo(Arc::downgrade(&owner)))) else {unreachable!()};
        let mut object=aim_binder_driver::uapi::FlatBinderObject {kind:aim_binder_driver::uapi::BINDER_TYPE_BINDER,flags:aim_binder_driver::uapi::FLAT_BINDER_FLAG_ACCEPTS_FDS,binder:ptr,cookie:ptr}.encode();
        let mut control=Gathered {segments:vec![],files:vec![],reserved:vec![],grow:false,reply:Default::default(),installed:vec![]};
        driver.ioctl(owner.proc_handle(),701,aim_binder_driver::uapi::BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut control).unwrap();
        owner.start();
        let mut request=Parcel::new();request.write_file(file);
        drop(source);drop(writer);
        let reply=caller.transact(0,1,&request,false).unwrap();
        drop(request);
        let fd=reply.reader().read_fd().unwrap();
        let file=caller.file(fd).unwrap();
        let stored=file.downcast_ref::<FilePort>().unwrap();
        assert_eq!(stored.2.as_ref().unwrap().metadata,metadata);
        let mut received=file_fd(&file).unwrap();
        assert!(received.try_clone().unwrap().into_owned_fd().is_err(),"typed regular owners cannot be extracted as bare fds");
        drop(file);drop(reply);
        assert!(caller.file(fd).is_none(),"reply buffer closes its single public FD");
        assert_eq!(unsafe {libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},-1);
        use std::io::{Read,Write,Seek,SeekFrom};
        received.write_all(b"actual backing").unwrap();
        received.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes=Vec::new();received.read_to_end(&mut bytes).unwrap();assert_eq!(bytes,b"actual backing");
        drop(received);
        assert_eq!(unsafe {libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0,"last writer descriptor closes synchronously");
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn regular_fileport_keeps_exact_writer_lock_until_final_reference_closes() {
        const MARKER:&str="REGULAR_FILEPORT_LIFETIME_FIXTURE_EXECUTED";
        if crate::socket_scm::tests::isolated_exec_fixture("server::path_carrier_tests::regular_fileport_keeps_exact_writer_lock_until_final_reference_closes",MARKER){return;}
        use std::fs::OpenOptions;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("aim-binder-regular-{}-{}",std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        let source = OpenOptions::new().read(true).write(true).create_new(true).open(path.join("source")).unwrap();
        let writer = OpenOptions::new().read(true).write(true).create_new(true).open(path.join("writers")).unwrap();
        let contender = OpenOptions::new().read(true).write(true).open(path.join("writers")).unwrap();
        assert_eq!(unsafe { libc::flock(writer.as_raw_fd(),libc::LOCK_SH) },0);
        let metadata = wire::RegularMetadata { flags:2, uid:1000, gid:1001, identity:crate::regular_file::identity(source.as_fd()).unwrap(), writer:true };
        let file = regular_file_from_fd(source.as_fd(),Some(writer.as_fd()),metadata.clone()).unwrap();
        drop(source); drop(writer);
        let queued = file.clone(); drop(file);
        assert_eq!(unsafe { libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB) },-1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::EWOULDBLOCK));
        let stored = queued.downcast_ref::<FilePort>().unwrap();
        assert_eq!(stored.2.as_ref().unwrap().metadata,metadata);
        let received = unsafe { OwnedFd::from_raw_fd(mach::port_to_fd(stored.2.as_ref().unwrap().writer.unwrap()).unwrap()) };
        let flags=unsafe{libc::fcntl(received.as_raw_fd(),libc::F_GETFD)};assert!(flags>=0);assert_ne!(flags&libc::FD_CLOEXEC,0);
        drop(queued);
        assert_eq!(unsafe { libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB) },-1);
        drop(received);
        assert_eq!(unsafe { libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB) },0,"last real fileport/fd close must release the writer immediately");
        std::fs::remove_dir_all(path).unwrap();
        println!("{MARKER}");
    }
    #[test]
    fn path_fileport_creation_preserves_kernel_descriptor_and_linux_error_contract() {
        assert_eq!(
            path_creation_errno(std::io::Error::from_raw_os_error(libc::EPROTO)),
            71
        );
        assert_eq!(
            path_creation_errno(std::io::Error::from_raw_os_error(libc::EAGAIN)),
            11
        );
        let name = CString::new(std::env::temp_dir().as_os_str().as_encoded_bytes()).unwrap();
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
        assert!(fd >= 0);
        let backing = unsafe { OwnedFd::from_raw_fd(fd) };
        let port = mach::fd_to_port(backing.as_raw_fd()).unwrap();
        assert_eq!(
            create_path_fileport(port, crate::path_file::O_PATH | 0x10000).unwrap_err(),
            errno::EINVAL
        );
        let flags = crate::path_file::O_PATH | 0x4000 | 0x8000;
        let (ports, metadata) = create_path_fileport(port, flags).unwrap();
        mach::release_send(port);
        assert_eq!(ports.len(), 1);
        assert_eq!(metadata, crate::path_file::CLASS.to_le_bytes());
        let carrier = unsafe { OwnedFd::from_raw_fd(mach::port_to_fd(ports[0].0).unwrap()) };
        mach::release_send(ports[0].0);
        assert_eq!(
            registered_descriptor_class(carrier.as_raw_fd()).unwrap(),
            crate::path_file::CLASS
        );
        let imported = crate::path_file::unwrap(carrier.as_fd()).unwrap();
        assert_eq!(imported.flags, flags);
        let mut before = std::mem::MaybeUninit::<libc::stat>::uninit();
        let mut after = std::mem::MaybeUninit::<libc::stat>::uninit();
        assert_eq!(
            unsafe { libc::fstat(backing.as_raw_fd(), before.as_mut_ptr()) },
            0
        );
        assert_eq!(
            unsafe { libc::fstat(imported.backing.as_raw_fd(), after.as_mut_ptr()) },
            0
        );
        let before = unsafe { before.assume_init() };
        let after = unsafe { after.assume_init() };
        assert_eq!(
            (before.st_dev, before.st_ino, before.st_mode),
            (after.st_dev, after.st_ino, after.st_mode)
        );
        let file = file_from_fd(carrier.as_fd()).unwrap();
        assert_eq!(file_class(&file), Some(crate::path_file::CLASS));
    }
}

#[cfg(test)]
mod ioctl_only_tests {
    use super::*;
    use std::{io::{Read,Write},os::fd::AsFd};
    #[test]
    fn ioctl_only_regular_transport_preserves_metadata_and_denies_data_io(){
        let path=std::env::temp_dir().join(format!("aim-ioctl-only-{}",std::process::id()));
        let backing=std::fs::File::options().read(true).write(true).create_new(true).open(&path).unwrap();
        let metadata=wire::RegularMetadata{flags:3,uid:1000,gid:1001,identity:crate::regular_file::identity(backing.as_fd()).unwrap(),writer:false};
        let message=wire::Ioctl{fds:vec![7],file_classes:vec![crate::regular_file::CLASS],regular:vec![Some(metadata.clone())],sockets:vec![None],..Default::default()};
        assert!(message.validate().is_ok());assert_eq!(wire::Ioctl::decode(&message.encode()).unwrap(),message);
        let owner=regular_file_from_fd(backing.as_fd(),None,metadata).unwrap();let mut file=file_fd(&owner).unwrap();
        assert_eq!(file.read(&mut[0]).unwrap_err().raw_os_error(),Some(libc::EBADF));
        assert_eq!(file.write(b"x").unwrap_err().raw_os_error(),Some(libc::EBADF));
        assert_eq!(file.set_len(1).unwrap_err().raw_os_error(),Some(libc::EBADF));
        assert_eq!(file.metadata().unwrap().len(),0);
        drop(file);drop(owner);drop(backing);std::fs::remove_file(path).unwrap();
    }
}
