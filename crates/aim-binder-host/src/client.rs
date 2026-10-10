//! The guest side: what the syscall layer does for `open`, `ioctl`, `mmap`
//! and poll registration on a binder fd.
//!
//! The guest's binder fd is one end of a Unix socket pair; the daemon holds
//! the other end. It is readable while the daemon reports work (epoll and
//! poll work on it unchanged), and its last close is the file's release.
//! Guest fds are host fds.

use std::cell::RefCell;
use std::collections::HashMap;

use aim_binder_driver::uapi::*;
use aim_binder_driver::{Device, Errno, MAX_MAPPING, errno};

use crate::mach::{self, Buffer, Msg, Port};
use crate::wire::{self, Ioctl, IoctlReply, Reader, SharedFile, Writer};

/// Guest memory, as the calling process sees it.
pub trait UserMemory {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, Errno>;
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), Errno>;
    /// A received file now sits at `fd` (for the caller's fd bookkeeping).
    fn installed(&mut self, _fd: i32) {}
    /// Borrow the incoming send right while installing the actual backing.
    /// Guest adapters allocate temporary descriptors under their private-FD guard.
    fn install_fileport(&mut self, fd: i32, port: Port) -> Result<(), Errno> { install(fd, port) }
    fn installed_typed(&mut self, fd: i32, _class: u32) {
        self.installed(fd);
    }
    fn install_typed(&mut self, fd: i32, class: u32) -> Result<(), Errno> {
        self.installed_typed(fd, class);
        Ok(())
    }
    fn export_fd(&mut self, fd: i32) -> Result<i32, Errno> {
        Ok(fd)
    }
    fn regular_export(&mut self, _fd: i32) -> Result<Option<crate::regular_file::Export>, Errno> { Ok(None) }
    /// The receiving ABI must validate/adopt the actual writer open description.
    fn install_regular(&mut self, _fd: i32, _metadata: &wire::RegularMetadata, _writer: Option<crate::regular_file::WriterPort>) -> Result<(), Errno> { Err(95) }
    fn socket_export(&mut self,_fd:i32)->Result<Option<aim_storage::socket_inode::Receipt>,Errno>{Ok(None)}
    fn install_socket(&mut self,_fd:i32,_receipt:aim_storage::socket_inode::Receipt)->Result<(),Errno>{Err(95)}
    fn file_class(&mut self, _fd: i32) -> u32 {
        0
    }
    /// Close this authenticated installed/reserved receipt. Guest adapters
    /// withdraw visibility and retain its owner through the actual close.
    fn close_file(&mut self, fd:i32) -> Result<(),Errno> {
        self.closed(fd);
        if unsafe { libc::close(fd) } < 0 { Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(EIO)) } else { Ok(()) }
    }
    /// The driver closed `fd`.
    fn closed(&mut self, _fd: i32) {}
}

/// Linux EIO: the daemon is gone.
const EIO: Errno = 5;

/// Placeholder fds kept per thread for files a read may deliver.
const RESERVED_FDS: usize = 8;
/// Placeholders are dup'ed at or above this number, away from the low fds
/// programs reuse.
const RESERVED_FD_BASE: i32 = 64;

struct ThreadState {
    buf: Buffer,
    reply: Port,
    /// Thread port by file port.
    threads: HashMap<Port, Port>,
    reserved: Vec<i32>,
}

thread_local! {
    static THREAD: RefCell<Option<ThreadState>> = const { RefCell::new(None) };
}

fn with_thread<R>(f: impl FnOnce(&mut ThreadState) -> R) -> Result<R, Errno> {
    THREAD.with(|t| {
        let mut t = t.borrow_mut();
        if t.is_none() {
            let reply = mach::new_port(false).map_err(|_| errno::ENOMEM)?;
            *t = Some(ThreadState {
                buf: Buffer::default(),
                reply,
                threads: HashMap::new(),
                reserved: Vec::new(),
            });
        }
        Ok(f(t.as_mut().unwrap()))
    })
}

