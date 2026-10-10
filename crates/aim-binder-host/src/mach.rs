//! The Mach primitives of the transport: ports, one message format, the
//! bootstrap name, fileports and memory entries.
//!
//! Every message has the same shape: a complex header, port descriptors,
//! and a byte payload. A payload up to [`INLINE_MAX`] travels inline; a
//! larger one travels as an out-of-line (copy-on-write) region.

use std::ffi::CString;

pub type Port = u32;
pub type Kern = i32;

pub const NULL: Port = 0;

// Port rights and message dispositions.
const RIGHT_SEND: u32 = 0;
const RIGHT_RECEIVE: u32 = 1;
const RIGHT_PORT_SET: u32 = 3;
pub const MOVE_SEND: u32 = 17;
const MOVE_SEND_ONCE: u32 = 18;
pub const COPY_SEND: u32 = 19;
pub const MAKE_SEND: u32 = 20;
const MAKE_SEND_ONCE: u32 = 21;

const MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const SEND_MSG: i32 = 1;
const RCV_MSG: i32 = 2;
/// MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT).
const RCV_TRAILER_AUDIT: i32 = 3 << 24;

const PORT_DESCRIPTOR: u8 = 0;
const OOL_DESCRIPTOR: u8 = 1;
const VIRTUAL_COPY: u8 = 1;

/// MACH_SEND_* errors (the message was not sent).
const SEND_ERROR: Kern = 0x1000_0000;
const SEND_ERROR_MASK: Kern = 0x3fff_c000;

const HEADER: usize = 24;
const PORT_DESC: usize = 12;
const OOL_DESC: usize = 16;
/// Largest payload sent inline.
pub const INLINE_MAX: usize = 32 << 10;
/// Most ports one message carries (fds of a transaction, or fds to install).
pub const MAX_PORTS: usize = 256;
/// `mach_msg_audit_trailer_t`.
const TRAILER: usize = 52;
/// A receive buffer that holds any message of this format.
pub const BUFFER: usize = HEADER + 4 + MAX_PORTS * PORT_DESC + OOL_DESC + 4 + INLINE_MAX + TRAILER;

unsafe extern "C" {
    static mach_task_self_: Port;
    static bootstrap_port: Port;
    fn mach_msg(
        msg: *mut u8,
        option: i32,
        send_size: u32,
        rcv_size: u32,
        rcv_name: Port,
        timeout: u32,
        notify: Port,
    ) -> Kern;
    fn mach_port_allocate(task: Port, right: u32, name: *mut Port) -> Kern;
    fn mach_port_insert_right(task: Port, name: Port, right: Port, kind: u32) -> Kern;
    fn mach_port_deallocate(task: Port, name: Port) -> Kern;
    fn mach_port_mod_refs(task: Port, name: Port, right: u32, delta: i32) -> Kern;
    fn mach_port_move_member(task: Port, member: Port, after: Port) -> Kern;
    fn bootstrap_check_in(bp: Port, name: *const libc::c_char, sp: *mut Port) -> Kern;
    fn bootstrap_look_up(bp: Port, name: *const libc::c_char, sp: *mut Port) -> Kern;
    fn fileport_makeport(fd: libc::c_int, port: *mut Port) -> libc::c_int;
    fn fileport_makefd(port: Port) -> libc::c_int;
    fn mach_make_memory_entry_64(
        task: Port,
        size: *mut u64,
        offset: u64,
        permission: i32,
        handle: *mut Port,
        parent: Port,
    ) -> Kern;
    fn mach_vm_map(
        task: Port,
        address: *mut u64,
        size: u64,
        mask: u64,
        flags: i32,
        object: Port,
        offset: u64,
        copy: i32,
        cur: i32,
        max: i32,
        inheritance: u32,
    ) -> Kern;
    fn mach_vm_deallocate(task: Port, address: u64, size: u64) -> Kern;
}

fn task() -> Port {
    // SAFETY: a process-constant set by libSystem.
    unsafe { mach_task_self_ }
}

fn check(kr: Kern) -> Result<(), Kern> {
    if kr == 0 { Ok(()) } else { Err(kr) }
}

/// A new receive right; `with_send` also gives this task a send right.
pub fn new_port(with_send: bool) -> Result<Port, Kern> {
    let mut p = NULL;
    // SAFETY: allocating a right in our own task.
    unsafe {
        check(mach_port_allocate(task(), RIGHT_RECEIVE, &mut p))?;
        if with_send {
            check(mach_port_insert_right(task(), p, p, MAKE_SEND))?;
        }
    }
    Ok(p)
}

