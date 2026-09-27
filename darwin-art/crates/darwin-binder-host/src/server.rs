//! The daemon side: one [`Driver`] for every guest process of a profile.
//!
//! - The **service port** (a bootstrap name) takes `OPEN`. Each open file
//!   gets a **file port** (`THREAD`, `MMAP`, `POLL`) and a readiness socket
//!   whose other end is the guest's binder fd.
//! - Each guest thread that issues binder ioctls gets a **thread port** and a
//!   daemon thread that serves it. The ioctl runs on that thread and may
//!   block in the driver, as the guest thread blocks in the kernel.
//! - **Release** is the last close of the guest's binder fd, which includes
//!   process death: the daemon's end of the readiness socket reads EOF.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use darwin_binder_driver::{
    Credentials, Device, Driver, Errno, File, GuestProcess, ProcHandle, ReceiveMemory, Tid, errno,
};

use crate::mach::{self, Buffer, Msg, Port, Received};
use crate::wire::{self, Ioctl, IoctlReply, Reader, Writer};

/// A fileport as the driver's opaque `File`.
struct FilePort(Port);

impl Drop for FilePort {
    fn drop(&mut self) {
        mach::release_send(self.0);
    }
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
    /// Stop serving a thread port, unless release already destroyed it.
    fn forget_thread(&self, port: Port) {
        let mut threads = self.threads.lock().unwrap();
        if let Some(i) = threads.iter().position(|p| *p == port) {
            threads.swap_remove(i);
            mach::destroy_receive(port);
        }
    }

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
}

impl Server {
    /// Check in `name` and start serving on background threads.
    pub fn start(name: &str) -> Result<Arc<Self>, String> {
        let service =
            mach::check_in(name).map_err(|kr| format!("bootstrap_check_in {name}: {kr:#x}"))?;
        let set = mach::new_port_set().map_err(|kr| format!("port set: {kr:#x}"))?;
        mach::move_member(service, set).map_err(|kr| format!("port set: {kr:#x}"))?;
        // SAFETY: plain kqueue.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err("kqueue failed".into());
        }
        let server = Arc::new(Self {
            driver: Driver::new(),
            service,
            set,
            files: Mutex::new(HashMap::new()),
            kq,
        });
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
            let result = if req.local == self.service {
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
                let worker = Worker {
                    driver: self.driver.clone(),
                    file: file.clone(),
                    tid,
                    port,
                };
                if std::thread::Builder::new()
                    .name(format!("binder-{tid}"))
                    .stack_size(256 << 10)
                    .spawn(move || worker.run())
                    .is_err()
                {
                    file.forget_thread(port);
                    return Err(errno::ENOMEM);
                }
                Ok((vec![(port, mach::MAKE_SEND)], Vec::new()))
            }
            wire::MMAP => {
                let (vm_start, len) = (r.u64()?, r.u64()?);
                let len = len.min(darwin_binder_driver::MAX_MAPPING as u64);
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
            for t in std::mem::take(&mut *file.threads.lock().unwrap()) {
                mach::destroy_receive(t);
            }
            // SAFETY: closing our end; kqueue drops the registration.
            unsafe { libc::close(fd) };
        }
    }
}

/// Serves one guest thread's ioctls.
struct Worker {
    driver: Arc<Driver>,
    file: Arc<OpenFile>,
    tid: Tid,
    port: Port,
}

impl Worker {
    fn run(self) {
        let mut buf = Buffer::default();
        // The previous answer goes out in the same mach_msg that waits for
        // the next request.
        let mut answer: Option<(Port, Msg, Vec<File>)> = None;
        loop {
            let next = match answer.take() {
                Some((to, msg, files)) => {
                    let r = mach::reply_and_receive(&mut buf, to, &msg, self.port);
                    drop(files);
                    r
                }
                None => mach::receive(&mut buf, self.port),
            };
            let Ok(req) = next else { return };
            let (reply, ports, files, cmd) = self.ioctl(&req);
            let exit = reply.status == 0 && cmd == darwin_binder_driver::uapi::BINDER_THREAD_EXIT;
            let msg = Msg {
                id: wire::REPLY,
                ports,
                data: reply.encode(),
            };
            if exit {
                mach::reply(&mut buf, req.reply, &msg);
                self.file.forget_thread(self.port);
                return;
            }
            answer = Some((req.reply, msg, files));
        }
    }

    fn ioctl(&self, req: &Received) -> (IoctlReply, Vec<(Port, u32)>, Vec<File>, u32) {
        let io = match Ioctl::decode(&req.data) {
            Ok(io) if io.fds.len() == req.ports.len() => io,
            _ => {
                for p in &req.ports {
                    mach::release_send(*p);
                }
                return (
                    IoctlReply {
                        status: wire::EPROTO,
                        ..Default::default()
                    },
                    Vec::new(),
                    Vec::new(),
                    0,
                );
            }
        };
        let cmd = io.cmd;
        let mut guest = Gathered {
            segments: io.segments,
            files: io
                .fds
                .iter()
                .zip(&req.ports)
                .map(|(fd, p)| (*fd, Arc::new(FilePort(*p)) as File))
                .collect(),
            reserved: io.reserved,
            reply: IoctlReply::default(),
            installed: Vec::new(),
        };
        let mut arg = io.arg;
        let result = self
            .driver
            .ioctl(self.file.handle, self.tid, io.cmd, &mut arg, &mut guest);
        let mut reply = guest.reply;
        reply.status = result.err().unwrap_or(0);
        reply.arg = arg;
        reply.drain = self.file.refresh(&self.driver);
        let ports = guest
            .installed
            .iter()
            .map(|f| (f.downcast_ref::<FilePort>().unwrap().0, mach::COPY_SEND))
            .collect();
        (reply, ports, guest.installed, cmd)
    }
}

/// The calling guest process as the driver sees it during one ioctl: the
/// memory the syscall layer gathered, and the fds it passed.
struct Gathered {
    segments: Vec<(u64, Vec<u8>)>,
    files: HashMap<u32, File>,
    reserved: Vec<u32>,
    reply: IoctlReply,
    installed: Vec<File>,
}

impl GuestProcess for Gathered {
    fn copy_from_user(&mut self, address: u64, out: &mut [u8]) -> Result<(), Errno> {
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
        self.files.get(&fd).cloned().ok_or(errno::EBADF)
    }

    fn install_file(&mut self, file: File) -> Result<u32, Errno> {
        if !file.is::<FilePort>() || self.reserved.is_empty() {
            // EMFILE: the reader has no fd for it.
            return Err(24);
        }
        let fd = self.reserved.remove(0);
        self.reply.installs.push(fd);
        self.installed.push(file);
        Ok(fd)
    }

    fn close_fd(&mut self, fd: u32) {
        self.reply.closes.push(fd);
    }
}