/// The calling thread after its process replaced its program in place
/// (execve): its placeholder fds went with the old image (close-on-exec),
/// and its thread ports serve files the process may no longer have.
pub fn forget_thread() {
    THREAD.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.reserved.clear();
            for (_, port) in t.threads.drain() {
                mach::release_send(port);
            }
        }
    });
}

fn call(t: &mut ThreadState, dest: Port, msg: &Msg) -> Result<mach::Received, Errno> {
    match mach::call(&mut t.buf, dest, t.reply, msg) {
        Ok(r) if r.id == wire::REPLY => Ok(r),
        // The daemon is gone (a send-once notification, or a dead port).
        _ => Err(EIO),
    }
}

/// A status word first; Ok with the rest.
fn status(r: &mach::Received) -> Result<Reader<'_>, Errno> {
    let mut rd = Reader::new(&r.data);
    match rd.i32()? {
        0 => Ok(rd),
        e => Err(e),
    }
}

/// The daemon's service, looked up by its bootstrap name.
pub struct Client {
    service: Port,
}

impl Client {
    #[cfg(test)]
    pub(crate) fn from_service_port(service:Port)->Self{Self{service}}
    pub fn socket_scm_endpoint(&self)->Result<crate::socket_scm::Endpoint,Errno>{
        let request=Msg{id:wire::SOCKET_SCM_CHANNEL,ports:vec![],data:vec![]};let reply=with_thread(|thread|call(thread,self.service,&request))??;
        let result=(||{let mut reader=status(&reply)?;let endpoint=crate::socket_scm::Endpoint::decode(&mut reader)?;if reader.remaining()!=0||!reply.ports.is_empty(){return Err(wire::EPROTO);}Ok(endpoint)})();
        for port in reply.ports{mach::release_send(port);}result
    }
    /// Return an opaque carrier fileport. The ABI materializes it privately.
    pub fn create_regular_scm(&self,backing:i32,writer:Option<i32>,metadata:&wire::RegularMetadata)->Result<crate::regular_scm::FilePort,Errno>{
        if metadata.writer!=writer.is_some(){return Err(wire::EPROTO);}
        let backing=crate::regular_scm::FilePort::new(mach::fd_to_port(backing).ok_or(errno::EBADF)?);
        let writer=writer.map(|fd|mach::fd_to_port(fd).map(crate::regular_scm::FilePort::new).ok_or(errno::EBADF)).transpose()?;
        let mut data=Writer::default();metadata.encode(&mut data);
        let mut ports=vec![(backing.as_port(),mach::COPY_SEND)];if let Some(writer)=&writer{ports.push((writer.as_port(),mach::COPY_SEND));}
        let request=Msg{id:wire::CREATE_REGULAR_SCM,ports,data:data.0};
        let reply=with_thread(|thread|call(thread,self.service,&request))??;
        let result=(||{let mut reader=status(&reply)?;if reader.u32()?!=crate::regular_scm::CLASS||reader.remaining()!=0||reply.ports.len()!=1{return Err(wire::EPROTO);}Ok(())})();
        if let Err(error)=result{for port in reply.ports{mach::release_send(port);}return Err(error);}
        Ok(crate::regular_scm::FilePort::new(reply.ports[0]))
    }
    pub fn resolve_regular_scm(&self,carrier:i32)->Result<crate::regular_scm::Resolved,Errno>{
        let carrier=crate::regular_scm::FilePort::new(mach::fd_to_port(carrier).ok_or(errno::EBADF)?);
        let request=Msg{id:wire::RESOLVE_REGULAR_SCM,ports:vec![(carrier.as_port(),mach::COPY_SEND)],data:vec![]};
        let reply=with_thread(|thread|call(thread,self.service,&request))??;
        let result=(||{let mut reader=status(&reply)?;let metadata=wire::RegularMetadata::decode(&mut reader)?;if reader.remaining()!=0||reply.ports.len()!=1+metadata.writer as usize{return Err(wire::EPROTO);}Ok(metadata)})();
        let metadata=match result{Ok(metadata)=>metadata,Err(error)=>{for port in reply.ports{mach::release_send(port);}return Err(error);}};
        Ok(crate::regular_scm::Resolved{backing:crate::regular_scm::FilePort::new(reply.ports[0]),writer:if metadata.writer{Some(crate::regular_file::WriterPort::new(reply.ports[1]))}else{None},metadata})
    }
    pub fn drain_regular_scm(&self,identity:&[u8;36])->Result<(),Errno>{
        let request=Msg{id:wire::DRAIN_REGULAR_SCM,ports:vec![],data:identity.to_vec()};
        let reply=with_thread(|thread|call(thread,self.service,&request))??;
        let result=(||{if !reply.ports.is_empty()||status(&reply)?.remaining()!=0{return Err(wire::EPROTO);}Ok(())})();
        for port in reply.ports{mach::release_send(port);}result
    }