pub fn new_port_set() -> Result<Port, Kern> {
    let mut p = NULL;
    // SAFETY: allocating a port set in our own task.
    check(unsafe { mach_port_allocate(task(), RIGHT_PORT_SET, &mut p) })?;
    Ok(p)
}

pub fn move_member(member: Port, set: Port) -> Result<(), Kern> {
    // SAFETY: both names are rights of this task.
    check(unsafe { mach_port_move_member(task(), member, set) })
}

/// Drop one send (or send-once, or dead-name) reference.
pub fn release_send(p: Port) {
    if p != NULL {
        // SAFETY: releasing a right this task holds.
        unsafe { mach_port_deallocate(task(), p) };
    }
}

/// Destroy a receive right: senders see a dead name, a thread blocked
/// receiving on it wakes with an error.
pub fn destroy_receive(p: Port) {
    // SAFETY: dropping a receive right this task holds.
    unsafe { mach_port_mod_refs(task(), p, RIGHT_RECEIVE, -1) };
}

/// Drop the send right paired with a receive right from [`new_port`].
pub fn drop_own_send(p: Port) {
    // SAFETY: as above, for the send right.
    unsafe { mach_port_mod_refs(task(), p, RIGHT_SEND, -1) };
}

/// Register `name` with launchd; the receive right is the service port.
pub fn check_in(name: &str) -> Result<Port, Kern> {
    let c = CString::new(name).map_err(|_| 4)?;
    let mut p = NULL;
    // SAFETY: plain bootstrap call.
    check(unsafe { bootstrap_check_in(bootstrap_port, c.as_ptr(), &mut p) })?;
    Ok(p)
}

pub fn look_up(name: &str) -> Result<Port, Kern> {
    let c = CString::new(name).map_err(|_| 4)?;
    let mut p = NULL;
    // SAFETY: plain bootstrap call.
    check(unsafe { bootstrap_look_up(bootstrap_port, c.as_ptr(), &mut p) })?;
    Ok(p)
}

/// A send right for an open file description (its fd stays open).
pub fn fd_to_port(fd: i32) -> Option<Port> {
    let mut p = NULL;
    // SAFETY: fileport_makeport only reads the fd table.
    (unsafe { fileport_makeport(fd, &mut p) } == 0).then_some(p)
}

/// A private fd for a fileport (the right is kept). The ABI explicitly
/// installs guest descriptor flags when publishing a public recipient fd.
pub fn port_to_fd(p: Port) -> Option<i32> {
    // SAFETY: the fresh descriptor is exclusively owned until returned.
    unsafe {
        let fd = fileport_makefd(p);
        if fd < 0 { return None; }
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            let error = *libc::__error();
            libc::close(fd);
            *libc::__error() = error;
            return None;
        }
        Some(fd)
    }
}

const VM_FLAGS_ANYWHERE: i32 = 1;
const VM_FLAGS_FIXED_OVERWRITE: i32 = 0x4000;
const VM_PROT_READ: i32 = 1;
const VM_PROT_WRITE: i32 = 2;
const MAP_MEM_NAMED_CREATE: i32 = 0x2_0000;
const MAP_MEM_VM_SHARE: i32 = 0x40_0000;
const VM_INHERIT_NONE: u32 = 2;

/// Fresh shared memory of `len` bytes, mapped read-write here. Returns the
/// memory entry (a send right) and the local address.
pub fn new_shared_memory(len: u64) -> Result<(Port, u64), Kern> {
    let mut size = len;
    let mut entry = NULL;
    let mut addr = 0u64;
    // SAFETY: creating and mapping a named entry in our own task.
    unsafe {
        check(mach_make_memory_entry_64(
            task(),
            &mut size,
            0,
            MAP_MEM_NAMED_CREATE | VM_PROT_READ | VM_PROT_WRITE,
            &mut entry,
            NULL,
        ))?;
        if let Err(kr) = check(mach_vm_map(
            task(),
            &mut addr,
            len,
            0,
            VM_FLAGS_ANYWHERE,
            entry,
            0,
            0,
            VM_PROT_READ | VM_PROT_WRITE,
            VM_PROT_READ | VM_PROT_WRITE,
            VM_INHERIT_NONE,
        )) {
            release_send(entry);
            return Err(kr);
        }
    }
    Ok((entry, addr))
}

/// A read-only memory entry of `[addr, addr+len)`, which this task maps
/// shared: the same pages, for other tasks to map.
pub fn share_read_only(addr: u64, len: u64) -> Result<Port, Kern> {
    let (mut size, mut entry) = (len, NULL);
    // SAFETY: making an entry of our own mapping.
    check(unsafe {
        mach_make_memory_entry_64(
            task(),
            &mut size,
            addr,
            MAP_MEM_VM_SHARE | VM_PROT_READ,
            &mut entry,
            NULL,
        )
    })?;
    Ok(entry)
}

