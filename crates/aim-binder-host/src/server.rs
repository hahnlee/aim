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
struct FilePort(Port, u32);

impl Drop for FilePort {
    fn drop(&mut self) {
        mach::release_send(self.0);
    }
}

/// Retain a host file descriptor as a Binder file without taking the caller's
/// fd. The returned fileport keeps the file alive while it crosses Binder.
pub fn file_from_fd(fd: std::os::fd::BorrowedFd<'_>) -> Option<File> {
    use std::os::fd::AsRawFd;
    mach::fd_to_port(fd.as_raw_fd()).map(|port| Arc::new(FilePort(port, 0)) as File)
}

/// Export a native proxy capability with an explicit versioned Binder class.
pub fn proxy_file_from_fd(fd: std::os::fd::BorrowedFd<'_>) -> Option<File> {
    use std::os::fd::AsRawFd;
    if crate::proxy_file::registered_class(fd.as_raw_fd()) != crate::proxy_file::CLASS {
        return None;
    }
    mach::fd_to_port(fd.as_raw_fd())
        .map(|port| Arc::new(FilePort(port, crate::proxy_file::CLASS)) as File)
}

pub fn file_class(file: &File) -> Option<u32> {
    file.downcast_ref::<FilePort>().map(|file| file.1)
}

/// A new host fd for a file a guest sent (a fileport), which a binder
/// process on the host received.
pub fn file_fd(file: &File) -> Option<std::os::fd::OwnedFd> {
    let port = file.downcast_ref::<FilePort>()?.0;
    // SAFETY: a new fd this process owns.
    mach::port_to_fd(port).map(|fd| unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) })
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
            let result = if req.local == self.service && req.id == wire::FILE_CLASS {
                if req.ports.len() != 1 || !req.data.is_empty() {
                    Err(wire::EPROTO)
                } else if let Some(fd) = mach::port_to_fd(req.ports[0]) {
                    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
                    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                    let mut out = Writer::default();
                    match crate::proxy_file::registered_class_result(fd.as_raw_fd()) {
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
        // The previous answer goes out in the same mach_msg that waits for
        // the next request.
        let mut answer: Option<Answer> = None;
        loop {
            let next = match answer.take() {
                Some(a) => {
                    let r = mach::reply_and_receive(&mut buf, a.to, &a.msg, self.0.set);
                    drop(a.files);
                    r
                }
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
        // A thread port of a file being released is gone.
        let status = if thread.is_some() {
            wire::EPROTO
        } else {
            errno::EBADF
        };
        let io = Ioctl::decode(&req.data)
            .ok()
            .filter(|io| io.fds.len() == req.ports.len())
            .filter(|io| {
                req.ports.iter().zip(&io.file_classes).all(|(port, class)| {
                    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
                    mach::port_to_fd(*port).is_some_and(|fd| {
                        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                        crate::proxy_file::registered_class_result(fd.as_raw_fd())
                            .is_ok_and(|actual| actual == *class)
                    })
                })
            });
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
        let guest = Gathered {
            segments: io.segments,
            files: io
                .fds
                .iter()
                .zip(&req.ports)
                .enumerate()
                .map(|(index, (fd, p))| {
                    (*fd, Arc::new(FilePort(*p, io.file_classes[index])) as File)
                })
                .collect(),
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
            .map(|f| (f.downcast_ref::<FilePort>().unwrap().0, mach::COPY_SEND))
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
        let fd = self.reserved.remove(0);
        self.reply.installs.push(fd);
        self.reply
            .file_classes
            .push(file.downcast_ref::<FilePort>().unwrap().1);
        self.installed.push(file);
        Ok(fd)
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