    /// Register an owned Linux path carrier with the native daemon. Errors are Linux wire errno.
    pub fn create_path(&self, fd: i32, flags: u32) -> Result<std::os::fd::OwnedFd, Errno> {
        use std::os::fd::FromRawFd;
        let port = mach::fd_to_port(fd).ok_or(errno::EBADF)?;
        let request = Msg {
            id: wire::CREATE_PATH,
            ports: vec![(port, mach::COPY_SEND)],
            data: flags.to_le_bytes().to_vec(),
        };
        let result = with_thread(|t| call(t, self.service, &request));
        mach::release_send(port);
        let reply = result??;
        let result = (|| {
            let class = status(&reply)?.u32()?;
            if class != crate::path_file::CLASS || reply.ports.len() != 1 {
                return Err(wire::EPROTO);
            }
            let fd = mach::port_to_fd(reply.ports[0]).ok_or(errno::EBADF)?;
            Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
        })();
        for port in &reply.ports {
            mach::release_send(*port);
        }
        result
    }

    /// Ask the native owner's registry about an actual descriptor capability.
    pub fn file_class(&self, fd: i32) -> Result<u32, Errno> {
        let mut ty = 0i32;
        let mut len = std::mem::size_of::<i32>() as u32;
        // Opaque PTY carriers use real pipe identities; endpoint owners
        // classify them without treating other native files as capabilities.
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut ty as *mut i32).cast(),
                &mut len,
            )
        } < 0
        {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOTSOCK) {
                match aim_storage::pipe_identity::key(fd) {
                    Ok(Some(_)) => self.pipe_file_class(fd),
                    Ok(None) => Ok(0),
                    Err(_) => Err(errno::EBADF),
                }
            } else {
                Err(errno::EBADF)
            };
        }
        let port = mach::fd_to_port(fd).ok_or(errno::EBADF)?;
        let msg = Msg {
            id: wire::FILE_CLASS,
            ports: vec![(port, mach::COPY_SEND)],
            data: Vec::new(),
        };
        let result = with_thread(|t| call(t, self.service, &msg));
        mach::release_send(port);
        let reply = result??;
        status(&reply)?.u32()
    }

    fn pipe_file_class(&self, fd: i32) -> Result<u32, Errno> {
        let port = mach::fd_to_port(fd).ok_or(errno::EBADF)?;
        let msg = Msg { id: wire::FILE_CLASS, ports: vec![(port, mach::COPY_SEND)], data: Vec::new() };
        let result = with_thread(|t| call(t, self.service, &msg));
        mach::release_send(port);
        status(&result??)?.u32()
    }

    pub fn connect(name: &str) -> Option<Self> {
        mach::look_up(name).ok().map(|service| Self { service })
    }

    /// The files whose pages the daemon's process shares; each entry is a
    /// send right the caller now holds.
    pub fn shared_files(&self) -> Result<Vec<SharedFile>, Errno> {
        let mut out = Vec::new();
        loop {
            let mut w = Writer::default();
            w.u32(out.len() as u32);
            let msg = Msg {
                id: wire::FILES,
                ports: Vec::new(),
                data: w.0,
            };
            let r = with_thread(|t| call(t, self.service, &msg))??;
            let mut rd = status(&r)?;
            let total = rd.u32()? as usize;
            for &entry in &r.ports {
                let (dev, ino, size) = (rd.u64()?, rd.u64()?, rd.u64()?);
                out.push(SharedFile {
                    dev,
                    ino,
                    size,
                    entry,
                });
            }
            if r.ports.is_empty() || out.len() >= total {
                return Ok(out);
            }
        }
    }

    /// `open("/dev/binder")`. Returns the file and the guest fd.
    pub fn open(
        &self,
        device: Device,
        nonblocking: bool,
        cloexec: bool,
        euid: u32,
        security_context: &str,
    ) -> Result<BinderFile, Errno> {
        let mut sv = [0i32; 2];
        // SAFETY: socketpair into a local array.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) } < 0 {
            return Err(errno::ENOMEM);
        }
        let [guest, daemon] = sv;
        let port = mach::fd_to_port(daemon);
        // SAFETY: the daemon end now travels as a fileport.
        unsafe { libc::close(daemon) };
        // SAFETY: closing our own end on failure.
        let close_guest = || unsafe { libc::close(guest) };
        let Some(port) = port else {
            close_guest();
            return Err(errno::ENOMEM);
        };
        let mut w = Writer::default();
        w.u32(match device {
            Device::Binder => 0,
            Device::HwBinder => 1,
            Device::VndBinder => 2,
        })
        .u32(nonblocking as u32)
        .u32(euid)
        .bytes(security_context.as_bytes());
        let msg = Msg {
            id: wire::OPEN,
            ports: vec![(port, mach::MOVE_SEND)],
            data: w.0,
        };
        let r = with_thread(|t| call(t, self.service, &msg)).and_then(|r| r);
        let file = r.and_then(|r| {
            status(&r)?;
            r.ports.first().copied().ok_or(wire::EPROTO)
        });
        match file {
            Ok(port) => {
                if cloexec {
                    // SAFETY: flagging our own fd.
                    unsafe { libc::fcntl(guest, libc::F_SETFD, libc::FD_CLOEXEC) };
                }
                Ok(BinderFile { port, fd: guest })
            }
            Err(e) => {
                close_guest();
                Err(e)
            }
        }
    }
}