/// Map a memory entry read-only (current and maximum protection) over
/// `[addr, addr+len)`, replacing what is there.
pub fn map_read_only(entry: Port, addr: u64, len: u64) -> Result<(), Kern> {
    let mut at = addr;
    // SAFETY: the caller owns the target range.
    check(unsafe {
        mach_vm_map(
            task(),
            &mut at,
            len,
            0,
            VM_FLAGS_FIXED_OVERWRITE,
            entry,
            0,
            0,
            VM_PROT_READ,
            VM_PROT_READ,
            VM_INHERIT_NONE,
        )
    })
}

pub fn unmap(addr: u64, len: u64) {
    // SAFETY: the caller owns the range.
    unsafe { mach_vm_deallocate(task(), addr, len) };
}

/// A message of the transport's format.
#[derive(Default)]
pub struct Msg {
    pub id: i32,
    /// Rights to send, with their dispositions (`MOVE_SEND`, ...).
    pub ports: Vec<(Port, u32)>,
    pub data: Vec<u8>,
}

/// A received message. Port names are rights now held by this task.
pub struct Received {
    pub id: i32,
    /// The reply right (send-once), or NULL.
    pub reply: Port,
    /// The port the message arrived on.
    pub local: Port,
    pub ports: Vec<Port>,
    pub data: Vec<u8>,
    /// The sender's pid, from the audit trailer.
    pub pid: i32,
}

/// An 8-byte aligned message buffer.
pub struct Buffer(Vec<u64>);

impl Default for Buffer {
    fn default() -> Self {
        Self(vec![0; BUFFER.div_ceil(8)])
    }
}

impl Buffer {
    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: reinterpreting our own u64 storage as bytes.
        unsafe { std::slice::from_raw_parts_mut(self.0.as_mut_ptr().cast(), self.0.len() * 8) }
    }
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn get32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Lay out `msg` in `buf`; returns the send size.
fn encode(buf: &mut Buffer, msg: &Msg, remote: (Port, u32), local: (Port, u32)) -> u32 {
    assert!(msg.ports.len() <= MAX_PORTS);
    let b = buf.bytes();
    let ool = msg.data.len() > INLINE_MAX;
    let descriptors = msg.ports.len() + ool as usize;
    put32(b, 0, remote.1 | local.1 << 8 | MSGH_BITS_COMPLEX);
    put32(b, 8, remote.0);
    put32(b, 12, local.0);
    put32(b, 16, 0);
    put32(b, 20, msg.id as u32);
    put32(b, 24, descriptors as u32);
    let mut at = HEADER + 4;
    for &(port, disposition) in &msg.ports {
        put32(b, at, port);
        put32(b, at + 4, 0);
        put32(
            b,
            at + 8,
            disposition << 16 | (PORT_DESCRIPTOR as u32) << 24,
        );
        at += PORT_DESC;
    }
    if ool {
        b[at..at + 8].copy_from_slice(&(msg.data.as_ptr() as u64).to_le_bytes());
        // deallocate = 0, copy = virtual, type = OOL.
        put32(
            b,
            at + 8,
            (VIRTUAL_COPY as u32) << 8 | (OOL_DESCRIPTOR as u32) << 24,
        );
        put32(b, at + 12, msg.data.len() as u32);
        at += OOL_DESC;
        put32(b, at, 0);
        at += 4;
    } else {
        put32(b, at, msg.data.len() as u32);
        at += 4;
        b[at..at + msg.data.len()].copy_from_slice(&msg.data);
        at += msg.data.len().next_multiple_of(4);
    }
    put32(b, 4, at as u32);
    at as u32
}

fn decode(buf: &mut Buffer) -> Received {
    let b = buf.bytes();
    let size = get32(b, 4) as usize;
    let mut r = Received {
        id: get32(b, 20) as i32,
        reply: get32(b, 8),
        local: get32(b, 12),
        ports: Vec::new(),
        data: Vec::new(),
        pid: get32(b, size.next_multiple_of(4) + 12 + 8 + 20) as i32,
    };
    let mut at = HEADER;
    if get32(b, 0) & MSGH_BITS_COMPLEX != 0 {
        let count = get32(b, at);
        at += 4;
        for _ in 0..count {
            match b[at + 11] {
                PORT_DESCRIPTOR => {
                    r.ports.push(get32(b, at));
                    at += PORT_DESC;
                }
                _ => {
                    let addr = u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
                    let len = get32(b, at + 12) as usize;
                    // SAFETY: the kernel mapped `len` bytes at `addr` for us.
                    r.data = unsafe { std::slice::from_raw_parts(addr as *const u8, len) }.to_vec();
                    unmap(addr, len as u64);
                    at += OOL_DESC;
                }
            }
        }
    }
    if at + 4 <= size {
        let len = get32(b, at) as usize;
        if len > 0 {
            r.data = b[at + 4..at + 4 + len].to_vec();
        }
    }
    r
}

