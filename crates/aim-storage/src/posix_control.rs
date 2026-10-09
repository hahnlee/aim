//! Native process-owned byte-range locks. Fileports are inspected before an
//! fd is created: closing a redundant vnode fd would revoke every POSIX lock.
use crate::{inode_lease::Identity, process_namespace::ProcessIdentity};
use std::{
    collections::HashMap,
    ffi::CString,
    io,
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
fn error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub process: ProcessIdentity,
    pub guest_pid: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub kind: i16,
    pub start: i64,
    pub len: i64,
}
pub fn normalize(
    kind: i16,
    whence: i16,
    start: i64,
    len: i64,
    current: i64,
    size: i64,
) -> io::Result<Range> {
    if !matches!(kind, 0..=2) {
        return Err(error(libc::EINVAL));
    }
    let base = match whence {
        0 => 0,
        1 => current,
        2 => size,
        _ => return Err(error(libc::EINVAL)),
    } as i128;
    let position = base + start as i128;
    if position < 0 {
        return Err(error(libc::EINVAL));
    }
    if position > i64::MAX as i128 {
        return Err(error(libc::EOVERFLOW));
    }
    let (first, length) = if len < 0 {
        (position + len as i128, -(len as i128))
    } else {
        (position, len as i128)
    };
    if first < 0 {
        return Err(error(libc::EINVAL));
    }
    if length > 0 && first + length - 1 > i64::MAX as i128 {
        return Err(error(libc::EOVERFLOW));
    }
    Ok(Range {
        kind,
        start: first as i64,
        len: if length > i64::MAX as i128 {
            0
        } else {
            length as i64
        },
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Operation {
    Register = 1,
    Get = 2,
    Set = 3,
    Wait = 4,
    Cancel = 5,
    Close = 6,
    Exit = 7,
    Attach = 8,
    ExternalAttach = 9,
}
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub operation: Operation,
    pub request: u64,
    pub ticket: u64,
    pub key: [u8; 36],
    pub access: i32,
    pub range: Range,
    pub errno: i32,
    pub host_pid: i32,
    pub guest_pid: i32,
}
impl Frame {
    pub fn new(operation: Operation, request: u64) -> Self {
        Self {
            operation,
            request,
            ticket: 0,
            key: [0; 36],
            access: 0,
            range: Range {
                kind: 2,
                start: 0,
                len: 0,
            },
            errno: 0,
            host_pid: 0,
            guest_pid: 0,
        }
    }
    pub fn encode(self) -> [u8; 112] {
        let mut b = [0; 112];
        b[..8].copy_from_slice(b"AIMPLK01");
        b[8..12].copy_from_slice(&(self.operation as u32).to_le_bytes());
        b[16..24].copy_from_slice(&self.request.to_le_bytes());
        b[24..32].copy_from_slice(&self.ticket.to_le_bytes());
        b[32..68].copy_from_slice(&self.key);
        b[68..72].copy_from_slice(&self.access.to_le_bytes());
        b[72..74].copy_from_slice(&self.range.kind.to_le_bytes());
        b[80..88].copy_from_slice(&self.range.start.to_le_bytes());
        b[88..96].copy_from_slice(&self.range.len.to_le_bytes());
        b[96..100].copy_from_slice(&self.errno.to_le_bytes());
        b[100..104].copy_from_slice(&self.host_pid.to_le_bytes());
        b[104..108].copy_from_slice(&self.guest_pid.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> io::Result<Self> {
        if b.len() != 112
            || &b[..8] != b"AIMPLK01"
            || b[12..16]
                .iter()
                .chain(&b[74..80])
                .chain(&b[108..])
                .any(|v| *v != 0)
        {
            return Err(error(libc::EPROTO));
        }
        let operation = match u32::from_le_bytes(b[8..12].try_into().unwrap()) {
            1 => Operation::Register,
            2 => Operation::Get,
            3 => Operation::Set,
            4 => Operation::Wait,
            5 => Operation::Cancel,
            6 => Operation::Close,
            7 => Operation::Exit,
            8 => Operation::Attach,
            9 => Operation::ExternalAttach,
            _ => return Err(error(libc::EPROTO)),
        };
        Ok(Self {
            operation,
            request: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            ticket: u64::from_le_bytes(b[24..32].try_into().unwrap()),
            key: b[32..68].try_into().unwrap(),
            access: i32::from_le_bytes(b[68..72].try_into().unwrap()),
            range: Range {
                kind: i16::from_le_bytes(b[72..74].try_into().unwrap()),
                start: i64::from_le_bytes(b[80..88].try_into().unwrap()),
                len: i64::from_le_bytes(b[88..96].try_into().unwrap()),
            },
            errno: i32::from_le_bytes(b[96..100].try_into().unwrap()),
            host_pid: i32::from_le_bytes(b[100..104].try_into().unwrap()),
            guest_pid: i32::from_le_bytes(b[104..108].try_into().unwrap()),
        })
    }
}
unsafe extern "C" {
    static mach_task_self_: u32;
    static bootstrap_port: u32;
    fn mach_port_allocate(task: u32, right: u32, name: *mut u32) -> i32;
    fn mach_port_insert_right(task: u32, name: u32, right: u32, kind: u32) -> i32;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: i32) -> i32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    fn mach_msg(
        msg: *mut u8,
        options: i32,
        send: u32,
        receive: u32,
        name: u32,
        timeout: u32,
        notify: u32,
    ) -> i32;
    fn bootstrap_register(bp: u32, name: *const i8, port: u32) -> i32;
    fn bootstrap_look_up(bp: u32, name: *const i8, port: *mut u32) -> i32;
    fn fileport_makeport(fd: i32, port: *mut u32) -> i32;
    fn fileport_makefd(port: u32) -> i32;
    fn proc_pidfileportinfo(
        pid: i32,
        port: u32,
        flavor: i32,
        buffer: *mut libc::c_void,
        size: i32,
    ) -> i32;
}
fn task() -> u32 {
    unsafe { mach_task_self_ }
}
pub struct SendRight(u32);
impl SendRight {
    pub fn adopt(port: u32) -> io::Result<Self> {
        if port == 0 {
            return Err(error(libc::EINVAL));
        }
        Ok(Self(port))
    }
    pub fn name(&self) -> u32 {
        self.0
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        if unsafe { mach_port_mod_refs(task(), self.0, 0, 1) } != 0 {
            return Err(error(libc::EIO));
        }
        Ok(Self(self.0))
    }
    pub fn from_fd(fd: BorrowedFd<'_>) -> io::Result<Self> {
        let mut port = 0;
        if unsafe { fileport_makeport(fd.as_raw_fd(), &mut port) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Self::adopt(port)
    }
    pub fn into_fd(&self) -> io::Result<OwnedFd> {
        let fd = unsafe { fileport_makefd(self.0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
    pub fn inode(&self) -> io::Result<Identity> {
        self.inode_access().map(|value| value.0)
    }
    fn inode_access(&self) -> io::Result<(Identity, i32)> {
        // SDK vnode_fdinfowithpath: proc_fileinfo(24), vinfo_stat(136),
        // vnode_info tail(16), path(1024). No native fd is installed by this call.
        let mut b = [0u8; 1200];
        let size =
            unsafe { proc_pidfileportinfo(libc::getpid(), self.0, 2, b.as_mut_ptr().cast(), 1200) };
        if size <= 0 {
            return Err(io::Error::last_os_error());
        }
        if size != 1200 {
            return Err(error(libc::EIO));
        }
        let identity = Identity::from_bytes(&{
            let mut v = [0u8; 36];
            v[..8].copy_from_slice(
                &(u32::from_ne_bytes(b[24..28].try_into().unwrap()) as u64).to_le_bytes(),
            );
            v[8..16]
                .copy_from_slice(&u64::from_ne_bytes(b[32..40].try_into().unwrap()).to_le_bytes());
            v[16..20].copy_from_slice(
                &u32::from_ne_bytes(b[136..140].try_into().unwrap()).to_le_bytes(),
            );
            v[20..28]
                .copy_from_slice(&i64::from_ne_bytes(b[96..104].try_into().unwrap()).to_le_bytes());
            v[28..36].copy_from_slice(
                &i64::from_ne_bytes(b[104..112].try_into().unwrap()).to_le_bytes(),
            );
            v
        })?;
        let access = match word(&b, 0) & 3 {
            1 => 0,
            2 => 1,
            3 => 2,
            _ => return Err(error(libc::EBADF)),
        };
        Ok((identity, access))
    }
}
impl Drop for SendRight {
    fn drop(&mut self) {
        unsafe {
            mach_port_deallocate(task(), self.0);
        }
    }
}
struct Receive(u32);
impl Receive {
    fn new() -> io::Result<Self> {
        let mut port = 0;
        if unsafe { mach_port_allocate(task(), 1, &mut port) } != 0 {
            return Err(error(libc::EIO));
        }
        Ok(Self(port))
    }
}
impl Drop for Receive {
    fn drop(&mut self) {
        unsafe {
            mach_port_mod_refs(task(), self.0, 1, -1);
        }
    }
}
fn put(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
}
fn message(
    dest: u32,
    reply: u32,
    frame: Frame,
    proof: Option<&SendRight>,
    once: bool,
) -> ([u64; 32], u32) {
    let mut buffer = [0u64; 32];
    let b = unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), 256) };
    let count = proof.is_some() as u32;
    let offset = 28 + count as usize * 12;
    let size = offset + 112;
    put(b, 0, 0x80000000 | if once { 18 } else { 19 | (21 << 8) });
    put(b, 4, size as u32);
    put(b, 8, dest);
    put(b, 12, reply);
    put(b, 24, count);
    if let Some(proof) = proof {
        put(b, 28, proof.0);
        b[38] = 19;
        b[39] = 0;
    }
    b[offset..offset + 112].copy_from_slice(&frame.encode());
    (buffer, size as u32)
}
fn send(
    dest: u32,
    reply: u32,
    frame: Frame,
    proof: Option<&SendRight>,
    once: bool,
) -> io::Result<()> {
    let (mut buffer, size) = message(dest, reply, frame, proof, once);
    let result = unsafe { mach_msg(buffer.as_mut_ptr().cast(), 1 | 16, size, 0, 0, 1000, 0) };
    if result != 0 {
        return Err(error(libc::EIO));
    }
    Ok(())
}
// XNU proc_info_private.h: PROC_PIDUNIQIDENTIFIERINFO(17), API struct
// proc_uniqidentifierinfo(56), p_uniqueid at16 and p_idversion at32.
fn process_generation(pid: i32) -> io::Result<(u64, u32)> {
    let mut bytes = [0u8; 56];
    let size = unsafe { libc::proc_pidinfo(pid, 17, 0, bytes.as_mut_ptr().cast(), 56) };
    if size <= 0 {
        return Err(io::Error::last_os_error());
    }
    if size != 56 {
        return Err(error(libc::EIO));
    }
    Ok((
        u64::from_ne_bytes(bytes[16..24].try_into().unwrap()),
        u32::from_ne_bytes(bytes[32..36].try_into().unwrap()),
    ))
}
fn authenticate_actor(pid: i32, audit_version: u32) -> io::Result<ProcessIdentity> {
    let before = process_generation(pid)?;
    if before.1 != audit_version {
        return Err(error(libc::EPERM));
    }
    let identity = ProcessIdentity::running(pid)?;
    if process_generation(pid)? != before {
        return Err(error(libc::EPERM));
    }
    Ok(identity)
}
struct Incoming {
    frame: Frame,
    proof: Option<SendRight>,
    reply: u32,
    pid: i32,
    pid_version: u32,
}
fn receive(port: u32, timeout: Duration) -> io::Result<Incoming> {
    let mut buffer = [0u64; 32];
    let result = unsafe {
        mach_msg(
            buffer.as_mut_ptr().cast(),
            2 | 0x100 | 0x400 | (3 << 24),
            0,
            256,
            port,
            timeout.as_millis().min(u32::MAX as u128) as u32,
            0,
        )
    };
    if result == 0x10004003 {
        return Err(error(libc::ETIMEDOUT));
    }
    if result == 0x10004005 {
        return Err(error(libc::EINTR));
    }
    if result != 0 {
        return Err(error(libc::EIO));
    }
    let b = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), 256) };
    let size = word(b, 4) as usize;
    let count = word(b, 24);
    let offset = 28 + count as usize * 12;
    if count > 1 || word(b, 0) & 0x80000000 == 0 || size != offset + 112 || size + 52 > 256 {
        return Err(error(libc::EPROTO));
    }
    let proof = if count == 1 {
        if b[39] != 0 {
            return Err(error(libc::EPROTO));
        }
        Some(SendRight::adopt(word(b, 28))?)
    } else {
        None
    };
    let trailer = size.next_multiple_of(4);
    if word(b, trailer + 4) < 52 {
        return Err(error(libc::EPROTO));
    }
    Ok(Incoming {
        frame: Frame::decode(&b[offset..size])?,
        proof,
        reply: word(b, 8),
        pid: word(b, trailer + 40) as i32,
        pid_version: word(b, trailer + 48),
    })
}
pub struct Reply(Option<u32>);
impl Reply {
    pub fn send(mut self, frame: Frame) -> io::Result<()> {
        let port = self.0.ok_or_else(|| error(libc::EPROTO))?;
        send(port, 0, frame, None, true)?;
        self.0 = None;
        Ok(())
    }
}
impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(port) = self.0.take() {
            unsafe {
                mach_port_deallocate(task(), port);
            }
        }
    }
}
pub struct Request {
    pub frame: Frame,
    pub proof: Option<SendRight>,
    pub actor: ProcessIdentity,
    pub reply: Reply,
}
pub struct Endpoint {
    receive: Receive,
    _send: SendRight,
}
impl Endpoint {
    pub fn register(name: &str) -> io::Result<Self> {
        let name = CString::new(name).map_err(|_| error(libc::EINVAL))?;
        let receive = Receive::new()?;
        if unsafe { mach_port_insert_right(task(), receive.0, receive.0, 20) } != 0 {
            return Err(error(libc::EIO));
        }
        let right = SendRight::adopt(receive.0)?;
        if unsafe { bootstrap_register(bootstrap_port, name.as_ptr(), right.0) } != 0 {
            return Err(error(libc::EADDRINUSE));
        }
        Ok(Self {
            receive,
            _send: right,
        })
    }
    pub fn receive(&self, timeout: Duration) -> io::Result<Request> {
        let incoming = receive(self.receive.0, timeout)?;
        let reply = Reply(Some(incoming.reply));
        let actor = authenticate_actor(incoming.pid, incoming.pid_version)?;
        Ok(Request {
            frame: incoming.frame,
            proof: incoming.proof,
            actor,
            reply,
        })
    }
}
pub struct Client {
    port: SendRight,
}
pub struct Pending {
    reply: Receive,
    pub request: u64,
}
impl Client {
    pub fn from_right(port: SendRight) -> Self {
        Self { port }
    }
    pub fn lookup(name: &str) -> io::Result<Self> {
        let name = CString::new(name).map_err(|_| error(libc::EINVAL))?;
        let mut port = 0;
        if unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut port) } != 0 {
            return Err(error(libc::ENOENT));
        }
        Ok(Self {
            port: SendRight::adopt(port)?,
        })
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            port: self.port.try_clone()?,
        })
    }
    pub fn capability(&self) -> io::Result<SendRight> {
        self.port.try_clone()
    }
    pub fn begin(&self, frame: Frame, proof: Option<BorrowedFd<'_>>) -> io::Result<Pending> {
        let reply = Receive::new()?;
        let proof = proof.map(SendRight::from_fd).transpose()?;
        send(self.port.0, reply.0, frame, proof.as_ref(), false)?;
        Ok(Pending {
            reply,
            request: frame.request,
        })
    }
    pub fn call(&self, frame: Frame, proof: Option<BorrowedFd<'_>>) -> io::Result<Frame> {
        self.begin(frame, proof)?.wait(Duration::from_secs(5))
    }
    pub fn begin_right(&self, frame: Frame, proof: Option<&SendRight>) -> io::Result<Pending> {
        let reply = Receive::new()?;
        send(self.port.0, reply.0, frame, proof, false)?;
        Ok(Pending {
            reply,
            request: frame.request,
        })
    }
    pub fn call_right(&self, frame: Frame, proof: Option<&SendRight>) -> io::Result<Frame> {
        self.begin_right(frame, proof)?.wait(Duration::from_secs(5))
    }
}
impl Pending {
    pub fn wait_authenticated(
        &self,
        timeout: Duration,
        controller: ProcessIdentity,
    ) -> io::Result<Frame> {
        let incoming = receive(self.reply.0, timeout)?;
        if authenticate_actor(incoming.pid, incoming.pid_version)? != controller {
            return Err(error(libc::EPERM));
        }
        self.frame(incoming)
    }
    fn frame(&self, incoming: Incoming) -> io::Result<Frame> {
        if incoming.frame.request != self.request || incoming.proof.is_some() {
            return Err(error(libc::EPROTO));
        }
        Ok(incoming.frame)
    }
    pub fn wait(&self, timeout: Duration) -> io::Result<Frame> {
        let incoming = receive(self.reply.0, timeout)?;
        self.frame(incoming)
    }
}
struct Backing {
    fd: OwnedFd,
    key: [u8; 36],
    access: i32,
}
struct Active {
    thread: Option<usize>,
    cancelled: bool,
    key: [u8; 36],
}
#[derive(Default)]
struct State {
    next: u64,
    files: HashMap<u64, Arc<Backing>>,
    active: HashMap<u64, Active>,
    closing: std::collections::HashSet<[u8; 36]>,
}
struct Worker {
    state: Mutex<State>,
    changed: Condvar,
}
extern "C" fn interrupt(_: i32) {}
impl Worker {
    fn register(&self, frame: Frame, proof: Option<SendRight>) -> io::Result<u64> {
        let proof = proof.ok_or_else(|| error(libc::EPROTO))?;
        let (identity, access) = proof.inode_access()?;
        if identity.to_bytes() != frame.key || frame.access != access {
            return Err(error(libc::EINVAL));
        }
        let mut state = self.state.lock().unwrap();
        if state.closing.contains(&frame.key) {
            return Err(error(libc::EBADF));
        }
        if let Some((ticket, _)) = state
            .files
            .iter()
            .find(|(_, file)| file.key == frame.key && file.access == frame.access)
        {
            return Ok(*ticket);
        }
        let fd = proof.into_fd()?;
        if Identity::from_fd(std::os::fd::AsFd::as_fd(&fd))? != identity {
            return Err(error(libc::ESTALE));
        }
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if flags & libc::O_ACCMODE != frame.access {
            return Err(error(libc::EBADF));
        }
        state.next = state
            .next
            .checked_add(1)
            .ok_or_else(|| error(libc::EOVERFLOW))?;
        let ticket = state.next;
        state.files.insert(
            ticket,
            Arc::new(Backing {
                fd,
                key: frame.key,
                access: frame.access,
            }),
        );
        Ok(ticket)
    }
    fn reserve(&self, frame: Frame) -> io::Result<Arc<Backing>> {
        let mut state = self.state.lock().unwrap();
        let file = state
            .files
            .get(&frame.ticket)
            .cloned()
            .ok_or_else(|| error(libc::EBADF))?;
        if state.closing.contains(&file.key) || state.active.contains_key(&frame.request) {
            return Err(error(libc::EBADF));
        }
        state.active.insert(
            frame.request,
            Active {
                thread: None,
                cancelled: false,
                key: file.key,
            },
        );
        Ok(file)
    }
    fn run(&self, frame: Frame, file: Arc<Backing>) -> Frame {
        let mut reply = frame;
        reply.host_pid = 0;
        reply.guest_pid = 0;
        let result = (|| -> io::Result<()> {
            let mut signals: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut signals);
                libc::sigaddset(&mut signals, libc::SIGUSR1);
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut());
            }
            let mut state = self.state.lock().unwrap();
            let active = state
                .active
                .get_mut(&frame.request)
                .ok_or_else(|| error(libc::EINTR))?;
            active.thread = Some(unsafe { libc::pthread_self() } as usize);
            if active.cancelled {
                return Err(error(libc::EINTR));
            }
            drop(state);
            if !matches!(frame.range.kind, 0..=2)
                || frame.range.start < 0
                || frame.range.len < 0
                || frame.range.len > 0
                    && frame.range.start.checked_add(frame.range.len - 1).is_none()
            {
                return Err(error(libc::EINVAL));
            }
            if frame.operation != Operation::Get
                && (frame.range.kind == 0 && file.access == 1
                    || frame.range.kind == 1 && file.access == 0)
            {
                return Err(error(libc::EBADF));
            }
            let mut lock = libc::flock {
                l_start: frame.range.start,
                l_len: frame.range.len,
                l_pid: 0,
                l_type: match frame.range.kind {
                    0 => libc::F_RDLCK as i16,
                    1 => libc::F_WRLCK as i16,
                    _ => libc::F_UNLCK as i16,
                },
                l_whence: libc::SEEK_SET as i16,
            };
            let operation = match frame.operation {
                Operation::Get => libc::F_GETLK,
                Operation::Wait => libc::F_SETLKW,
                _ => libc::F_SETLK,
            };
            if unsafe { libc::fcntl(file.fd.as_raw_fd(), operation, &mut lock) } < 0 {
                return Err(io::Error::last_os_error());
            }
            if frame.operation == Operation::Get {
                reply.range = Range {
                    kind: if lock.l_type == libc::F_RDLCK as i16 {
                        0
                    } else if lock.l_type == libc::F_WRLCK as i16 {
                        1
                    } else {
                        2
                    },
                    start: lock.l_start,
                    len: lock.l_len,
                };
                reply.host_pid = lock.l_pid;
            }
            Ok(())
        })();
        reply.errno = match result {
            Ok(()) => 0,
            Err(error) => error.raw_os_error().unwrap_or(libc::EIO),
        };
        self.state.lock().unwrap().active.remove(&frame.request);
        self.changed.notify_all();
        reply
    }
    fn cancel(&self, request: u64) {
        let mut state = self.state.lock().unwrap();
        loop {
            let Some(active) = state.active.get_mut(&request) else {
                return;
            };
            active.cancelled = true;
            if let Some(thread) = active.thread {
                unsafe {
                    libc::pthread_kill(thread as libc::pthread_t, libc::SIGUSR1);
                }
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
    fn close(&self, key: Option<[u8; 36]>) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let keys = state
            .files
            .values()
            .filter(|file| key.is_none_or(|key| key == file.key))
            .map(|file| file.key)
            .collect::<Vec<_>>();
        state.closing.extend(keys.iter().copied());
        loop {
            let mut waiting = false;
            for active in state
                .active
                .values_mut()
                .filter(|active| keys.contains(&active.key))
            {
                waiting = true;
                active.cancelled = true;
                if let Some(thread) = active.thread {
                    unsafe {
                        libc::pthread_kill(thread as libc::pthread_t, libc::SIGUSR1);
                    }
                }
            }
            if !waiting {
                break;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
        let mut failed = None;
        for key in &keys {
            if let Some(file) = state.files.values().find(|file| file.key == *key) {
                let mut lock = libc::flock {
                    l_start: 0,
                    l_len: 0,
                    l_pid: 0,
                    l_type: libc::F_UNLCK as i16,
                    l_whence: libc::SEEK_SET as i16,
                };
                if unsafe { libc::fcntl(file.fd.as_raw_fd(), libc::F_SETLK, &mut lock) } < 0 {
                    failed = Some(io::Error::last_os_error());
                }
            }
        }
        state.files.retain(|_, file| !keys.contains(&file.key));
        for key in keys {
            state.closing.remove(&key);
        }
        match failed {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
pub fn serve(name: &str, controller: ProcessIdentity) -> io::Result<()> {
    if !controller.is_live() {
        return Err(error(libc::ESRCH));
    }
    let name = CString::new(name).map_err(|_| error(libc::EINVAL))?;
    let port = Receive::new()?;
    if unsafe { mach_port_insert_right(task(), port.0, port.0, 20) } != 0 {
        return Err(error(libc::EIO));
    }
    let send_right = SendRight::adopt(port.0)?;
    if unsafe { bootstrap_register(bootstrap_port, name.as_ptr(), send_right.0) } != 0 {
        return Err(error(libc::EADDRINUSE));
    }
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = interrupt as usize;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    if unsafe { libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let worker = Arc::new(Worker {
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
    });
    loop {
        let incoming = match receive(port.0, Duration::from_millis(100)) {
            Ok(value) => value,
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ETIMEDOUT) | Some(libc::EINTR)
                ) =>
            {
                if controller.is_live() {
                    continue;
                }
                worker.close(None)?;
                break;
            }
            Err(error) => return Err(error),
        };
        let actor = authenticate_actor(incoming.pid, incoming.pid_version);
        let frame = incoming.frame;
        let reply_port = incoming.reply;
        if !actor.is_ok_and(|actor| actor == controller) {
            let mut reply = frame;
            reply.errno = libc::EPERM;
            send(reply_port, 0, reply, None, true)?;
            continue;
        }
        if matches!(
            frame.operation,
            Operation::Get | Operation::Set | Operation::Wait
        ) {
            let reserved = worker.reserve(frame);
            match reserved {
                Ok(file) => {
                    let active_worker = worker.clone();
                    if let Err(error) = std::thread::Builder::new()
                        .name("aim-posix-wait".into())
                        .spawn(move || {
                            let reply = active_worker.run(frame, file);
                            if let Err(error) = send(reply_port, 0, reply, None, true) {
                                eprintln!("native POSIX holder reply: {error}");
                            }
                        })
                    {
                        worker.state.lock().unwrap().active.remove(&frame.request);
                        worker.changed.notify_all();
                        let mut reply = frame;
                        reply.errno = error.raw_os_error().unwrap_or(libc::EIO);
                        send(reply_port, 0, reply, None, true)?;
                    }
                }
                Err(error) => {
                    let mut reply = frame;
                    reply.errno = error.raw_os_error().unwrap_or(libc::EIO);
                    send(reply_port, 0, reply, None, true)?;
                }
            }
            continue;
        }
        let mut reply = frame;
        let result = match frame.operation {
            Operation::Register => worker
                .register(frame, incoming.proof)
                .map(|ticket| reply.ticket = ticket),
            Operation::Cancel => {
                worker.cancel(frame.ticket);
                Ok(())
            }
            Operation::Close => worker.close(Some(frame.key)),
            Operation::Exit => worker.close(None),
            _ => Err(error(libc::EPROTO)),
        };
        reply.errno = match result {
            Ok(()) => 0,
            Err(error) => error.raw_os_error().unwrap_or(libc::EIO),
        };
        send(reply_port, 0, reply, None, true)?;
        if frame.operation == Operation::Exit {
            break;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_negative_ranges_and_protocol_are_checked() {
        assert_eq!(
            normalize(1, 0, 10, -6, 0, 0).unwrap(),
            Range {
                kind: 1,
                start: 4,
                len: 6
            }
        );
        assert_eq!(normalize(0, 1, -2, 0, 10, 0).unwrap().start, 8);
        assert_eq!(
            normalize(1, 0, 1, -2, 0, 0).unwrap_err().raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(
            normalize(1, 0, i64::MAX, 2, 0, 0)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EOVERFLOW)
        );
        let mut frame = Frame::new(Operation::Set, 12);
        frame.guest_pid = unsafe { libc::getpid() };
        let decoded = Frame::decode(&frame.encode()).unwrap();
        assert_eq!(decoded.request, 12);
        assert_eq!(decoded.guest_pid, frame.guest_pid);
        let mut bytes = frame.encode();
        bytes[74] = 1;
        assert!(Frame::decode(&bytes).is_err());
    }
    #[test]
    fn kernel_audit_generation_rejects_stale_pid_messages() {
        let pid = unsafe { libc::getpid() };
        let (_, version) = process_generation(pid).unwrap();
        assert_eq!(
            authenticate_actor(pid, version).unwrap(),
            ProcessIdentity::running(pid).unwrap()
        );
        assert_eq!(
            authenticate_actor(pid, version.wrapping_add(1))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        // mach_msg_audit_trailer_t has audit PID at40 and PID version at48;
        // these are the sender's queued kernel snapshot, not message payload.
        let mut trailer = [0u8; 52];
        put(&mut trailer, 40, pid as u32);
        put(&mut trailer, 48, version.wrapping_add(1));
        assert_eq!(
            authenticate_actor(word(&trailer, 40) as i32, word(&trailer, 48))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
    }
    struct Holder {
        child: Option<std::process::Child>,
        client: Client,
        pid: i32,
    }
    impl Holder {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let name = format!(
                "com.aim.posix-proof.{}.{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let current = std::env::current_exe().unwrap();
            let directory = current.parent().unwrap();
            let directory = if directory.file_name().is_some_and(|part| part == "deps") {
                directory.parent().unwrap()
            } else {
                directory
            };
            let actor = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
            let mut child = std::process::Command::new(directory.join("aim-lock-holder"))
                .args([
                    "--service-name",
                    &name,
                    "--controller-pid",
                    &actor.host_pid.to_string(),
                    "--controller-seconds",
                    &actor.start_seconds.to_string(),
                    "--controller-microseconds",
                    &actor.start_microseconds.to_string(),
                ])
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let client = loop {
                match Client::lookup(&name) {
                    Ok(client) => break client,
                    Err(error) => {
                        if let Some(status) = child.try_wait().unwrap() {
                            panic!("holder exited {status}: {error}");
                        }
                        if std::time::Instant::now() >= deadline {
                            child.kill().unwrap();
                            child.wait().unwrap();
                            panic!("holder lookup: {error}");
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            };
            let pid = child.id() as i32;
            Self {
                child: Some(child),
                client,
                pid,
            }
        }
        fn stop(mut self) {
            let reply = self
                .client
                .call(Frame::new(Operation::Exit, request()), None)
                .unwrap();
            assert_eq!(reply.errno, 0);
            assert!(self.child.take().unwrap().wait().unwrap().success());
        }
    }
    impl Drop for Holder {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                if child.try_wait().unwrap().is_none() {
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
        }
    }
    fn request() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
    #[test]
    fn attach_reply_rejects_another_actual_process_as_controller() {
        let helper = Holder::new();
        let expected = ProcessIdentity::running(helper.pid).unwrap();
        let endpoint =
            Endpoint::register(&format!("com.aim.posix-wrong-reply.{}", std::process::id()))
                .unwrap();
        let client =
            Client::lookup(&format!("com.aim.posix-wrong-reply.{}", std::process::id())).unwrap();
        let request = Frame::new(Operation::Attach, request());
        let pending = client.begin(request, None).unwrap();
        let message = endpoint.receive(Duration::from_secs(1)).unwrap();
        assert_eq!(
            message.actor,
            ProcessIdentity::running(unsafe { libc::getpid() }).unwrap()
        );
        let mut response = message.frame;
        response.guest_pid = unsafe { libc::getpid() };
        message.reply.send(response).unwrap();
        assert_eq!(
            pending
                .wait_authenticated(Duration::from_secs(1), expected)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        helper.stop();
    }
    fn register(holder: &Holder, file: &std::fs::File) -> u64 {
        use std::os::fd::AsFd;
        let mut frame = Frame::new(Operation::Register, request());
        frame.key = Identity::from_fd(file.as_fd()).unwrap().to_bytes();
        frame.access = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) } & libc::O_ACCMODE;
        let reply = holder.client.call(frame, Some(file.as_fd())).unwrap();
        assert_eq!(reply.errno, 0, "register fileport");
        reply.ticket
    }
    fn set(holder: &Holder, ticket: u64, kind: i16, start: i64, len: i64) -> Frame {
        let mut frame = Frame::new(Operation::Set, request());
        frame.ticket = ticket;
        frame.range = Range { kind, start, len };
        holder.client.call(frame, None).unwrap()
    }
    #[test]
    fn actual_holder_keeps_process_locks_across_private_pin_and_cancels_waits() {
        use std::os::fd::AsFd;
        let first = Holder::new();
        let second = Holder::new();
        let path = std::env::temp_dir().join(format!("aim-posix-holders-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::fs::write(&path, b"real lock owner").unwrap();
        let key = Identity::from_fd(file.as_fd()).unwrap();
        let a = register(&first, &file);
        let b = register(&second, &file);
        assert_eq!(set(&first, a, 1, 0, 8).errno, 0);
        let mut ofd = libc::flock {
            l_start: 0,
            l_len: 8,
            l_pid: 0,
            l_type: libc::F_WRLCK as i16,
            l_whence: libc::SEEK_SET as i16,
        };
        assert_eq!(unsafe { libc::fcntl(file.as_raw_fd(), 90, &mut ofd) }, -1);
        assert!(
            matches!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::EACCES) | Some(libc::EAGAIN)
            ),
            "real host OFD lock must conflict"
        );
        assert!(matches!(
            set(&second, b, 1, 0, 8).errno,
            libc::EACCES | libc::EAGAIN
        ));
        let duplicate = file.try_clone().unwrap();
        let mut byte = 0u8;
        assert_eq!(
            unsafe { libc::pread(duplicate.as_raw_fd(), (&mut byte as *mut u8).cast(), 1, 0) },
            1
        );
        assert_eq!(byte, b'r');
        drop(duplicate);
        assert!(
            matches!(set(&second, b, 1, 0, 8).errno, libc::EACCES | libc::EAGAIN),
            "private caller close must not release holder lock"
        );
        assert_eq!(register(&first, &file), a);
        assert!(
            matches!(set(&second, b, 1, 0, 8).errno, libc::EACCES | libc::EAGAIN),
            "redundant fileport must not create and close a same-inode fd"
        );
        let mut get = Frame::new(Operation::Get, request());
        get.ticket = b;
        get.range = Range {
            kind: 1,
            start: 0,
            len: 8,
        };
        let observed = second.client.call(get, None).unwrap();
        assert_eq!(observed.errno, 0);
        assert_eq!(observed.host_pid, first.pid);
        assert_eq!(
            observed.range,
            Range {
                kind: 1,
                start: 0,
                len: 8
            }
        );
        let mut wait = Frame::new(Operation::Wait, request());
        wait.ticket = b;
        wait.range = get.range;
        let pending = second.client.begin(wait, None).unwrap();
        assert_eq!(
            pending
                .wait(Duration::from_millis(30))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ETIMEDOUT)
        );
        let mut cancel = Frame::new(Operation::Cancel, request());
        cancel.ticket = wait.request;
        assert_eq!(second.client.call(cancel, None).unwrap().errno, 0);
        assert_eq!(
            pending.wait(Duration::from_secs(1)).unwrap().errno,
            libc::EINTR
        );
        assert!(matches!(
            set(&second, b, 1, 0, 8).errno,
            libc::EACCES | libc::EAGAIN
        ));
        let mut close = Frame::new(Operation::Close, request());
        close.key = key.to_bytes();
        assert_eq!(first.client.call(close, None).unwrap().errno, 0);
        assert_eq!(set(&second, b, 1, 0, 8).errno, 0);
        assert_eq!(set(&second, b, 2, 0, 0).errno, 0);
        let a = register(&first, &file);
        assert_eq!(set(&first, a, 1, 0, 8).errno, 0);
        assert_eq!(set(&second, b, 1, 8, 8).errno, 0);
        let mut waits = Frame::new(Operation::Wait, request());
        waits.ticket = a;
        waits.range = Range {
            kind: 1,
            start: 8,
            len: 8,
        };
        let pending = first.client.begin(waits, None).unwrap();
        assert_eq!(
            pending
                .wait(Duration::from_millis(30))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ETIMEDOUT)
        );
        let mut cycle = Frame::new(Operation::Wait, request());
        cycle.ticket = b;
        cycle.range = Range {
            kind: 1,
            start: 0,
            len: 8,
        };
        assert_eq!(
            second.client.call(cycle, None).unwrap().errno,
            libc::EDEADLK
        );
        assert_eq!(set(&second, b, 2, 0, 0).errno, 0);
        assert_eq!(pending.wait(Duration::from_secs(1)).unwrap().errno, 0);
        assert_eq!(set(&first, a, 2, 0, 0).errno, 0);
        assert_eq!(set(&first, a, 0, 0, 8).errno, 0);
        assert_eq!(set(&second, b, 0, 0, 8).errno, 0);
        assert!(matches!(
            set(&second, b, 1, 0, 8).errno,
            libc::EACCES | libc::EAGAIN
        ));
        drop(file);
        std::fs::remove_file(path).unwrap();
        first.stop();
        second.stop();
    }
}