/// An open binder file description of this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinderFile {
    /// The file port (a send right).
    pub port: Port,
    /// The guest fd that stands for it.
    pub fd: i32,
}

impl BinderFile {
    fn control(
        &self,
        t: &mut ThreadState,
        id: i32,
        data: Vec<u8>,
    ) -> Result<mach::Received, Errno> {
        let r = call(
            t,
            self.port,
            &Msg {
                id,
                ports: Vec::new(),
                data,
            },
        )?;
        status(&r)?;
        Ok(r)
    }

    fn thread_port(&self, t: &mut ThreadState, tid: i32) -> Result<Port, Errno> {
        if let Some(p) = t.threads.get(&self.port) {
            return Ok(*p);
        }
        let mut w = Writer::default();
        w.i32(tid);
        let r = self.control(t, wire::THREAD, w.0)?;
        let p = *r.ports.first().ok_or(wire::EPROTO)?;
        t.threads.insert(self.port, p);
        Ok(p)
    }

    /// Flush this actual open description after a public Linux fd close.
    /// The caller retains a private close pin until this acknowledgement.
    pub fn flush(&self) -> Result<(), Errno> {
        with_thread(|t| self.control(t, wire::FLUSH, Vec::new()))??;
        Ok(())
    }

    /// Install the receive buffer at `[vm_start, vm_start+len)`, which the
    /// caller reserved; it becomes a read-only view of the daemon's buffer.
    pub fn mmap(&self, vm_start: u64, len: u64) -> Result<(), Errno> {
        let mut w = Writer::default();
        w.u64(vm_start).u64(len);
        let r = with_thread(|t| self.control(t, wire::MMAP, w.0))??;
        let entry = *r.ports.first().ok_or(wire::EPROTO)?;
        let mapped = mach::map_read_only(entry, vm_start, len.min(MAX_MAPPING as u64));
        mach::release_send(entry);
        mapped.map_err(|_| errno::ENOMEM)
    }

    /// The calling thread polls this fd (epoll_ctl, poll): `binder_poll`.
    pub fn poll(&self, tid: i32) -> Result<(), Errno> {
        let mut w = Writer::default();
        w.i32(tid);
        let r = with_thread(|t| self.control(t, wire::POLL, w.0))??;
        let mut rd = status(&r)?;
        self.drain(rd.u32()?);
        Ok(())
    }

