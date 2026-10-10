//! Event-driven PTY description capabilities. No-senders retires the actual OFD lease.
use super::{Endpoint, Pair, Readiness, Side};
use crate::{
    posix_control::{SendRight, authenticate_actor, task},
    private_fd::{self, PrivateFd},
    process_namespace::ProcessIdentity,
};
use std::{
    collections::HashMap,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    sync::Arc,
    time::Duration,
};
fn error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}
#[repr(C)]
#[derive(Default)]
struct Header {
    bits: u32,
    size: u32,
    remote: u32,
    local: u32,
    voucher: u32,
    id: i32,
}
unsafe extern "C" {
    fn mach_msg_destroy(message: *mut Header);
    fn mach_port_allocate(task: u32, right: u32, name: *mut u32) -> i32;
    fn mach_port_insert_right(task: u32, name: u32, right: u32, kind: u32) -> i32;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: i32) -> i32;
    fn mach_port_move_member(task: u32, name: u32, set: u32) -> i32;
    fn mach_port_request_notification(
        task: u32,
        name: u32,
        id: i32,
        sync: u32,
        notify: u32,
        kind: u32,
        previous: *mut u32,
    ) -> i32;
    fn mach_msg(
        message: *mut u8,
        options: i32,
        send_size: u32,
        receive_size: u32,
        name: u32,
        timeout: u32,
        notify: u32,
    ) -> i32;
}
struct Port {
    name: u32,
    right: u32,
}
impl Port {
    fn allocate(right: u32) -> io::Result<Self> {
        let mut name = 0;
        if unsafe { mach_port_allocate(task(), right, &mut name) } != 0 {
            return Err(error(libc::EIO));
        }
        Ok(Self { name, right })
    }
}
impl Drop for Port {
    fn drop(&mut self) {
        unsafe {
            mach_port_mod_refs(task(), self.name, self.right, -1);
        }
    }
}
fn pipe() -> io::Result<(PrivateFd, PrivateFd)> {
    let (_, mut fds) = private_fd::receive_allocations(|| {
        let mut fds = [-1; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let fds = fds
            .into_iter()
            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
            .collect::<Vec<_>>();
        for fd in &fds {
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(((), fds))
    })?;
    let write = fds.pop().unwrap();
    let read = fds.pop().unwrap();
    Ok((read, write))
}
struct Notification {
    read: PrivateFd,
    write: PrivateFd,
    armed: bool,
}
impl Notification {
    fn new() -> io::Result<Self> {
        let (read, write) = pipe()?;
        Ok(Self {
            read,
            write,
            armed: false,
        })
    }
    fn set(&mut self, armed: bool) -> io::Result<()> {
        if self.armed == armed {
            return Ok(());
        }
        if armed {
            if unsafe { libc::write(self.write.as_raw_fd(), b"H".as_ptr().cast(), 1) } != 1 {
                return Err(io::Error::last_os_error());
            }
        } else {
            let mut byte = 0u8;
            let count =
                unsafe { libc::read(self.read.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) };
            if count != 1 {
                return Err(io::Error::last_os_error());
            }
        }
        self.armed = armed;
        Ok(())
    }
}
struct Description {
    status: super::status::Status,
    _endpoint: Endpoint,
    flags: u64,
    pair: Arc<Pair>,
    notification: Option<Notification>,
    _receive: Port,
    _notify: Port,
}
/// Backing destruction precedes capability release, including failed imports.
pub struct Capability {
    status: super::status::ReadOnly,
    backing: PrivateFd,
    notification: Option<PrivateFd>,
    carrier: Option<PrivateFd>,
    right: SendRight,
}
impl Capability {
    pub fn observe(&self) -> io::Result<Observation> {
        Ok(Observation {
            data: self.backing.try_clone()?,
            notification: self
                .notification
                .as_ref()
                .map(PrivateFd::try_clone)
                .transpose()?,
            status: self.status.try_clone()?,
        })
    }
    pub fn status(&self) -> &super::status::ReadOnly {
        &self.status
    }
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.backing.as_fd()
    }
    pub fn notification(&self) -> Option<BorrowedFd<'_>> {
        self.notification.as_ref().map(AsFd::as_fd)
    }
    pub fn carrier(&self) -> Option<BorrowedFd<'_>> {
        self.carrier.as_ref().map(AsFd::as_fd)
    }
    pub fn right(&self) -> &SendRight {
        &self.right
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            status: self.status.try_clone()?,
            backing: self.backing.try_clone()?,
            notification: self
                .notification
                .as_ref()
                .map(PrivateFd::try_clone)
                .transpose()?,
            carrier: self
                .carrier
                .as_ref()
                .map(PrivateFd::try_clone)
                .transpose()?,
            right: self.right.try_clone()?,
        })
    }
}
pub struct Observation {
    data: PrivateFd,
    notification: Option<PrivateFd>,
    pub status: super::status::ReadOnly,
}
impl Observation {
    /// Restore weak resources from an authenticated kernel fork receipt.
    pub fn from_parts(data:PrivateFd,status:super::status::ReadOnly,notification:PrivateFd)->io::Result<Self>{
        if unsafe{libc::isatty(data.as_raw_fd())}!=1{return Err(error(libc::ENOTTY));}
        let mut stat:libc::stat=unsafe{std::mem::zeroed()};
        if unsafe{libc::fstat(notification.as_raw_fd(),&mut stat)}<0{return Err(io::Error::last_os_error());}
        if stat.st_mode&libc::S_IFMT!=libc::S_IFIFO{return Err(error(libc::EPROTO));}
        Ok(Self{data,status,notification:Some(notification)})
    }
    pub fn data(&self) -> BorrowedFd<'_> {
        self.data.as_fd()
    }
    pub fn notification(&self) -> Option<BorrowedFd<'_>> {
        self.notification.as_ref().map(AsFd::as_fd)
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            data: self.data.try_clone()?,
            notification: self
                .notification
                .as_ref()
                .map(PrivateFd::try_clone)
                .transpose()?,
            status: self.status.try_clone()?,
        })
    }
}
/// One port set receives every description's private no-senders notification.
struct Carrier {
    reader: PrivateFd,
    right: SendRight,
    tag: u64,
}
pub struct Registry {
    ports: Port,
    descriptions: HashMap<u32, Description>,
    carriers: HashMap<crate::pipe_identity::Key, Carrier>,
    bridge_port: Option<Port>,
    bridge: Option<super::carrier::Bridge>,
    next_carrier: u64,
}
impl Registry {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            ports: Port::allocate(3)?,
            descriptions: HashMap::new(),
            carriers: HashMap::new(),
            bridge_port: None,
            bridge: None,
            next_carrier: 1,
        })
    }
    pub fn grant(&mut self, pair: Arc<Pair>, side: Side) -> io::Result<Capability> {
        self.grant_flags(pair, side, 2)
    }
    fn grant_flags(&mut self, pair: Arc<Pair>, side: Side, flags: u64) -> io::Result<Capability> {
        let endpoint = pair.acquire(side)?;
        let receive = Port::allocate(1)?;
        let notify = Port::allocate(1)?;
        if unsafe { mach_port_insert_right(task(), receive.name, receive.name, 20) } != 0 {
            return Err(error(libc::EIO));
        }
        let right = SendRight::adopt(receive.name)?;
        if unsafe { mach_port_move_member(task(), notify.name, self.ports.name) } != 0 {
            return Err(error(libc::EIO));
        }
        let mut previous = 0;
        if unsafe {
            mach_port_request_notification(
                task(),
                receive.name,
                70,
                1,
                notify.name,
                21,
                &mut previous,
            )
        } != 0
        {
            return Err(error(libc::EIO));
        }
        if previous != 0 {
            drop(SendRight::adopt(previous)?);
            return Err(error(libc::EPROTO));
        }
        let notification = Some(Notification::new()?);
        let status = super::status::Status::new(&pair, receive.name, flags & !0x80000)?;
        let readonly = super::status::ReadOnly::adopt(PrivateFd::allocate(|| {
            let fd = unsafe { libc::fcntl(status.read_fd().as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        })?)?;
        let capability = Capability {
            status: readonly,
            backing: crate::private_fd::PrivateFd::allocate(|| {
                let fd = unsafe {
                    libc::fcntl(endpoint.descriptor().as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0)
                };
                if fd < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
                }
            })?,
            notification: notification
                .as_ref()
                .map(|notification| notification.read.try_clone())
                .transpose()?,
            carrier: None,
            right,
        };
        self.descriptions.insert(
            notify.name,
            Description {
                status,
                _endpoint: endpoint,
                flags: flags & !0x80000,
                pair: pair.clone(),
                notification,
                _receive: receive,
                _notify: notify,
            },
        );
        self.notify_pair(&pair)?;
        Ok(capability)
    }
    fn notify_pair(&mut self, pair: &Arc<Pair>) -> io::Result<()> {
        for description in self
            .descriptions
            .values_mut()
            .filter(|description| Arc::ptr_eq(&description.pair, pair))
        {
            description.status.publish(
                pair.peer_closed(description._endpoint.side)?,
                false,
                description.flags,
            );
            if let Some(notification) = &mut description.notification {
                notification.set(pair.peer_closed(description._endpoint.side)?)?;
            }
        }
        Ok(())
    }
    pub fn receive_event(&mut self, timeout: Duration) -> io::Result<()> {
        let mut bytes = [0u64; 64];
        let result = unsafe {
            mach_msg(
                bytes.as_mut_ptr().cast(),
                2 | 0x100 | 0x400,
                0,
                512,
                self.ports.name,
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
        let header = unsafe { &*bytes.as_ptr().cast::<Header>() };
        if header.id != 70 || header.bits & 0x80000000 != 0 || header.size != 36 {
            return Err(error(libc::EPROTO));
        }
        let description = self
            .descriptions
            .remove(&header.local)
            .ok_or_else(|| error(libc::EPROTO))?;
        let pair = description.pair.clone();
        description.status.publish(true, true, description.flags);
        drop(description);
        pair.refresh()?;
        self.notify_pair(&pair)
    }
    fn carrier(&mut self, right: &SendRight) -> io::Result<PrivateFd> {
        self.description(right)?;
        self.ensure_bridge()?;
        let (reader, writer) = pipe()?;
        let key =
            crate::pipe_identity::key(writer.as_raw_fd())?.ok_or_else(|| error(libc::EPROTO))?;
        if crate::pipe_identity::info(reader.as_raw_fd())? != (key.1, key.0) {
            return Err(error(libc::EPROTO));
        }
        let tag = self.next_carrier;
        self.next_carrier = self
            .next_carrier
            .checked_add(1)
            .ok_or_else(|| error(libc::EOVERFLOW))?;
        let retained = right.try_clone()?;
        self.bridge.as_ref().unwrap().register(reader.as_raw_fd(), tag)?;
        self.carriers.insert(key, Carrier { reader, right: retained, tag });
        Ok(writer)
    }
    fn ensure_bridge(&mut self) -> io::Result<()> {
        if self.bridge.is_none() {
            let receive = Port::allocate(1)?;
            if unsafe { mach_port_insert_right(task(), receive.name, receive.name, 20) } != 0
                || unsafe { mach_port_move_member(task(), receive.name, self.ports.name) } != 0
            {
                return Err(error(libc::EIO));
            }
            let sender = SendRight::adopt(receive.name)?;
            self.bridge = Some(super::carrier::Bridge::new(sender)?);
            self.bridge_port = Some(receive);
        }
        Ok(())
    }
    fn resolve_carrier(&self, right: &SendRight) -> io::Result<&SendRight> {
        if let Some(bridge) = &self.bridge {
            bridge.healthy()?;
        }
        let fd = PrivateFd::allocate(|| right.into_fd())?;
        let key = crate::pipe_identity::key(fd.as_raw_fd())?.ok_or_else(|| error(libc::EPERM))?;
        let carrier = self.carriers.get(&key).ok_or_else(|| error(libc::EPERM))?;
        if crate::pipe_identity::info(carrier.reader.as_raw_fd())? != (key.1, key.0) {
            return Err(error(libc::EPERM));
        }
        Ok(&carrier.right)
    }
    fn bridge_event(&mut self, header: &Header, words: &[u64]) -> io::Result<bool> {
        if self
            .bridge_port
            .as_ref()
            .is_none_or(|port| port.name != header.local)
        {
            return Ok(false);
        }
        let (tag, fd, eof) =
            super::carrier::event(header.id, words).ok_or_else(|| error(libc::EPROTO))?;
        let key = self
            .carriers
            .iter()
            .find(|(_, carrier)| carrier.tag == tag && carrier.reader.as_raw_fd() == fd)
            .map(|(key, _)| *key);
        let Some(key) = key else {
            return Ok(true);
        };
        if !eof {
            return Err(error(libc::EPROTO));
        }
        let carrier = self.carriers.remove(&key).unwrap();
        let mut byte = 0;
        let count = unsafe {
            libc::read(
                carrier.reader.as_raw_fd(),
                (&mut byte as *mut i32).cast(),
                1,
            )
        };
        if count != 0 {
            return Err(error(libc::EPROTO));
        }
        drop(carrier);
        Ok(true)
    }
    pub fn count(&self) -> usize {
        self.descriptions.len()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        if let Some(bridge) = &self.bridge {
            bridge.stopping();
        }
        self.bridge_port.take();
        if let Some(mut bridge) = self.bridge.take() {
            bridge.stop();
        }
    }
}
const PROTOCOL: i32 = 0x50545932;
const ALLOCATE: u32 = 1;
const OPEN_SLAVE: u32 = 2;
const READINESS: u32 = 3;
const READ: u32 = 4;
const WRITE: u32 = 5;
const GET_FLAGS: u32 = 6;
const SET_FLAGS: u32 = 7;
const SHUTDOWN: u32 = 8;
const EXPORT_CARRIER: u32 = 9;
const IMPORT_CARRIER: u32 = 10;
const CLASSIFY_CARRIER: u32 = 11;
const GET_LOCK:u32=12;
const SET_LOCK:u32=13;
const CLAIM_TTY:u32=14;
const GET_SESSION:u32=15;
const SET_FOREGROUND:u32=16;
const DETACH_TTY:u32=17;
const OPEN_SLAVE_NODE:u32=18;
const INHERIT_PROCESS:u32=19;
pub const CLASS: u32 = 7;
const MAX_DATA: usize = 4096;
#[derive(Default)]
struct Frame {
    op: u32,
    flags: u64,
    serial: u64,
    status: i32,
    length: u32,
    side: u32,
    data: Vec<u8>,
}
struct Message {
    frame: Frame,
    ports: Vec<SendRight>,
    actor: ProcessIdentity,
    reply: Option<Reply>,
}
struct Reply(std::cell::Cell<u32>);
impl Reply {
    fn send(&self, frame: &Frame, ports: &[&SendRight]) -> io::Result<()> {
        let port = self.0.get();
        if port == 0 {
            return Err(error(libc::EPROTO));
        }
        send(port, 0, true, frame, ports)?;
        self.0.set(0);
        Ok(())
    }
}
impl Drop for Reply {
    fn drop(&mut self) {
        if self.0.get() != 0 {
            unsafe {
                mach_port_mod_refs(task(), self.0.get(), 2, -1);
            }
        }
    }
}
fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn get(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn send(dest: u32, reply: u32, once: bool, frame: &Frame, ports: &[&SendRight]) -> io::Result<()> {
    if ports.len() > 4 || frame.data.len() > MAX_DATA {
        return Err(error(libc::EMSGSIZE));
    }
    let mut words = [0u64; 600];
    let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), 4800) };
    let start = 28 + ports.len() * 12;
    let payload_end = start + 40 + frame.data.len();
    let size = payload_end.next_multiple_of(4);
    put(
        bytes,
        0,
        0x80000000 | if once { 18 } else { 19 | (21 << 8) },
    );
    put(bytes, 4, size as u32);
    put(bytes, 8, dest);
    put(bytes, 12, reply);
    put(bytes, 20, PROTOCOL as u32);
    put(bytes, 24, ports.len() as u32);
    for (index, right) in ports.iter().enumerate() {
        let at = 28 + index * 12;
        put(bytes, at, right.name());
        bytes[at + 10] = 19;
        bytes[at + 11] = 0;
    }
    put(bytes, start, frame.op);
    bytes[start + 4..start + 12].copy_from_slice(&frame.flags.to_le_bytes());
    bytes[start + 12..start + 20].copy_from_slice(&frame.serial.to_le_bytes());
    put(bytes, start + 20, frame.status as u32);
    put(bytes, start + 24, frame.length);
    put(bytes, start + 28, frame.data.len() as u32);
    put(bytes, start + 32, frame.side);
    put(bytes, start + 36, 2);
    bytes[start + 40..payload_end].copy_from_slice(&frame.data);
    if unsafe {
        mach_msg(
            words.as_mut_ptr().cast(),
            1 | 0x10,
            size as u32,
            0,
            0,
            1000,
            0,
        )
    } != 0
    {
        return Err(error(libc::EIO));
    }
    Ok(())
}
fn receive(port: u32, timeout: Duration) -> io::Result<Message> {
    let mut words = [0u64; 600];
    let result = unsafe {
        mach_msg(
            words.as_mut_ptr().cast(),
            2 | 0x100 | 0x400 | (3 << 24),
            0,
            4800,
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
    decode(words)
}
impl Registry {
    fn description(&self, right: &SendRight) -> io::Result<&Description> {
        self.descriptions
            .values()
            .find(|description| description._receive.name == right.name())
            .ok_or_else(|| error(libc::EPERM))
    }
    fn existing_reply(&self, right: &SendRight, request: &Message) -> io::Result<()> {
        let description = self.description(right)?;
        let backing = SendRight::from_fd(description._endpoint.descriptor())?;
        let status = SendRight::from_fd(description.status.read_fd())?;
        let notification = SendRight::from_fd(
            description
                .notification
                .as_ref()
                .ok_or_else(|| error(libc::EPROTO))?
                .read
                .as_fd(),
        )?;
        request
            .reply
            .as_ref()
            .ok_or_else(|| error(libc::EPROTO))?
            .send(
                &Frame {
                    op: request.frame.op,
                    serial: request.frame.serial,
                    flags: description.flags,
                    side: u32::from(description._endpoint.side == Side::Slave),
                    ..Default::default()
                },
                &[&backing, right, &status, &notification],
            )
    }
    fn grant_reply(
        &mut self,
        pair: Arc<Pair>,
        side: Side,
        flags: u64,
        request: &Message,
    ) -> io::Result<()> {
        let capability = self.grant_flags(pair, side, flags)?;
        self.capability_reply(capability, side, flags, request)
    }
    fn capability_reply(&self, capability: Capability, side: Side, flags: u64, request: &Message) -> io::Result<()> {
        let backing = SendRight::from_fd(capability.descriptor())?;
        let notification = capability
            .notification()
            .map(SendRight::from_fd)
            .transpose()?;
        let status = SendRight::from_fd(capability.status().descriptor())?;
        let mut ports = vec![&backing, capability.right(), &status];
        if let Some(notification) = &notification {
            ports.push(notification);
        }
        let frame = Frame {
            op: request.frame.op,
            flags,
            serial: request.frame.serial,
            side: if side == Side::Master { 0 } else { 1 },
            ..Default::default()
        };
        request
            .reply
            .as_ref()
            .ok_or_else(|| error(libc::EPROTO))?
            .send(&frame, &ports)
    }
}
pub struct Server {
    registry: Registry,
    service: Port,
    _send: SendRight,
    runtime: std::path::PathBuf,
    controller: ProcessIdentity,
    stopping: bool,
    parent_controller: bool,
    sessions:super::session::Sessions,
}
#[derive(Clone, Debug)]
pub struct OwnerConfig {
    pub endpoint: String,
    pub process: ProcessIdentity,
}
impl OwnerConfig {
    pub fn write(&self, path: &std::path::Path) -> io::Result<()> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        if !self.process.is_live()
            || self.endpoint.is_empty()
            || self.endpoint.contains(['\t', '\n', '\0'])
        {
            return Err(error(libc::EINVAL));
        }
        let mut file = crate::private_fd::PrivateFile::allocate(|| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
        })?;
        writeln!(
            file,
            "AIMPTY2\t{}\t{}\t{}\t{}",
            self.process.host_pid,
            self.process.start_seconds,
            self.process.start_microseconds,
            self.endpoint
        )?;
        file.sync_all()
    }
    pub fn read(path: &std::path::Path, init: ProcessIdentity) -> io::Result<Self> {
        let record=Self::read_record(path)?;
        if record.process!=init||!init.is_live(){return Err(error(libc::EPERM));}
        Ok(record)
    }
    pub fn read_standalone(path:&std::path::Path)->io::Result<Self>{Self::read_record(path)}
    fn read_record(path:&std::path::Path)->io::Result<Self>{
        use std::{io::Read, os::unix::fs::OpenOptionsExt};
        let mut file = crate::private_fd::PrivateFile::allocate(|| {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
        })?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(error(libc::EPROTO));
        }
        if stat.st_uid!=unsafe{libc::geteuid()}||stat.st_mode&0o077!=0{return Err(error(libc::EPERM));}
        let mut text = String::new();
        file.by_ref().take(1025).read_to_string(&mut text)?;
        if text.len() > 1024 || !text.ends_with('\n') {
            return Err(error(libc::EPROTO));
        }
        let fields = text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
        if fields.len() != 5
            || fields[0] != "AIMPTY2"
            || fields[4].is_empty()
            || fields[4].contains('\0')
        {
            return Err(error(libc::EPROTO));
        }
        let process = ProcessIdentity {
            host_pid: fields[1].parse().map_err(|_| error(libc::EPROTO))?,
            start_seconds: fields[2].parse().map_err(|_| error(libc::EPROTO))?,
            start_microseconds: fields[3].parse().map_err(|_| error(libc::EPROTO))?,
        };
        if !process.is_live() {
            return Err(error(libc::EPERM));
        }
        Ok(Self {
            endpoint: fields[4].into(),
            process,
        })
    }
}
pub struct RunningServer {
    pub config: OwnerConfig,
    client: Client,
    worker: Option<std::thread::JoinHandle<io::Result<()>>>,
}
impl RunningServer {
    pub fn start(endpoint: &str, runtime: &std::path::Path) -> io::Result<Self> {
        Self::from_server(endpoint,Server::register(endpoint,runtime)?)
    }
    pub fn start_with_credentials(endpoint:&str,runtime:&std::path::Path,table:&std::path::Path)->io::Result<Self>{
        Self::from_server(endpoint,Server::register(endpoint,runtime)?.with_credentials(table)?)
    }
    fn from_server(endpoint:&str,mut server:Server)->io::Result<Self>{
        let process = ProcessIdentity::running(unsafe { libc::getpid() })?;
        let client = Client::lookup(endpoint, process)?;
        let worker = std::thread::Builder::new()
            .name("pty-owner".into())
            .spawn(move || server.run())?;
        Ok(Self {
            config: OwnerConfig {
                endpoint: endpoint.into(),
                process,
            },
            client,
            worker: Some(worker),
        })
    }
    pub fn shutdown(mut self) -> io::Result<()> {
        self.stop()
    }
    fn stop(&mut self) -> io::Result<()> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        if worker.is_finished() {
            return worker.join().map_err(|_| error(libc::EIO))?;
        }
        if let Err(failure) = self.client.shutdown() {
            self.worker = Some(worker);
            return Err(failure);
        }
        worker.join().map_err(|_| error(libc::EIO))?
    }
}
impl Drop for RunningServer {
    fn drop(&mut self) {
        if let Err(failure) = self.stop() {
            eprintln!("PTY owner shutdown: {failure}");
        }
    }
}
impl Server {
    pub fn register(name: &str, runtime: &std::path::Path) -> io::Result<Self> {
        let registry = Registry::new()?;
        let service = Port::allocate(1)?;
        if unsafe { mach_port_insert_right(task(), service.name, service.name, 20) } != 0 {
            return Err(error(libc::EIO));
        }
        let right = SendRight::adopt(service.name)?;
        crate::posix_control::register_service(name, &right)?;
        if unsafe { mach_port_move_member(task(), service.name, registry.ports.name) } != 0 {
            return Err(error(libc::EIO));
        }
        Ok(Self {
            registry,
            service,
            _send: right,
            runtime: runtime.into(),
            controller: ProcessIdentity::running(unsafe { libc::getpid() })?,
            stopping: false,
            parent_controller: false,
            sessions:super::session::Sessions::new(),
        })
    }
    pub fn with_parent_controller(mut self) -> io::Result<Self> {
        self.controller = ProcessIdentity::running(unsafe { libc::getppid() })?;
        self.registry.ensure_bridge()?;
        self.registry.bridge.as_ref().unwrap().watch_process(self.controller.host_pid)?;
        self.parent_controller = true;
        Ok(self)
    }
    pub fn with_credentials(mut self,table:&std::path::Path)->io::Result<Self>{
        let table=std::fs::canonicalize(table)?;
        if !std::fs::symlink_metadata(&table)?.is_dir(){return Err(error(libc::ENOTDIR));}
        self.sessions.set_table(table);Ok(self)
    }
    pub fn run(&mut self) -> io::Result<()> {
        while !self.stopping {
            match self.step(Duration::from_secs(3600)) {
                Ok(()) => {
                    if self.parent_controller && !self.controller.is_live()
                        && self.registry.descriptions.is_empty() && self.registry.carriers.is_empty() {
                        self.stopping = true;
                    }
                }
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::EINTR) | Some(libc::ETIMEDOUT)
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub fn step(&mut self, timeout: Duration) -> io::Result<()> {
        let mut words = [0u64; 600];
        let result = unsafe {
            mach_msg(
                words.as_mut_ptr().cast(),
                2 | 0x100 | 0x400 | (3 << 24),
                0,
                4800,
                self.registry.ports.name,
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
        let header = unsafe { &*words.as_ptr().cast::<Header>() };
        if self.registry.bridge_port.as_ref().is_some_and(|port|port.name==header.local){
            if let Some((0,pid,true))=super::carrier::event(header.id,&words){self.sessions.leader_exit(pid)?;return Ok(());}
        }
        if self.registry.bridge_event(header, &words)? {
            return Ok(());
        }
        if header.local != self.service.name {
            if header.id != 70 || header.bits & 0x80000000 != 0 || header.size != 36 {
                return Err(error(libc::EPROTO));
            }
            let description = self
                .registry
                .descriptions
                .remove(&header.local)
                .ok_or_else(|| error(libc::EPROTO))?;
            let pair = description.pair.clone();
            description.status.publish(true, true, description.flags);
            drop(description);
            pair.refresh()?;
            if pair.peer_closed(Side::Slave)?{self.sessions.master_closed(&pair)?;}
            return self.registry.notify_pair(&pair);
        }
        // The shared decoder also validates the real kernel audit generation.
        let request = decode(words)?;
        let result = self.handle(&request);
        if let Err(failure) = result {
            let frame = Frame {
                op: request.frame.op,
                serial: request.frame.serial,
                status: failure.raw_os_error().unwrap_or(libc::EIO),
                ..Default::default()
            };
            request
                .reply
                .as_ref()
                .ok_or_else(|| error(libc::EPROTO))?
                .send(&frame, &[])?;
        }
        Ok(())
    }
    fn grant_slave(&mut self, pair: Arc<Pair>, request: &Message) -> io::Result<()> {
        let flags = request.frame.flags;
        let capability = self.registry.grant_flags(pair.clone(), Side::Slave, flags)?;
        let previous = self.sessions.clone();
        let result = (|| {
            if flags & 0x100 == 0 && matches!(flags & 3, 0 | 2)
                && self.sessions.claim(&pair, request.actor, false, true, true)? {
                self.registry.ensure_bridge()?;
                self.registry.bridge.as_ref().unwrap().watch_process(request.actor.host_pid)?;
            }
            self.registry.capability_reply(capability, Side::Slave, flags, request)
        })();
        if result.is_err() { self.sessions = previous; }
        result
    }
    fn handle(&mut self, request: &Message) -> io::Result<()> {
        if request.frame.op==INHERIT_PROCESS{
            if !request.ports.is_empty()||request.frame.data.len()!=20{return Err(error(libc::EPROTO));}
            let bytes=&request.frame.data;
            let child=ProcessIdentity{host_pid:i32::from_le_bytes(bytes[..4].try_into().unwrap()),start_seconds:u64::from_le_bytes(bytes[4..12].try_into().unwrap()),start_microseconds:u64::from_le_bytes(bytes[12..20].try_into().unwrap())};
            self.sessions.inherit(request.actor,child)?;
            return request.reply.as_ref().ok_or_else(||error(libc::EPROTO))?.send(&Frame{op:INHERIT_PROCESS,serial:request.frame.serial,..Default::default()},&[]);
        }
        if request.frame.op==OPEN_SLAVE_NODE{
            if !request.ports.is_empty()||request.frame.data.contains(&0){return Err(error(libc::EPROTO));}
            let pair=self.registry.descriptions.values().find(|description|description.pair.slave_path()==request.frame.data).map(|description|description.pair.clone()).ok_or_else(||error(libc::ENOENT))?;
            if pair.slave_lock(){return Err(error(libc::EIO));}
            return self.grant_slave(pair, request);
        }
        if request.frame.op == SHUTDOWN {
            if request.actor != self.controller {
                return Err(error(libc::EPERM));
            }
            self.registry.descriptions.clear();
            self.stopping = true;
            return request
                .reply
                .as_ref()
                .ok_or_else(|| error(libc::EPROTO))?
                .send(
                    &Frame {
                        op: SHUTDOWN,
                        serial: request.frame.serial,
                        ..Default::default()
                    },
                    &[],
                );
        }
        if request.frame.op == ALLOCATE {
            if !request.ports.is_empty() {
                return Err(error(libc::EPROTO));
            }
            let pair = Arc::new(Pair::allocate(&self.runtime)?);
            return self
                .registry
                .grant_reply(pair, Side::Master, request.frame.flags, request);
        }
        if request.ports.len() != 1 {
            return Err(error(libc::EPROTO));
        }
        if request.frame.op == EXPORT_CARRIER {
            let descriptor = self.registry.carrier(&request.ports[0])?;
            let port = SendRight::from_fd(descriptor.as_fd())?;
            return request
                .reply
                .as_ref()
                .ok_or_else(|| error(libc::EPROTO))?
                .send(
                    &Frame {
                        op: EXPORT_CARRIER,
                        serial: request.frame.serial,
                        ..Default::default()
                    },
                    &[&port],
                );
        }
        if request.frame.op == IMPORT_CARRIER {
            let right = self
                .registry
                .resolve_carrier(&request.ports[0])?
                .try_clone()?;
            return self.registry.existing_reply(&right, request);
        }
        if request.frame.op == CLASSIFY_CARRIER {
            let class = match self.registry.resolve_carrier(&request.ports[0]) {
                Ok(_) => CLASS,
                Err(error) if error.raw_os_error() == Some(libc::EPERM) => 0,
                Err(error) => return Err(error),
            };
            return request
                .reply
                .as_ref()
                .ok_or_else(|| error(libc::EPROTO))?
                .send(
                    &Frame {
                        op: CLASSIFY_CARRIER,
                        serial: request.frame.serial,
                        flags: class as u64,
                        ..Default::default()
                    },
                    &[],
                );
        }
        let description = self.registry.description(&request.ports[0])?;
        let pair = description.pair.clone();
        let side = description._endpoint.side;
        let flags = description.flags;
        if request.frame.op == OPEN_SLAVE {
            if side != Side::Master {
                return Err(error(libc::EPERM));
            }
            if pair.slave_lock(){return Err(error(libc::EIO));}
            return self.grant_slave(pair, request);
        }
        let frame = match request.frame.op {
            GET_LOCK|SET_LOCK=>{
                if side!=Side::Master{return Err(error(libc::ENOTTY));}
                if request.frame.op==SET_LOCK{pair.set_slave_lock(request.frame.flags!=0);}
                Frame{flags:u64::from(pair.slave_lock()),..Default::default()}
            }
            CLAIM_TTY=>{
                if self.sessions.claim(&pair,request.actor,request.frame.flags==1,false,matches!(flags&3,0|2))?{
                    self.registry.ensure_bridge()?;self.registry.bridge.as_ref().unwrap().watch_process(request.actor.host_pid)?;
                }
                Frame::default()
            }
            GET_SESSION=>{let(sid,pgrp)=self.sessions.state(&pair,request.actor,side==Side::Master)?;Frame{flags:sid as u32 as u64|(pgrp as u32 as u64)<<32,..Default::default()}},
            SET_FOREGROUND=>{self.sessions.set_foreground(&pair,request.actor,request.frame.flags as i32)?;Frame::default()},
            DETACH_TTY=>{if side==Side::Master{return Err(error(libc::ENOTTY));}self.sessions.detach(&pair,request.actor)?;Frame::default()},
            READINESS => {
                let readiness = if side == Side::Master {
                    pair.master_readiness()?
                } else {
                    let hangup = pair.peer_closed(side)?;
                    Readiness {
                        readable: hangup
                            || super::readable(description._endpoint.descriptor().as_raw_fd())?,
                        hangup,
                    }
                };
                Frame {
                    flags: u64::from(readiness.readable) | u64::from(readiness.hangup) << 1,
                    ..Default::default()
                }
            }
            READ => {
                if !matches!(flags & 3, 0 | 2) {
                    return Err(error(libc::EBADF));
                }
                let mut data = vec![0; request.frame.length.min(MAX_DATA as u32) as usize];
                let count = if side == Side::Master {
                    pair.read_master(&mut data)?
                } else if !data.is_empty()
                    && pair.peer_closed(side)?
                    && !super::readable(description._endpoint.descriptor().as_raw_fd())?
                {
                    0
                } else {
                    let count = unsafe {
                        libc::read(
                            description._endpoint.descriptor().as_raw_fd(),
                            data.as_mut_ptr().cast(),
                            data.len(),
                        )
                    };
                    if count < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    count as usize
                };
                data.truncate(count);
                Frame {
                    data,
                    ..Default::default()
                }
            }
            WRITE => {
                if !matches!(flags & 3, 1 | 2) {
                    return Err(error(libc::EBADF));
                }
                if pair.peer_closed(side)? {
                    return Err(error(libc::EIO));
                }
                let fd = description._endpoint.descriptor().as_raw_fd();
                let count = unsafe {
                    libc::write(
                        fd,
                        request.frame.data.as_ptr().cast(),
                        request.frame.data.len(),
                    )
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                Frame {
                    length: count as u32,
                    ..Default::default()
                }
            }
            GET_FLAGS => Frame {
                flags,
                ..Default::default()
            },
            SET_FLAGS => {
                if request.frame.flags & 0x2000 != 0 {
                    return Err(error(libc::EOPNOTSUPP));
                }
                let value = (flags & !0xc00) | (request.frame.flags & 0xc00);
                let target = request.ports[0].name();
                self.registry
                    .descriptions
                    .values_mut()
                    .find(|description| description._receive.name == target)
                    .unwrap()
                    .flags = value;
                let description = self.registry.description(&request.ports[0])?;
                description.status.publish(
                    description.pair.peer_closed(description._endpoint.side)?,
                    false,
                    value,
                );
                Frame {
                    flags: value,
                    ..Default::default()
                }
            }
            _ => return Err(error(libc::EINVAL)),
        };
        let frame = Frame {
            op: request.frame.op,
            serial: request.frame.serial,
            ..frame
        };
        request
            .reply
            .as_ref()
            .ok_or_else(|| error(libc::EPROTO))?
            .send(&frame, &[])
    }
}
struct Received([u64; 600]);
impl Drop for Received {
    fn drop(&mut self) {
        unsafe {
            mach_msg_destroy(self.0.as_mut_ptr().cast());
        }
    }
}
fn decode(words: [u64; 600]) -> io::Result<Message> {
    let mut received = Received(words);
    let bytes =
        unsafe { std::slice::from_raw_parts_mut(received.0.as_mut_ptr().cast::<u8>(), 4800) };
    let size = get(bytes, 4) as usize;
    let count = get(bytes, 24) as usize;
    if get(bytes, 20) != PROTOCOL as u32
        || get(bytes, 0) & 0x80000000 == 0
        || count > 4
        || size > 4700
        || size < 28 + count * 12 + 40
    {
        return Err(error(libc::EPROTO));
    }
    let mut ports = Vec::new();
    for index in 0..count {
        let at = 28 + index * 12;
        if bytes[at + 11] != 0 || bytes[at + 10] != 17 {
            return Err(error(libc::EPROTO));
        }
        ports.push(SendRight::adopt(get(bytes, at))?);
        put(bytes, at, 0);
    }
    let start = 28 + count * 12;
    let length = get(bytes, start + 28) as usize;
    let payload_end = start + 40 + length;
    if length > MAX_DATA || size != payload_end.next_multiple_of(4) || get(bytes, start + 36) != 2
        || bytes[payload_end..size].iter().any(|byte| *byte != 0) {
        return Err(error(libc::EPROTO));
    }
    let trailer = size.next_multiple_of(4);
    let actor = authenticate_actor(get(bytes, trailer + 40) as i32, get(bytes, trailer + 48))?;
    let reply = get(bytes, 8);
    if reply != 0 && get(bytes, 0) & 0x1f != 18 {
        return Err(error(libc::EPROTO));
    }
    let result = Message {
        frame: Frame {
            op: get(bytes, start),
            flags: u64::from_le_bytes(bytes[start + 4..start + 12].try_into().unwrap()),
            serial: u64::from_le_bytes(bytes[start + 12..start + 20].try_into().unwrap()),
            status: get(bytes, start + 20) as i32,
            length: get(bytes, start + 24),
            side: get(bytes, start + 32),
            data: bytes[start + 40..payload_end].to_vec(),
        },
        ports,
        actor,
        reply: (reply != 0).then_some(Reply(std::cell::Cell::new(reply))),
    };
    put(bytes, 8, 0);
    Ok(result)
}

pub struct Client {
    service: SendRight,
    owner: ProcessIdentity,
    next: std::sync::atomic::AtomicU64,
}
pub struct RemoteEndpoint {
    pub capability: Capability,
    pub side: Side,
    pub flags: u64,
}
impl RemoteEndpoint {
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            capability: self.capability.try_clone()?,
            side: self.side,
            flags: self.flags,
        })
    }
}
impl Client {
    pub fn lookup(name: &str, owner: ProcessIdentity) -> io::Result<Self> {
        Ok(Self {
            service: crate::posix_control::Client::lookup(name)?.capability()?,
            owner,
            next: std::sync::atomic::AtomicU64::new(1),
        })
    }
    fn call(&self, mut frame: Frame, proof: Option<&SendRight>) -> io::Result<Message> {
        let serial = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        frame.serial = serial;
        let reply = Port::allocate(1)?;
        let ports = proof.into_iter().collect::<Vec<_>>();
        send(self.service.name(), reply.name, false, &frame, &ports)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let response = loop {
            let timeout = deadline.saturating_duration_since(std::time::Instant::now());
            if timeout.is_zero() {
                return Err(error(libc::ETIMEDOUT));
            }
            match receive(reply.name, timeout) {
                Ok(response) => break response,
                Err(error) if error.raw_os_error() == Some(libc::EINTR) => continue,
                Err(error) => return Err(error),
            }
        };
        if response.actor != self.owner
            || response.frame.serial != serial
            || response.frame.op != frame.op
        {
            return Err(error(libc::EPERM));
        }
        if response.frame.status != 0 {
            return Err(error(response.frame.status));
        }
        Ok(response)
    }
    fn import(&self, response: Message) -> io::Result<RemoteEndpoint> {
        let side = match response.frame.side {
            0 => Side::Master,
            1 => Side::Slave,
            _ => return Err(error(libc::EPROTO)),
        };
        if response.ports.len() != 4 {
            return Err(error(libc::EPROTO));
        }
        let mut ports = response.ports.into_iter();
        let backing = ports.next().unwrap();
        let right = ports.next().unwrap();
        let status = ports.next().unwrap();
        let notification = ports.next();
        let install = |right: SendRight| {
            PrivateFd::allocate(|| {
                let fd = right.into_fd()?;
                if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(fd)
            })
        };
        let backing = install(backing)?;
        let status = super::status::ReadOnly::adopt(install(status)?)?;
        let notification = notification.map(install).transpose()?;
        Ok(RemoteEndpoint {
            capability: Capability {
                carrier: None,
                status,
                backing,
                notification,
                right,
            },
            side,
            flags: response.frame.flags,
        })
    }
    pub fn allocate(&self, flags: u64) -> io::Result<RemoteEndpoint> {
        self.with_carrier(self.import(self.call(
            Frame {
                op: ALLOCATE,
                flags,
                ..Default::default()
            },
            None,
        )?)?)
    }
    pub fn open_slave(&self, master: &RemoteEndpoint, flags: u64) -> io::Result<RemoteEndpoint> {
        self.slave_from_right(master.capability.right(), flags)
    }
    pub fn open_slave_path(&self,path:&[u8],flags:u64)->io::Result<RemoteEndpoint>{
        self.with_carrier(self.import(self.call(Frame{op:OPEN_SLAVE_NODE,data:path.to_vec(),flags,..Default::default()},None)?)?)
    }
    pub fn slave_from_right(&self, master: &SendRight, flags: u64) -> io::Result<RemoteEndpoint> {
        self.with_carrier(self.import(self.call(
            Frame {
                op: OPEN_SLAVE,
                flags,
                ..Default::default()
            },
            Some(master),
        )?)?)
    }
    fn with_carrier(&self, mut endpoint: RemoteEndpoint) -> io::Result<RemoteEndpoint> {
        endpoint.capability.carrier = Some(self.export_carrier(&endpoint)?);
        Ok(endpoint)
    }
    pub fn export_carrier(&self, endpoint: &RemoteEndpoint) -> io::Result<PrivateFd> {
        let mut response = self.call(
            Frame {
                op: EXPORT_CARRIER,
                ..Default::default()
            },
            Some(endpoint.capability.right()),
        )?;
        if response.ports.len() != 1 {
            return Err(error(libc::EPROTO));
        }
        let port = response.ports.pop().unwrap();
        PrivateFd::allocate(|| port.into_fd())
    }
    pub fn import_carrier(&self, carrier: PrivateFd) -> io::Result<RemoteEndpoint> {
        let proof = SendRight::from_fd(carrier.as_fd())?;
        let mut endpoint = self.import(self.call(
            Frame {
                op: IMPORT_CARRIER,
                ..Default::default()
            },
            Some(&proof),
        )?)?;
        endpoint.capability.carrier = Some(carrier);
        Ok(endpoint)
    }
    pub fn classify(&self, fd: BorrowedFd<'_>) -> io::Result<u32> {
        let proof = SendRight::from_fd(fd)?;
        Ok(self
            .call(
                Frame {
                    op: CLASSIFY_CARRIER,
                    ..Default::default()
                },
                Some(&proof),
            )?
            .frame
            .flags as u32)
    }
    pub fn readiness(&self, endpoint: &RemoteEndpoint) -> io::Result<Readiness> {
        let response = self.call(
            Frame {
                op: READINESS,
                ..Default::default()
            },
            Some(endpoint.capability.right()),
        )?;
        Ok(Readiness {
            readable: response.frame.flags & 1 != 0,
            hangup: response.frame.flags & 2 != 0,
        })
    }
    pub fn read(&self, endpoint: &RemoteEndpoint, bytes: &mut [u8]) -> io::Result<usize> {
        let response = self.call(
            Frame {
                op: READ,
                length: bytes.len().min(MAX_DATA) as u32,
                ..Default::default()
            },
            Some(endpoint.capability.right()),
        )?;
        if response.frame.data.len() > bytes.len() {
            return Err(error(libc::EPROTO));
        }
        bytes[..response.frame.data.len()].copy_from_slice(&response.frame.data);
        Ok(response.frame.data.len())
    }
    pub fn write(&self, endpoint: &RemoteEndpoint, bytes: &[u8]) -> io::Result<usize> {
        let response = self.call(
            Frame {
                op: WRITE,
                data: bytes[..bytes.len().min(MAX_DATA)].to_vec(),
                ..Default::default()
            },
            Some(endpoint.capability.right()),
        )?;
        Ok(response.frame.length as usize)
    }
    pub fn shutdown(&self) -> io::Result<()> {
        self.call(
            Frame {
                op: SHUTDOWN,
                ..Default::default()
            },
            None,
        )
        .map(|_| ())
    }
    pub fn flags(&self, endpoint: &RemoteEndpoint) -> io::Result<u64> {
        Ok(self
            .call(
                Frame {
                    op: GET_FLAGS,
                    ..Default::default()
                },
                Some(endpoint.capability.right()),
            )?
            .frame
            .flags)
    }
    pub fn set_flags(&self, endpoint: &RemoteEndpoint, flags: u64) -> io::Result<()> {
        self.call(
            Frame {
                op: SET_FLAGS,
                flags,
                ..Default::default()
            },
            Some(endpoint.capability.right()),
        )
        .map(|_| ())
    }
    pub fn slave_lock(&self,endpoint:&RemoteEndpoint,set:Option<bool>)->io::Result<bool>{
        self.call(Frame{op:if set.is_some(){SET_LOCK}else{GET_LOCK},flags:u64::from(set.unwrap_or(false)),..Default::default()},Some(endpoint.capability.right())).map(|reply|reply.frame.flags!=0)
    }
    pub fn claim_tty(&self,endpoint:&RemoteEndpoint,force:bool)->io::Result<()> {
        self.call(Frame{op:CLAIM_TTY,flags:u64::from(force),..Default::default()},Some(endpoint.capability.right())).map(|_|())
    }
    pub fn tty_session(&self,endpoint:&RemoteEndpoint)->io::Result<(i32,i32)>{
        self.call(Frame{op:GET_SESSION,..Default::default()},Some(endpoint.capability.right())).map(|reply|(reply.frame.flags as u32 as i32,(reply.frame.flags>>32)as u32 as i32))
    }
    pub fn set_foreground(&self,endpoint:&RemoteEndpoint,pgrp:i32)->io::Result<()>{
        self.call(Frame{op:SET_FOREGROUND,flags:pgrp as u32 as u64,..Default::default()},Some(endpoint.capability.right())).map(|_|())
    }
    pub fn detach_tty(&self,endpoint:&RemoteEndpoint)->io::Result<()>{
        self.call(Frame{op:DETACH_TTY,..Default::default()},Some(endpoint.capability.right())).map(|_|())
    }
    pub fn inherit_child(&self,child:ProcessIdentity)->io::Result<()>{
        let mut data=child.host_pid.to_le_bytes().to_vec();data.extend(child.start_seconds.to_le_bytes());data.extend(child.start_microseconds.to_le_bytes());
        self.call(Frame{op:INHERIT_PROCESS,data,..Default::default()},None).map(|_|())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    fn runtime() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aim-pty-transport-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }
    #[test]
    fn mach_frame_preserves_unaligned_payload_and_rejects_nonzero_padding() {
        let receiver = Port::allocate(1).unwrap();
        assert_eq!(unsafe { mach_port_insert_right(task(), receiver.name, receiver.name, 20) }, 0);
        let destination = SendRight::adopt(receiver.name).unwrap();
        for length in [1, 2, 3, 5, 7, 268] {
            let data = vec![b'x'; length];
            send(destination.name(), 0, false, &Frame { data: data.clone(), ..Default::default() }, &[]).unwrap();
            let received = receive(receiver.name, Duration::from_secs(1)).unwrap();
            assert_eq!(received.frame.data, data);
        }
        send(destination.name(), 0, false, &Frame { data: vec![b'x'; 2], ..Default::default() }, &[]).unwrap();
        let mut words = [0u64; 600];
        assert_eq!(unsafe { mach_msg(words.as_mut_ptr().cast(), 2 | 0x100 | 0x400 | (3 << 24), 0, 4800, receiver.name, 1000, 0) }, 0);
        let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), 4800) };
        assert_eq!(get(bytes, 4), 72);
        bytes[70] = 1;
        assert_eq!(decode(words).err().unwrap().raw_os_error(), Some(libc::EPROTO));
    }
    #[test]
    #[ignore="real session child of virtual_session_detach_preserves_unread_output"]
    fn session_child(){
        use std::io::{BufRead,Write};
        assert!(unsafe{libc::setsid()}>0);
        unsafe{libc::alarm(8);}
        let mut mask:libc::sigset_t=unsafe{std::mem::zeroed()};
        unsafe{libc::sigemptyset(&mut mask);libc::sigaddset(&mut mask,libc::SIGHUP);libc::sigaddset(&mut mask,libc::SIGCONT);assert_eq!(libc::pthread_sigmask(libc::SIG_BLOCK,&mask,std::ptr::null_mut()),0);}
        let mut line=String::new();std::io::stdin().lock().read_line(&mut line).unwrap();
        let fields=line.trim().split(',').collect::<Vec<_>>();
        let owner=ProcessIdentity{host_pid:fields[1].parse().unwrap(),start_seconds:fields[2].parse().unwrap(),start_microseconds:fields[3].parse().unwrap()};
        let client=Client::lookup(fields[0],owner).unwrap();
        let receiver=Port::allocate(1).unwrap();
        assert_eq!(unsafe{mach_port_insert_right(task(),receiver.name,receiver.name,20)},0);
        let right=SendRight::adopt(receiver.name).unwrap();
        let name=format!("dev.aim.pty-session-child.{}",std::process::id());
        crate::posix_control::register_service(&name,&right).unwrap();
        println!("SESSION_RECEIVER {name}");std::io::stdout().flush().unwrap();
        let mut message=receive(receiver.name,Duration::from_secs(3)).unwrap();assert_eq!(message.actor,owner);assert_eq!(message.ports.len(),1);
        let master=message.ports.pop().unwrap();
        let slave=client.slave_from_right(&master,2).unwrap();
        let pid=unsafe{libc::getpid()};assert_eq!(client.tty_session(&slave).unwrap(),(pid,pid));
        assert_eq!(unsafe{libc::tcgetsid(slave.capability.descriptor().as_raw_fd())},-1,"Darwin controlling tty would revoke the keeper on exit");
        client.claim_tty(&slave,false).unwrap();client.set_foreground(&slave,pid).unwrap();
        assert_eq!(client.set_foreground(&slave,-1).unwrap_err().raw_os_error(),Some(libc::EINVAL));
        assert_eq!(client.write(&slave,&[b'x';268]).unwrap(),268);
        client.detach_tty(&slave).unwrap();
        let(mut first,mut second)=(0,0);assert_eq!(unsafe{libc::sigwait(&mask,&mut first)},0);assert_eq!(unsafe{libc::sigwait(&mask,&mut second)},0);
        assert!([first,second].contains(&libc::SIGHUP)&&[first,second].contains(&libc::SIGCONT));
        assert_eq!(client.tty_session(&slave).unwrap_err().raw_os_error(),Some(libc::ENOTTY));
        drop(slave);drop(master);println!("VIRTUAL_SESSION_CHILD_EXECUTED");
    }
    #[test]
    fn virtual_session_detach_preserves_unread_output(){
        if super::super::tests::isolated("pty_owner::transport::tests::virtual_session_detach_preserves_unread_output"){return;}
        use std::{io::{BufRead,BufReader,Write},process::{Command,Stdio}};
        let runtime=runtime();let pairs=runtime.join("pairs");fs::DirBuilder::new().mode(0o700).create(&pairs).unwrap();
        let table=runtime.join("identity/by-pid");fs::create_dir_all(&table).unwrap();
        let namespace=crate::mount_namespace::Namespace::open(&runtime,"session").unwrap();namespace.initialize(&format!("root\t/\t{}\n",pairs.display())).unwrap();
        let actual=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let name=format!("dev.aim.pty-session-owner.{}",actual.host_pid);
        let owner=RunningServer::start_with_credentials(&name,&pairs,&table).unwrap();let client=Client::lookup(&name,actual).unwrap();let master=client.allocate(2).unwrap();
        struct Child(std::process::Child);impl Drop for Child{fn drop(&mut self){if self.0.try_wait().unwrap().is_none(){self.0.kill().unwrap();}self.0.wait().unwrap();}}
        let mut command=Command::new(std::env::current_exe().unwrap());
        command.args(["--exact","pty_owner::transport::tests::session_child","--ignored","--nocapture"]).stdin(Stdio::piped()).stdout(Stdio::piped());
        use std::os::unix::process::CommandExt;
        // Block before exec so every libtest thread inherits the wait set.
        unsafe{command.pre_exec(||{let mut mask:libc::sigset_t=std::mem::zeroed();libc::sigemptyset(&mut mask);libc::sigaddset(&mut mask,libc::SIGHUP);libc::sigaddset(&mut mask,libc::SIGCONT);if libc::sigprocmask(libc::SIG_BLOCK,&mask,std::ptr::null_mut())<0{Err(io::Error::last_os_error())}else{Ok(())}});}
        let mut child=Child(command.spawn().unwrap());
        let process=ProcessIdentity::running(child.0.id()as i32).unwrap();
        crate::process_namespace::register_mount_namespace(&table,process,namespace.id()).unwrap();
        fs::write(table.join(process.host_pid.to_string()),"uid\t2000\ncap_effective\t0x0\n").unwrap();
        writeln!(child.0.stdin.as_mut().unwrap(),"{name},{},{},{}",actual.host_pid,actual.start_seconds,actual.start_microseconds).unwrap();
        let mut output=BufReader::new(child.0.stdout.take().unwrap());let mut line=String::new();
        let receiver=loop{line.clear();assert!(output.read_line(&mut line).unwrap()>0);if let Some(name)=line.trim().strip_prefix("SESSION_RECEIVER "){break name.to_string();}};
        let destination=crate::posix_control::Client::lookup(&receiver).unwrap().capability().unwrap();send(destination.name(),0,false,&Frame::default(),&[master.capability.right()]).unwrap();
        let mut marker=false;let mut transcript=String::new();loop{line.clear();if output.read_line(&mut line).unwrap()==0{break;}marker|=line.contains("VIRTUAL_SESSION_CHILD_EXECUTED");transcript.push_str(&line);}
        let status=child.0.wait().unwrap();assert!(status.success(),"session child {status}: {transcript}");assert!(marker);
        let mut notification=libc::pollfd{fd:master.capability.notification().unwrap().as_raw_fd(),events:libc::POLLIN,revents:0};assert_eq!(unsafe{libc::poll(&mut notification,1,2000)},1);
        let mut bytes=[0;268];assert_eq!(client.read(&master,&mut bytes).unwrap(),268);assert!(bytes.iter().all(|byte|*byte==b'x'));
        drop(master);owner.shutdown().unwrap();fs::remove_dir_all(runtime).unwrap();
    }
    #[test]
    fn final_description_event_wakes_master_without_query_and_preserves_bytes() {
        if super::super::tests::isolated(
            "pty_owner::transport::tests::final_description_event_wakes_master_without_query_and_preserves_bytes",
        ) {
            return;
        }
        let runtime = runtime();
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let name = format!("dev.aim.pty-test.{}", actual.host_pid);
        let mut server = Server::register(&name, &runtime).unwrap();
        let worker = std::thread::spawn(move || server.run().unwrap());
        let client = Client::lookup(&name, actual).unwrap();
        let master = client.allocate(2 | 0x800).unwrap();
        assert!(!client.readiness(&master).unwrap().hangup);
        let mut name = [0i8; 128];
        assert_eq!(unsafe { libc::ioctl(master.capability.descriptor().as_raw_fd(), 0x40807453u64 as libc::c_ulong, name.as_mut_ptr()) }, 0);
        let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_bytes();
        let slave = client.open_slave_path(path, 2 | 0x100 | 0x800).unwrap();
        let duplicate = slave.try_clone().unwrap();
        let bytes = [b'x'; 268];
        assert_eq!(client.write(&slave, &bytes).unwrap(), 268);
        drop(slave);
        assert!(!client.readiness(&master).unwrap().hangup);
        let pin = master.capability.try_clone().unwrap();
        let (fdtx, fdrx) = std::sync::mpsc::sync_channel(1);
        let blocked = std::thread::spawn(move || {
            let fd = pin.notification().unwrap().as_raw_fd();
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            fdtx.send(()).unwrap();
            assert_eq!(unsafe { libc::poll(&mut poll, 1, 2000) }, 1);
            assert!(poll.revents & libc::POLLIN != 0);
        });
        fdrx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(duplicate);
        blocked.join().unwrap();
        // No readiness RPC occurred between the last close and the actual FD wake.
        assert_eq!(
            client.readiness(&master).unwrap(),
            Readiness {
                readable: true,
                hangup: true
            }
        );
        let mut answer = [0; 268];
        assert_eq!(client.read(&master, &mut answer).unwrap(), 268);
        assert_eq!(answer, bytes);
        assert_eq!(
            client
                .read(&master, &mut answer)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EIO)
        );
        let slave = client.open_slave(&master, 2).unwrap();
        assert!(!client.readiness(&master).unwrap().hangup);
        let mut notification = libc::pollfd {
            fd: master.capability.notification().unwrap().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut notification, 1, 0) }, 0);
        client.set_flags(&master, 0).unwrap();
        assert_eq!(client.flags(&master).unwrap() & 0x800, 0);
        assert_ne!(
            unsafe { libc::fcntl(master.capability.descriptor().as_raw_fd(), libc::F_GETFL) }
                & libc::O_NONBLOCK,
            0,
            "native immutable status is separate from logical flags"
        );
        drop(slave);
        drop(master);
        client.shutdown().unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
        fs::remove_dir(runtime).unwrap();
    }
    #[test]
    fn master_final_close_wakes_slave_without_observer_retention() {
        if super::super::tests::isolated(
            "pty_owner::transport::tests::master_final_close_wakes_slave_without_observer_retention",
        ) {
            return;
        }
        let runtime = runtime();
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let name = format!("dev.aim.pty-slave-hup.{}", actual.host_pid);
        let owner = RunningServer::start(&name, &runtime).unwrap();
        let client = Client::lookup(&name, actual).unwrap();
        let master = client.allocate(2).unwrap();
        let slave = client.open_slave(&master, 2).unwrap();
        let weak = master.capability.observe().unwrap();
        assert!(!client.readiness(&slave).unwrap().hangup);
        drop(master);
        let mut notification = libc::pollfd {
            fd: slave.capability.notification().unwrap().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut notification, 1, 2000) }, 1);
        assert!(weak.status.retired());
        assert!(slave.capability.status().hangup());
        let readiness = client.readiness(&slave).unwrap();
        assert!(readiness.readable && readiness.hangup);
        assert_eq!(client.read(&slave, &mut [0; 1]).unwrap(), 0);
        assert_eq!(
            client.write(&slave, b"x").unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        drop(slave);
        drop(weak);
        owner.shutdown().unwrap();
        fs::remove_dir(runtime).unwrap();
    }
    #[test]
    fn opaque_carrier_and_weak_observer_keep_correct_endpoint_lifetime() {
        if super::super::tests::isolated(
            "pty_owner::transport::tests::opaque_carrier_and_weak_observer_keep_correct_endpoint_lifetime",
        ) {
            return;
        }
        let runtime = runtime();
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let name = format!("dev.aim.pty-carrier.{}", actual.host_pid);
        let owner = RunningServer::start(&name, &runtime).unwrap();
        let locator = runtime.join("owner");
        owner.config.write(&locator).unwrap();
        let config = OwnerConfig::read(&locator, actual).unwrap();
        let mut foreign = actual;
        foreign.start_microseconds = foreign.start_microseconds.wrapping_add(1);
        assert_eq!(
            OwnerConfig::read(&locator, foreign)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        let alias = runtime.join("owner-alias");
        std::os::unix::fs::symlink(&locator, &alias).unwrap();
        assert_eq!(
            OwnerConfig::read(&alias, actual)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ELOOP)
        );
        fs::remove_file(alias).unwrap();
        let client = Client::lookup(&config.endpoint, config.process).unwrap();
        let master = client.allocate(2).unwrap();
        let slave = client.open_slave(&master, 2).unwrap();
        let observer = slave.capability.observe().unwrap();
        let carrier = client.export_carrier(&slave).unwrap();
        assert_eq!(client.classify(carrier.as_fd()).unwrap(), CLASS);
        let impostor = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(client.classify(impostor.as_fd()).unwrap(), 0);
        assert_eq!(client.write(&slave, &[b'x'; 268]).unwrap(), 268);
        drop(slave);
        assert!(
            !client.readiness(&master).unwrap().hangup,
            "opaque FD keeps actual native OFD alive"
        );
        let restored = client.import_carrier(carrier).unwrap();
        assert_eq!(restored.side, Side::Slave);
        assert_eq!(client.flags(&restored).unwrap() & 3, 2);
        drop(restored);
        let mut ready = libc::pollfd {
            fd: master.capability.notification().unwrap().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut ready, 1, 2000) }, 1);
        assert!(
            observer.status.retired(),
            "weak watch does not retain SEND right or opaque carrier"
        );
        assert!(master.capability.status().hangup());
        let mut bytes = [0; 268];
        assert_eq!(client.read(&master, &mut bytes).unwrap(), 268);
        assert!(bytes.iter().all(|byte| *byte == b'x'));
        assert_eq!(
            client.read(&master, &mut bytes).unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        drop(observer);
        drop(master);
        owner.shutdown().unwrap();
        fs::remove_file(locator).unwrap();
        fs::remove_dir(runtime).unwrap();
    }
    #[test]
    fn queued_capability_right_keeps_ofd_lease_until_received_and_final_drop() {
        if super::super::tests::isolated(
            "pty_owner::transport::tests::queued_capability_right_keeps_ofd_lease_until_received_and_final_drop",
        ) {
            return;
        }
        let runtime = runtime();
        let pair = Arc::new(Pair::allocate(&runtime).unwrap());
        let mut registry = Registry::new().unwrap();
        let master = registry.grant(pair.clone(), Side::Master).unwrap();
        let slave = registry.grant(pair.clone(), Side::Slave).unwrap();
        let receiver = Port::allocate(1).unwrap();
        assert_eq!(
            unsafe { mach_port_insert_right(task(), receiver.name, receiver.name, 20) },
            0
        );
        let destination = SendRight::adopt(receiver.name).unwrap();
        send(
            destination.name(),
            0,
            false,
            &Frame::default(),
            &[slave.right()],
        )
        .unwrap();
        drop(slave);
        assert_eq!(
            registry
                .receive_event(Duration::from_millis(10))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ETIMEDOUT)
        );
        assert!(!pair.master_readiness().unwrap().hangup);
        let mut message = receive(receiver.name, Duration::from_secs(1)).unwrap();
        let duplicate = message.ports[0].try_clone().unwrap();
        message.ports.clear();
        assert_eq!(
            registry
                .receive_event(Duration::from_millis(10))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ETIMEDOUT)
        );
        drop(duplicate);
        registry.receive_event(Duration::from_secs(1)).unwrap();
        assert_eq!(registry.count(), 1);
        assert!(pair.master_readiness().unwrap().hangup);
        let mut poll = libc::pollfd {
            fd: master.notification().unwrap().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 1);
        drop(master);
        registry.receive_event(Duration::from_secs(1)).unwrap();
        assert_eq!(registry.count(), 0);
        drop(registry);
        drop(pair);
        fs::remove_dir(runtime).unwrap();
    }
    #[test]
    #[ignore = "owned crash child exercised by client_crash_retires_ofd_and_wakes_master"]
    fn crash_client() {
        use std::io::{BufRead, Write};
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).unwrap();
        let fields = line.trim().split(',').collect::<Vec<_>>();
        let owner = ProcessIdentity {
            host_pid: fields[1].parse().unwrap(),
            start_seconds: fields[2].parse().unwrap(),
            start_microseconds: fields[3].parse().unwrap(),
        };
        let client = Client::lookup(fields[0], owner).unwrap();
        let name = format!("dev.aim.pty-crash-transfer.{}", std::process::id());
        let receiver = Port::allocate(1).unwrap();
        assert_eq!(
            unsafe { mach_port_insert_right(task(), receiver.name, receiver.name, 20) },
            0
        );
        let destination = SendRight::adopt(receiver.name).unwrap();
        crate::posix_control::register_service(&name, &destination).unwrap();
        println!("PTY_CAPABILITY_RECEIVER_READY");
        std::io::stdout().flush().unwrap();
        let mut message = receive(receiver.name, Duration::from_secs(2)).unwrap();
        assert_eq!(message.actor, owner);
        assert_eq!(message.ports.len(), 1);
        let master = message.ports.pop().unwrap();
        let slave = client.slave_from_right(&master, 2).unwrap();
        assert_eq!(client.write(&slave, &[b'x'; 268]).unwrap(), 268);
        println!("PTY_CRASH_CLIENT_READY");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    #[test]
    fn client_crash_retires_ofd_and_wakes_master() {
        if super::super::tests::isolated(
            "pty_owner::transport::tests::client_crash_retires_ofd_and_wakes_master",
        ) {
            return;
        }
        use std::{
            io::{BufRead, Write},
            process::{Command, Stdio},
        };
        let runtime = runtime();
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let name = format!("dev.aim.pty-crash-test.{}", actual.host_pid);
        let mut server = Server::register(&name, &runtime).unwrap();
        let worker = std::thread::spawn(move || server.run().unwrap());
        let client = Client::lookup(&name, actual).unwrap();
        let master = client.allocate(2).unwrap();
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                if self.0.try_wait().ok().flatten().is_none() {
                    let _ = self.0.kill();
                }
                let _ = self.0.wait();
            }
        }
        let mut child = Child(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pty_owner::transport::tests::crash_client",
                    "--ignored",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        writeln!(
            child.0.stdin.take().unwrap(),
            "{name},{},{},{}",
            actual.host_pid,
            actual.start_seconds,
            actual.start_microseconds
        )
        .unwrap();
        let mut output = io::BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.trim() == "PTY_CAPABILITY_RECEIVER_READY" {
                break;
            }
        }
        let destination = crate::posix_control::Client::lookup(&format!(
            "dev.aim.pty-crash-transfer.{}",
            child.0.id()
        ))
        .unwrap()
        .capability()
        .unwrap();
        send(
            destination.name(),
            0,
            false,
            &Frame::default(),
            &[master.capability.right()],
        )
        .unwrap();
        loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.trim() == "PTY_CRASH_CLIENT_READY" {
                break;
            }
        }
        assert!(!client.readiness(&master).unwrap().hangup);
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        let mut poll = libc::pollfd {
            fd: master.capability.notification().unwrap().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut poll, 1, 2000) },
            1,
            "crash must cause unsolicited HUP notification"
        );
        assert_eq!(
            client.readiness(&master).unwrap(),
            Readiness {
                readable: true,
                hangup: true
            }
        );
        let mut bytes = [0; 268];
        assert_eq!(client.read(&master, &mut bytes).unwrap(), 268);
        assert!(bytes.iter().all(|byte| *byte == b'x'));
        drop(master);
        client.shutdown().unwrap();
        worker.join().unwrap();
        fs::remove_dir(runtime).unwrap();
    }
}