fn receive_options() -> i32 {
    RCV_MSG | RCV_TRAILER_AUDIT
}

/// Send `msg` to `dest` and wait for the answer on `reply_port` (one
/// `mach_msg`). The answer comes back through a send-once right.
pub fn call(buf: &mut Buffer, dest: Port, reply_port: Port, msg: &Msg) -> Result<Received, Kern> {
    let size = encode(buf, msg, (dest, COPY_SEND), (reply_port, MAKE_SEND_ONCE));
    // SAFETY: `buf` holds a well-formed message and has room for the answer.
    check(unsafe {
        mach_msg(
            buf.bytes().as_mut_ptr(),
            SEND_MSG | receive_options(),
            size,
            (BUFFER) as u32,
            reply_port,
            0,
            NULL,
        )
    })?;
    Ok(decode(buf))
}

/// A bounded request/reply exchange for native startup handshakes.
/// The caller owns the reply receive right and must destroy it on timeout.
pub fn call_bounded(buf: &mut Buffer, dest: Port, reply_port: Port, msg: &Msg,
    timeout_ms: u32) -> Result<Received, Kern> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
    let size = encode(buf, msg, (dest, COPY_SEND), (reply_port, MAKE_SEND_ONCE));
    // Send once. After an interrupted receive, wait for this same pending reply.
    check(unsafe {
        mach_msg(buf.bytes().as_mut_ptr(), SEND_MSG | 0x10,
            size, 0, NULL, timeout_ms, NULL)
    })?;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() { return Err(0x10004003); }
        let timeout = remaining.as_millis().max(1).min(u32::MAX as u128) as u32;
        let result = unsafe {
            mach_msg(buf.bytes().as_mut_ptr(), receive_options() | 0x100 | 0x400,
                0, BUFFER as u32, reply_port, timeout, NULL)
        };
        if result == 0x10004005 { continue; } // MACH_RCV_INTERRUPTED
        check(result)?;
        return Ok(decode(buf));
    }
}

/// Receive one message on `port` (or port set).
pub fn receive(buf: &mut Buffer, port: Port) -> Result<Received, Kern> {
    // SAFETY: `buf` has room for any message of this format.
    check(unsafe {
        mach_msg(
            buf.bytes().as_mut_ptr(),
            receive_options(),
            0,
            BUFFER as u32,
            port,
            0,
            NULL,
        )
    })?;
    Ok(decode(buf))
}

/// [`reply`], then wait for the next message on `port`, in one `mach_msg`.
pub fn reply_and_receive(
    buf: &mut Buffer,
    to: Port,
    msg: &Msg,
    port: Port,
) -> Result<Received, Kern> {
    let size = encode(buf, msg, (to, MOVE_SEND_ONCE), (NULL, 0));
    // SAFETY: `buf` holds a well-formed message and has room for any other.
    let kr = unsafe {
        mach_msg(
            buf.bytes().as_mut_ptr(),
            SEND_MSG | receive_options(),
            size,
            BUFFER as u32,
            port,
            0,
            NULL,
        )
    };
    match kr {
        0 => Ok(decode(buf)),
        // The requester is gone: the send failed and nothing was received.
        kr if kr & SEND_ERROR_MASK == SEND_ERROR => {
            release_send(to);
            receive(buf, port)
        }
        kr => Err(kr),
    }
}

/// Answer a request through its send-once right. A dead requester is not an
/// error.
pub fn reply(buf: &mut Buffer, to: Port, msg: &Msg) {
    let size = encode(buf, msg, (to, MOVE_SEND_ONCE), (NULL, 0));
    // SAFETY: `buf` holds a well-formed message.
    let kr = unsafe { mach_msg(buf.bytes().as_mut_ptr(), SEND_MSG, size, 0, NULL, 0, NULL) };
    if kr != 0 {
        release_send(to);
    }
}

/// Bounded send-once reply. COPY_SEND attachments remain owned by the caller
/// through success or failure; the caller releases its reply right on error.
pub fn reply_bounded(buf:&mut Buffer,to:Port,msg:&Msg,timeout_ms:u32)->Result<(),i32>{
    let size=encode(buf,msg,(to,MOVE_SEND_ONCE),(NULL,0));
    let result=unsafe{mach_msg(buf.bytes().as_mut_ptr(),SEND_MSG|0x10,size,0,NULL,timeout_ms,NULL)};
    if result==0{Ok(())}else{Err(result)}
}