    /// A signal is due for guest thread `tid`: its blocking read (or the
    /// next one it starts) fails with EINTR. Called from any thread.
    pub fn interrupt(&self, tid: i32) -> Result<(), Errno> {
        let mut w = Writer::default();
        w.i32(tid);
        with_thread(|t| self.control(t, wire::INTERRUPT, w.0))??;
        Ok(())
    }

    fn drain(&self, n: u32) {
        let mut sink = [0u8; 8];
        for _ in 0..n.min(sink.len() as u32) {
            // SAFETY: consuming a readiness byte from our own socket.
            unsafe { libc::recv(self.fd, sink.as_mut_ptr().cast(), 1, libc::MSG_DONTWAIT) };
        }
    }

    /// `ioctl(fd, cmd, arg)` with `arg` a guest address.
    pub fn ioctl(
        &self,
        tid: i32,
        cmd: u32,
        arg: u64,
        mem: &mut dyn UserMemory,
    ) -> Result<(), Errno> {
        self.ioctl_reserving(tid, cmd, arg, mem, RESERVED_FDS)
    }

    /// [`Self::ioctl`] with at least `reserve` placeholders for a read.
    fn ioctl_reserving(
        &self,
        tid: i32,
        cmd: u32,
        arg: u64,
        mem: &mut dyn UserMemory,
        reserve: usize,
    ) -> Result<(), Errno> {
        let size = ioc_size(cmd);
        // binder_ioctl never reads the argument of these; libbinder's
        // thread destructor passes 0 for BINDER_THREAD_EXIT.
        let ignores_arg = matches!(cmd, BINDER_THREAD_EXIT | BINDER_SET_CONTEXT_MGR);
        let reads_arg = cmd >> 30 & 1 != 0 && !ignores_arg;
        let writes_arg = cmd >> 31 != 0;
        let mut io = Ioctl {
            cmd,
            arg: if reads_arg && size > 0 {
                mem.read(arg, size)?
            } else {
                vec![0; size]
            },
            ..Default::default()
        };
        let mut reads = false;
        if cmd == BINDER_WRITE_READ {
            let bwr = WriteRead::decode(&io.arg);
            gather_writes(&bwr, mem, &mut io);
            reads = bwr.read_size > bwr.read_consumed;
        }
        let fds = std::mem::take(&mut io.fds);
        io.file_classes.clear();
        io.regular.clear();
        io.sockets.clear();
        let exports = fds
            .iter()
            .map(|fd| mem.export_fd(*fd as i32))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ports = Vec::new();
        for (fd, exported) in fds.into_iter().zip(exports) {
            let regular = match mem.regular_export(fd as i32) {
                Ok(regular) => regular,
                Err(error) => { for (port, _) in ports { mach::release_send(port); } return Err(error); }
            };
            let socket=match mem.socket_export(fd as i32){Ok(socket)=>socket,Err(error)=>{for(port,_)in ports{mach::release_send(port);}return Err(error);}};
            let class = mem.file_class(fd as i32);
            if (class==crate::socket_scm::CLASS)!=socket.is_some(){for(port,_)in ports{mach::release_send(port);}return Err(wire::EPROTO);}
            if (class == crate::regular_file::CLASS) != regular.is_some() { for (port, _) in ports { mach::release_send(port); } return Err(wire::EPROTO); }
            let needed = 1 + regular.as_ref().is_some_and(|regular| regular.metadata.writer) as usize;
            if ports.len() + needed > mach::MAX_PORTS { break; }
            if let Some(p) = mach::fd_to_port(exported) {
                let writer = if let Some(regular) = &regular {
                    if regular.metadata.writer != regular.writer_fd.is_some() {
                        mach::release_send(p);
                        for (port, _) in ports { mach::release_send(port); }
                        return Err(wire::EPROTO);
                    }
                    match regular.writer_fd {
                        Some(fd) => match mach::fd_to_port(fd) {
                            Some(port) => Some(port),
                            None => { mach::release_send(p); for (port, _) in ports { mach::release_send(port); } return Err(errno::EBADF); }
                        },
                        None => None,
                    }
                } else { None };
                io.fds.push(fd);
                io.file_classes.push(class);
                io.regular.push(regular.map(|regular| regular.metadata));
                io.sockets.push(socket);
                ports.push((p, mach::MOVE_SEND));
                if let Some(writer) = writer { ports.push((writer, mach::MOVE_SEND)); }
            }
        }
        if let Err(error) = io.validate() { for (port, _) in ports { mach::release_send(port); } return Err(error); }
        let (reply, writers) = with_thread(|t| -> Result<(IoctlReply, Vec<Option<crate::regular_file::WriterPort>>), Errno> {
            if reads {
                refill_reserved(&mut t.reserved, reserve);
                io.grow = t.reserved.len() >= reserve;
            }
            io.reserved = t.reserved.iter().map(|fd| *fd as u32).collect();
            let port = self.thread_port(t, tid)?;
            let msg = Msg {
                id: wire::IOCTL,
                ports,
                data: io.encode(),
            };
            let r = call(t, port, &msg)?;
            let reply = match IoctlReply::decode(&r.data) {
                Ok(reply) => reply,
                Err(error) => { for port in r.ports { mach::release_send(port); } return Err(error); }
            };
            let count = reply.installs.len() + reply.regular.iter().flatten().filter(|metadata| metadata.writer).count();
            if count != r.ports.len() {
                for port in r.ports { mach::release_send(port); }
                reject_deliveries(t, port, &reply)?;
                return Err(wire::EPROTO);
            }
            let mut ports = r.ports.into_iter();
            let mut writers = Vec::with_capacity(reply.installs.len());
            for (index, fd) in reply.installs.iter().enumerate() {
                let p = ports.next().unwrap();
                let result = mem.install_fileport(*fd as i32, p);
                mach::release_send(p);
                if let Err(error) = result {
                    for port in ports { mach::release_send(port); }
                    for fd in reply.installs.iter().chain(&reply.closes) {
                        if let Err(secondary) = mem.close_file(*fd as i32) { eprintln!("Binder import rollback close fd={fd}: errno {secondary}"); }
                        t.reserved.retain(|reserved| *reserved != *fd as i32);
                    }
                    reject_deliveries(t, port, &reply)?;
                    return Err(error);
                }
                writers.push(if reply.regular[index].as_ref().is_some_and(|metadata| metadata.writer) {
                    Some(crate::regular_file::WriterPort::new(ports.next().unwrap()))
                } else { None });
                t.reserved.retain(|reserved| *reserved != *fd as i32);
            }
            if cmd == BINDER_THREAD_EXIT
                && reply.status == 0
                && let Some(p) = t.threads.remove(&self.port)
            {
                mach::release_send(p);
            }
            Ok((reply, writers))
        })??;
        for (index, (fd, writer)) in reply.installs.iter().zip(writers).enumerate() {
            let result = match (&reply.regular[index],reply.sockets[index]) {
                (None,Some(receipt))=>mem.install_socket(*fd as i32,receipt),
                (Some(metadata),None) => mem.install_regular(*fd as i32, metadata, writer),
                (None,None) => mem.install_typed(*fd as i32, reply.file_classes[index]),
                _=>Err(wire::EPROTO),
            };
            if let Err(error) = result {
                for fd in reply.installs.iter().chain(&reply.closes) {
                    if let Err(secondary) = mem.close_file(*fd as i32) { eprintln!("Binder typed import rollback close fd={fd}: errno {secondary}"); }
                }
                with_thread(|t| -> Result<(), Errno> {
                    let port = self.thread_port(t, tid)?;
                    reject_deliveries(t, port, &reply)
                })??;
                self.drain(reply.drain);
                return Err(error);
            }
        }
        for (addr, bytes) in &reply.writes {
            mem.write(*addr, bytes)?;
        }
        let mut close_error = None;
        for fd in &reply.closes {
            if let Err(error) = mem.close_file(*fd as i32) { close_error.get_or_insert(error); }
        }
        self.drain(reply.drain);
        if let Some(error) = close_error { return Err(error); }
        if writes_arg && size > 0 && (reply.status == 0 || cmd == BINDER_WRITE_READ) {
            mem.write(arg, &reply.arg)?;
        }
        if reply.status == 0 && reply.want_fds > 0 {
            // The read stopped before a transaction with more files than we
            // had placeholders for; the updated arg continues it.
            return self.ioctl_reserving(tid, cmd, arg, mem, reply.want_fds as usize);
        }
        match reply.status {
            0 => Ok(()),
            e => Err(e),
        }
    }
}

