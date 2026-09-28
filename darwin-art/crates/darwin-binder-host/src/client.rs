//! The guest side: what the syscall layer does for `open`, `ioctl`, `mmap`
//! and poll registration on a binder fd.
//!
//! The guest's binder fd is one end of a Unix socket pair; the daemon holds
//! the other end. It is readable while the daemon reports work (epoll and
//! poll work on it unchanged), and its last close is the file's release.
//! Guest fds are host fds.

use std::cell::RefCell;
use std::collections::HashMap;

use darwin_binder_driver::uapi::*;
use darwin_binder_driver::{Device, Errno, MAX_MAPPING, errno};

use crate::mach::{self, Buffer, Msg, Port};
use crate::wire::{self, Ioctl, IoctlReply, Reader, Writer};

/// Guest memory, as the calling process sees it.
pub trait UserMemory {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, Errno>;
    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), Errno>;
    /// A received file now sits at `fd` (for the caller's fd bookkeeping).
    fn installed(&mut self, _fd: i32) {}
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
    pub fn connect(name: &str) -> Option<Self> {
        mach::look_up(name).ok().map(|service| Self { service })
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
        let mut ports = Vec::new();
        for fd in fds {
            if ports.len() == mach::MAX_PORTS {
                break;
            }
            // An fd that is not open has no port; the driver fails the
            // transaction with EBADF.
            if let Some(p) = mach::fd_to_port(fd as i32) {
                io.fds.push(fd);
                ports.push((p, mach::MOVE_SEND));
            }
        }
        let reply = with_thread(|t| -> Result<IoctlReply, Errno> {
            if reads {
                refill_reserved(&mut t.reserved);
            }
            io.reserved = t.reserved.iter().map(|fd| *fd as u32).collect();
            let port = self.thread_port(t, tid)?;
            let msg = Msg {
                id: wire::IOCTL,
                ports,
                data: io.encode(),
            };
            let r = call(t, port, &msg)?;
            let reply = IoctlReply::decode(&r.data)?;
            for (fd, p) in reply.installs.iter().zip(&r.ports) {
                install(*fd as i32, *p);
                t.reserved.retain(|r| *r != *fd as i32);
            }
            for p in r.ports.iter().skip(reply.installs.len()) {
                mach::release_send(*p);
            }
            if cmd == BINDER_THREAD_EXIT
                && reply.status == 0
                && let Some(p) = t.threads.remove(&self.port)
            {
                mach::release_send(p);
            }
            Ok(reply)
        })??;
        for fd in &reply.installs {
            mem.installed(*fd as i32);
        }
        for (addr, bytes) in &reply.writes {
            mem.write(*addr, bytes)?;
        }
        for fd in &reply.closes {
            mem.closed(*fd as i32);
            // SAFETY: closing an fd the driver installed and now frees.
            unsafe { libc::close(*fd as i32) };
        }
        self.drain(reply.drain);
        if writes_arg && size > 0 && (reply.status == 0 || cmd == BINDER_WRITE_READ) {
            mem.write(arg, &reply.arg)?;
        }
        match reply.status {
            0 => Ok(()),
            e => Err(e),
        }
    }
}

fn refill_reserved(pool: &mut Vec<i32>) {
    while pool.len() < RESERVED_FDS {
        // SAFETY: opening /dev/null and duplicating it upward.
        let fd = unsafe {
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if null < 0 {
                return;
            }
            let fd = libc::fcntl(null, libc::F_DUPFD_CLOEXEC, RESERVED_FD_BASE);
            libc::close(null);
            fd
        };
        if fd < 0 {
            return;
        }
        pool.push(fd);
    }
}

/// Put a received file at the reserved number `fd` (close-on-exec, as the
/// driver installs it).
fn install(fd: i32, port: Port) {
    if let Some(new) = mach::port_to_fd(port) {
        // SAFETY: replacing our placeholder with the received file.
        unsafe {
            libc::dup2(new, fd);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::close(new);
        }
    }
    mach::release_send(port);
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