fn reject_deliveries(
    thread: &mut ThreadState,
    port: Port,
    reply: &IoctlReply,
) -> Result<(), Errno> {
    for (buffer, id) in reply.deliveries.iter().rev() {
        let mut data = Writer::default();
        data.u64(*buffer).u64(*id);
        let result = call(
            thread,
            port,
            &Msg {
                id: wire::REJECT_DELIVERY,
                ports: Vec::new(),
                data: data.0,
            },
        )?;
        let result = IoctlReply::decode(&result.data)?;
        if result.status != 0 {
            return Err(result.status);
        }
    }
    Ok(())
}

/// Top the pool up to `count` placeholders. A placeholder only holds its
/// number, so it is a socket that was never connected: installing a file
/// over it (dup2) closes a socket, where closing a vnode such as /dev/null
/// costs microseconds of host kernel time. New ones are duplicates of one
/// already held.
fn refill_reserved(pool: &mut Vec<i32>, count: usize) {
    while pool.len() < count {
        // SAFETY: duplicating a placeholder, or a new socket, upward.
        let fd = unsafe {
            match pool.first() {
                Some(&held) => libc::fcntl(held, libc::F_DUPFD_CLOEXEC, RESERVED_FD_BASE),
                None => {
                    let s = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
                    if s < 0 {
                        return;
                    }
                    let fd = libc::fcntl(s, libc::F_DUPFD_CLOEXEC, RESERVED_FD_BASE);
                    libc::close(s);
                    fd
                }
            }
        };
        if fd >= 0 {
            pool.push(fd);
        } else if !pool.is_empty()
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::EBADF)
        {
            // The guest closed that number: it holds nothing of ours.
            pool.remove(0);
        } else {
            return;
        }
    }
}

/// Put a received file at the reserved number `fd` (close-on-exec, as the
/// driver installs it).
fn install(fd: i32, port: Port) -> Result<(), Errno> {
    let result = (|| {
        let new = mach::port_to_fd(port).ok_or(errno::EBADF)?;
        let result = if unsafe { libc::dup2(new, fd) } < 0 {
            Err(errno::EBADF)
        } else if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            Err(errno::EBADF)
        } else {
            Ok(())
        };
        unsafe { libc::close(new) };
        result
    })();
    result
}

/// Gather the guest memory the write stream makes the driver read.
fn gather_writes(bwr: &WriteRead, mem: &mut dyn UserMemory, io: &mut Ioctl) {
    if bwr.write_size <= bwr.write_consumed || bwr.write_size > MAX_MAPPING as u64 {
        return;
    }
    let start = bwr.write_buffer + bwr.write_consumed;
    let Ok(stream) = mem.read(start, (bwr.write_size - bwr.write_consumed) as usize) else {
        return;
    };
    let mut pos = 0;
    while pos + 4 <= stream.len() {
        let cmd = u32::from_le_bytes(stream[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let Some(size) = command_payload_size(cmd) else {
            break;
        };
        if pos + size > stream.len() {
            break;
        }
        if matches!(
            cmd,
            BC_TRANSACTION | BC_REPLY | BC_TRANSACTION_SG | BC_REPLY_SG
        ) {
            let tr = TransactionData::decode(&stream[pos..]);
            let sg = matches!(cmd, BC_TRANSACTION_SG | BC_REPLY_SG);
            gather_transaction(&tr, sg, mem, io);
        }
        pos += size;
    }
    io.segments.push((start, stream));
}

fn read_segment(mem: &mut dyn UserMemory, addr: u64, len: u64, io: &mut Ioctl) -> Option<Vec<u8>> {
    if len == 0 || len > MAX_MAPPING as u64 {
        return None;
    }
    let bytes = mem.read(addr, len as usize).ok()?;
    io.segments.push((addr, bytes.clone()));
    Some(bytes)
}

/// A transaction's data and offsets, its scatter-gather buffers, and the
/// fds of its FD and FDA objects.
fn gather_transaction(tr: &TransactionData, sg: bool, mem: &mut dyn UserMemory, io: &mut Ioctl) {
    let data = read_segment(mem, tr.buffer, tr.data_size, io).unwrap_or_default();
    let offsets = read_segment(mem, tr.offsets, tr.offsets_size, io).unwrap_or_default();
    let mut buffers: Vec<Option<(u64, Vec<u8>)>> = Vec::new();
    let add_fd = |fd: u32, io: &mut Ioctl| {
        if !io.fds.contains(&fd) {
            io.fds.push(fd);
        }
    };
    for chunk in offsets.chunks_exact(8) {
        let off = u64::from_le_bytes(chunk.try_into().unwrap()) as usize;
        let object = |len: usize| data.get(off..off.checked_add(len)?);
        let mut buffer = None;
        match object(4).map(|k| u32::from_le_bytes(k.try_into().unwrap())) {
            Some(BINDER_TYPE_FD) => {
                if let Some(o) = object(FD_OBJECT_SIZE) {
                    add_fd(u32::from_le_bytes(o[8..12].try_into().unwrap()), io);
                }
            }
            Some(BINDER_TYPE_PTR) if sg => {
                if let Some(o) = object(BUFFER_OBJECT_SIZE) {
                    let bp = BufferObject::decode(o);
                    buffer = read_segment(mem, bp.buffer, bp.length, io).map(|b| (bp.buffer, b));
                }
            }
            Some(BINDER_TYPE_FDA) if sg => {
                if let Some(o) = object(FD_ARRAY_OBJECT_SIZE) {
                    let fda = FdArrayObject::decode(o);
                    if let Some(Some((_, parent))) = buffers.get(fda.parent as usize) {
                        let base = fda.parent_offset as usize;
                        for i in 0..fda.num_fds.min(mach::MAX_PORTS as u64) as usize {
                            if let Some(b) = parent.get(base + i * 4..base + i * 4 + 4) {
                                add_fd(u32::from_le_bytes(b.try_into().unwrap()), io);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        buffers.push(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_socket(fd: i32) -> bool {
        // SAFETY: fstat into a local buffer.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        unsafe { libc::fstat(fd, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFSOCK }
    }

    /// Placeholders are distinct close-on-exec sockets high up, and a
    /// number the process closed behind the pool's back is replaced.
    #[test]
    fn placeholders_are_sockets_and_survive_a_closed_one() {
        let mut pool = Vec::new();
        refill_reserved(&mut pool, RESERVED_FDS);
        assert_eq!(pool.len(), RESERVED_FDS);
        let mut sorted = pool.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), RESERVED_FDS);
        for &fd in &pool {
            assert!(fd >= RESERVED_FD_BASE && is_socket(fd));
            // SAFETY: plain fcntl on our fd.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, libc::FD_CLOEXEC);
        }
        let gone = pool[0];
        // SAFETY: closing our own placeholder, as a guest might.
        unsafe { libc::close(gone) };
        refill_reserved(&mut pool, RESERVED_FDS + 1);
        assert_eq!(pool.len(), RESERVED_FDS + 1);
        assert!(pool.iter().all(|&fd| is_socket(fd)));
        for fd in pool {
            // SAFETY: our placeholders.
            unsafe { libc::close(fd) };
        }
    }
    #[test]
    fn failed_host_descriptor_install_cannot_succeed_with_a_placeholder() {
        use std::os::fd::AsRawFd;
        let source = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(install(source.as_raw_fd(), mach::NULL), Err(errno::EBADF));
        assert!(unsafe { libc::fcntl(source.as_raw_fd(), libc::F_GETFD) } >= 0);
        let port = mach::fd_to_port(source.as_raw_fd()).unwrap();
        assert_eq!(install(-1, port), Err(errno::EBADF));
        assert!(unsafe { libc::fcntl(source.as_raw_fd(), libc::F_GETFD) } >= 0);
    }
}
