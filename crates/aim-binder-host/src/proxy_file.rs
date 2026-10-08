//! Typed proxy file capability. Requests are serialized at the native owner;
//! each datagram carries its own reply channel so dup and fork share offsets
//! without sharing a userspace mutex.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::net::{UnixDatagram, UnixStream},
    },
    sync::{
        Arc, LazyLock, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
pub const CLASS: u32 = 1;
pub const MAX_DATA: usize = 0x7ffff000;

#[derive(Default)]
struct Registry {
    endpoints: HashMap<(u64, u64), Registered>,
    reaping: bool,
}
struct Registered {
    socket: UnixDatagram,
    owner: Weak<Owner>,
}
static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Default::default);

/// Darwin's kernel socket handles, not a socket type or userspace marker.
fn socket_identity(fd: RawFd) -> io::Result<(u64, u64)> {
    // SDK sys/proc_info.h: socket_fdinfo, psi.soi_so, psi.soi_proto.pri_un.
    let mut info = [0u8; 792];
    let n = unsafe {
        libc::proc_pidfdinfo(
            libc::getpid(),
            fd,
            3,
            info.as_mut_ptr().cast(),
            info.len() as i32,
        )
    };
    if n as usize != info.len() {
        return Err(io::Error::last_os_error());
    }
    let word = |at| u64::from_ne_bytes(info[at..at + 8].try_into().unwrap());
    Ok((word(160), word(264)))
}

fn register(client: &UnixDatagram, server: &UnixDatagram, owner: &Arc<Owner>) -> io::Result<()> {
    let identity = socket_identity(client.as_raw_fd())?;
    let peer = socket_identity(server.as_raw_fd())?;
    if identity.0 == 0 || identity.1 != peer.0 || peer.1 != identity.0 {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    let retained = server.try_clone()?;
    let mut registry = REGISTRY.lock().unwrap();
    registry.endpoints.insert(
        identity,
        Registered {
            socket: retained,
            owner: Arc::downgrade(owner),
        },
    );
    if !registry.reaping {
        let reaper = thread::Builder::new()
            .name("proxy-registry".into())
            .spawn(|| {
                loop {
                    thread::sleep(std::time::Duration::from_millis(100));
                    let mut registry = REGISTRY.lock().unwrap();
                    registry.endpoints.retain(|identity, registered| {
                        match socket_identity(registered.socket.as_raw_fd()) {
                            Ok(peer) if peer != (identity.1, identity.0) => return false,
                            Err(_) => return true,
                            _ => {}
                        }
                        if registered
                            .owner
                            .upgrade()
                            .is_none_or(|owner| owner.is_released())
                        {
                            let mut bytes = [0u8; 16];
                            for _ in 0..64 {
                                let Ok((_, Some(reply))) = receive_flags(
                                    registered.socket.as_raw_fd(),
                                    &mut bytes,
                                    libc::MSG_DONTWAIT,
                                ) else {
                                    break;
                                };
                                let mut reply = UnixStream::from(reply);
                                let _ = reply.write_all(&(-(libc::EPERM as i64)).to_le_bytes());
                            }
                        }
                        true
                    });
                    if registry.endpoints.is_empty() {
                        registry.reaping = false;
                        break;
                    }
                }
            });
        if let Err(error) = reaper {
            registry.endpoints.remove(&identity);
            return Err(error);
        }
        registry.reaping = true;
    }
    Ok(())
}

/// Only native `open` registers capabilities. A receipt's real endpoint must
/// match both registered kernel handles; retaining the server prevents peer
/// handle reuse without retaining the client and delaying its last close.
pub fn registered_class(fd: RawFd) -> u32 {
    registered_class_result(fd).unwrap_or(0)
}

pub fn registered_class_result(fd: RawFd) -> io::Result<u32> {
    let mut ty = 0i32;
    let mut length = std::mem::size_of::<i32>() as u32;
    // SAFETY: socket option into bounded local storage.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut ty as *mut i32).cast(),
            &mut length,
        )
    } < 0
    {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOTSOCK) {
            Ok(0)
        } else {
            Err(error)
        };
    }
    let identity = socket_identity(fd)?;
    let registry = REGISTRY.lock().unwrap();
    if let Some(registered) = registry.endpoints.get(&identity) {
        if socket_identity(registered.socket.as_raw_fd())? == (identity.1, identity.0) {
            return Ok(CLASS);
        }
    }
    Ok(0)
}
#[repr(u32)]
#[derive(Clone, Copy)]
pub enum Operation {
    Read = 1,
    Write = 2,
    Pread = 3,
    Pwrite = 4,
    Sync = 5,
    Size = 6,
    Seek = 7,
}
struct State {
    file: Option<File>,
    position: u64,
}
pub struct Owner {
    stopped: AtomicBool,
    revoked: AtomicBool,
    released: AtomicBool,
    active: Mutex<Option<UnixStream>>,
    state: Mutex<State>,
}
impl Owner {
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        if let Ok(mut state) = self.state.try_lock() {
            state.file.take();
        }
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }
    fn run(&self, op: u32, offset: i64, count: u32, data: &[u8]) -> io::Result<(i64, Vec<u8>)> {
        let mut state = self.state.lock().unwrap();
        if self.is_revoked() {
            state.file.take();
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        let result = (|| {
            let position = state.position;
            let file = state
                .file
                .as_mut()
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EPERM))?;
            match op {
                1 | 3 => {
                    let pos = if op == 1 {
                        position
                    } else {
                        offset
                            .try_into()
                            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?
                    };
                    use std::os::unix::fs::FileExt;
                    let mut bytes = Vec::new();
                    let length = count.min(MAX_DATA as u32) as usize;
                    bytes
                        .try_reserve_exact(length)
                        .map_err(|_| io::Error::from_raw_os_error(libc::ENOMEM))?;
                    bytes.resize(length, 0);
                    let n = file.read_at(&mut bytes, pos)?;
                    bytes.truncate(n);
                    if op == 1 {
                        state.position = pos + n as u64;
                    }
                    Ok((n as i64, bytes))
                }
                2 | 4 => {
                    let pos = if op == 2 {
                        position
                    } else {
                        offset
                            .try_into()
                            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?
                    };
                    use std::os::unix::fs::FileExt;
                    let n = file.write_at(data, pos)?;
                    if op == 2 {
                        state.position = pos + n as u64;
                    }
                    Ok((n as i64, Vec::new()))
                }
                5 => {
                    file.sync_all()?;
                    Ok((0, Vec::new()))
                }
                6 => Ok((file.metadata()?.len() as i64, Vec::new())),
                7 => {
                    let base = match count {
                        0 => 0,
                        1 => position,
                        2 => file.metadata()?.len(),
                        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
                    };
                    let pos = base as i128 + offset as i128;
                    if pos < 0 || pos > i64::MAX as i128 {
                        return Err(io::Error::from_raw_os_error(libc::EINVAL));
                    }
                    state.position = pos as u64;
                    Ok((pos as i64, Vec::new()))
                }
                _ => Err(io::Error::from_raw_os_error(libc::ENOSYS)),
            }
        })();
        if self.is_revoked() {
            state.file.take();
        }
        result
    }
}
pub struct Worker {
    owner: Arc<Owner>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn shutdown(&self) {
        self.owner.stopped.store(true, Ordering::SeqCst);
        self.owner.revoke();
        if let Some(active) = self.owner.active.lock().unwrap().as_ref() {
            let _ = active.shutdown(std::net::Shutdown::Both);
        }
    }
    pub fn detach(mut self) {
        self.shutdown();
        self.thread.take();
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(worker) = self.thread.take() {
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}
/// The caller retains the joining worker outside service-owner locks.
pub fn open(file: File) -> io::Result<(Arc<Owner>, UnixDatagram, Worker)> {
    let (server, client) = UnixDatagram::pair()?;
    server.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
    let owner = Arc::new(Owner {
        stopped: AtomicBool::new(false),
        revoked: AtomicBool::new(false),
        released: AtomicBool::new(false),
        active: Mutex::new(None),
        state: Mutex::new(State {
            file: Some(file),
            position: 0,
        }),
    });
    register(&client, &server, &owner)?;
    let state = owner.clone();
    let worker = thread::Builder::new()
        .name("proxy-file".into())
        .spawn(move || {
            let mut buffer = [0u8; 16];
            while !state.stopped.load(Ordering::SeqCst) {
                let (n, reply) = match receive(server.as_raw_fd(), &mut buffer) {
                    Ok((n, Some(reply))) => (n, reply),
                    Ok(_) => continue,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.kind() == io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    Err(_) => break,
                };
                if n != 16 {
                    let mut reply = UnixStream::from(reply);
                    let _ = reply.write_all(&(-(libc::EINVAL as i64)).to_le_bytes());
                    continue;
                }
                let op = u32::from_le_bytes(buffer[..4].try_into().unwrap());
                let offset = i64::from_le_bytes(buffer[4..12].try_into().unwrap());
                let count = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
                let mut reply = UnixStream::from(reply);
                if matches!(op, 1..=4) && count as usize > MAX_DATA {
                    let _ = reply.write_all(&(-(libc::EINVAL as i64)).to_le_bytes());
                    continue;
                }
                *state.active.lock().unwrap() = match reply.try_clone() {
                    Ok(retained) => Some(retained),
                    Err(error) => {
                        let _ = reply.write_all(
                            &(-(error.raw_os_error().unwrap_or(libc::EIO) as i64)).to_le_bytes(),
                        );
                        continue;
                    }
                };
                if state.stopped.load(Ordering::SeqCst) {
                    let _ = reply.write_all(&(-(libc::EPERM as i64)).to_le_bytes());
                    break;
                }
                let mut data = Vec::new();
                if matches!(op, 2 | 4) {
                    let length = count.min(MAX_DATA as u32) as usize;
                    if data.try_reserve_exact(length).is_err() {
                        let _ = reply.write_all(&(-(libc::ENOMEM as i64)).to_le_bytes());
                        continue;
                    }
                    data.resize(length, 0);
                    if reply.write_all(&0i64.to_le_bytes()).is_err() {
                        continue;
                    }
                    if reply.read_exact(&mut data).is_err() {
                        continue;
                    }
                }
                let result = state.run(op, offset, count, &data);
                let (value, bytes) = match result {
                    Ok(value) => value,
                    Err(e) => (-(e.raw_os_error().unwrap_or(libc::EIO) as i64), Vec::new()),
                };
                if reply.write_all(&value.to_le_bytes()).is_ok() {
                    let _ = reply.write_all(&bytes);
                }
            }
            state.active.lock().unwrap().take();
            state.revoke();
            state.released.store(true, Ordering::SeqCst);
        })?;
    Ok((
        owner.clone(),
        client,
        Worker {
            owner,
            thread: Some(worker),
        },
    ))
}
/// Returns a Darwin errno on failure; the Linux syscall owner translates it.
pub fn request(
    fd: RawFd,
    op: Operation,
    offset: i64,
    count: u32,
    data: &[u8],
) -> io::Result<(i64, Vec<u8>)> {
    let transfers_data = matches!(
        op,
        Operation::Read | Operation::Write | Operation::Pread | Operation::Pwrite
    );
    let writes_data = matches!(op, Operation::Write | Operation::Pwrite);
    if (transfers_data && count as usize > MAX_DATA)
        || (writes_data && data.len() != count as usize)
        || (!writes_data && !data.is_empty())
    {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let (mut receiver, sender) = UnixStream::pair()?;
    let mut bytes = (op as u32).to_le_bytes().to_vec();
    bytes.extend(offset.to_le_bytes());
    bytes.extend(count.to_le_bytes());
    // Retain the transferred reply endpoint until its response completes.
    send(fd, &bytes, sender.as_raw_fd())?;
    if matches!(op, Operation::Write | Operation::Pwrite) {
        let mut ready = [0u8; 8];
        receiver.read_exact(&mut ready)?;
        let ready = i64::from_le_bytes(ready);
        if ready < 0 {
            return Err(io::Error::from_raw_os_error((-ready) as i32));
        }
        if ready != 0 {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        receiver.write_all(data)?;
    }
    let mut status = [0; 8];
    receiver.read_exact(&mut status)?;
    let value = i64::from_le_bytes(status);
    if value < 0 {
        return Err(io::Error::from_raw_os_error((-value) as i32));
    }
    let mut result = Vec::new();
    if matches!(op, Operation::Read | Operation::Pread) {
        let length =
            usize::try_from(value).map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?;
        if length > count as usize {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        result
            .try_reserve_exact(length)
            .map_err(|_| io::Error::from_raw_os_error(libc::ENOMEM))?;
        result.resize(length, 0);
        receiver.read_exact(&mut result)?;
    }
    Ok((value, result))
}

fn send(fd: RawFd, bytes: &[u8], reply: RawFd) -> io::Result<()> {
    let mut control = [0usize; 4];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut _,
        iov_len: bytes.len(),
    };
    // SAFETY: aligned control buffer with one SCM_RIGHTS descriptor.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(4);
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(4);
        std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<i32>(), reply);
        let n = libc::sendmsg(fd, &msg, 0);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize != bytes.len() {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
    }
    Ok(())
}
fn receive(fd: RawFd, bytes: &mut [u8]) -> io::Result<(usize, Option<OwnedFd>)> {
    receive_flags(fd, bytes, 0)
}
fn receive_flags(fd: RawFd, bytes: &mut [u8], flags: i32) -> io::Result<(usize, Option<OwnedFd>)> {
    let mut control = [0usize; 4];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: bounded data/control buffers; transferred descriptors are owned.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&control) as u32;
        let n = libc::recvmsg(fd, &mut msg, flags);
        if n <= 0 {
            return Err(if n < 0 {
                io::Error::last_os_error()
            } else {
                io::Error::from_raw_os_error(libc::ECONNRESET)
            });
        }
        let mut descriptors = Vec::new();
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let length = (*c).cmsg_len.saturating_sub(libc::CMSG_LEN(0));
                for index in 0..length / 4 {
                    descriptors.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(
                        libc::CMSG_DATA(c).add(index as usize * 4).cast::<i32>(),
                    )));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        let reply = if descriptors.len() == 1
            && msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) == 0
        {
            descriptors.pop()
        } else {
            None
        };
        Ok((n as usize, reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_request_lengths_fail_before_sending_or_waiting() {
        // No descriptor is needed: validation precedes socket transport.
        for (op, count, bytes) in [
            (Operation::Write, 2, b"x".as_slice()),
            (Operation::Pwrite, 0, b"x".as_slice()),
            (Operation::Read, 1, b"x".as_slice()),
            (Operation::Sync, 0, b"x".as_slice()),
            (Operation::Read, MAX_DATA as u32 + 1, b"".as_slice()),
        ] {
            assert_eq!(
                request(-1, op, 0, count, bytes).unwrap_err().raw_os_error(),
                Some(libc::EINVAL)
            );
        }
    }
    #[test]
    fn registry_follows_actual_scm_receipt_and_rejects_unregistered_sockets() {
        let path = std::env::temp_dir().join(format!("aim-proxy-registry-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let (owner, client, worker) = open(file).unwrap();
        let identity = socket_identity(client.as_raw_fd()).unwrap();
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        assert_eq!(registered_class(sender.as_raw_fd()), 0);
        use std::os::fd::AsFd;
        assert!(crate::server::proxy_file_from_fd(sender.as_fd()).is_none());
        assert_eq!(registered_class(client.as_raw_fd()), CLASS);
        send(sender.as_raw_fd(), b"pass", client.as_raw_fd()).unwrap();
        let mut bytes = [0u8; 16];
        let (_, received) = receive(receiver.as_raw_fd(), &mut bytes).unwrap();
        let received = received.unwrap();
        assert_eq!(socket_identity(received.as_raw_fd()).unwrap(), identity);
        assert_eq!(registered_class(received.as_raw_fd()), CLASS);
        drop(client);
        assert_eq!(registered_class(received.as_raw_fd()), CLASS);
        assert!(!owner.is_released());
        owner.revoke();
        assert_eq!(registered_class(received.as_raw_fd()), CLASS);
        assert_eq!(
            request(received.as_raw_fd(), Operation::Read, 0, 1, &[])
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        drop(worker);
        assert_eq!(registered_class(received.as_raw_fd()), CLASS);
        assert_eq!(
            request(
                received.as_raw_fd(),
                Operation::Write,
                0,
                131072,
                &vec![b'x'; 131072]
            )
            .unwrap_err()
            .raw_os_error(),
            Some(libc::EPERM)
        );
        drop(received);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while REGISTRY.lock().unwrap().endpoints.contains_key(&identity) || !owner.is_released() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(1));
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn real_capability_dup_shares_offset_revoke_and_last_close_releases() {
        let path = std::env::temp_dir().join(format!("aim-proxy-file-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let (owner, client, worker) = open(file).unwrap();
        for metadata in [vec![0u8; 8], {
            let mut bytes = (Operation::Read as u32).to_le_bytes().to_vec();
            bytes.extend(0i64.to_le_bytes());
            bytes.extend((MAX_DATA as u32 + 1).to_le_bytes());
            bytes
        }] {
            let (mut response, reply) = UnixStream::pair().unwrap();
            response
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            send(client.as_raw_fd(), &metadata, reply.as_raw_fd()).unwrap();
            let mut status = [0u8; 8];
            response.read_exact(&mut status).unwrap();
            assert_eq!(i64::from_le_bytes(status), -(libc::EINVAL as i64));
        }
        let duplicate = client.try_clone().unwrap();
        assert_eq!(
            request(client.as_raw_fd(), Operation::Write, 0, 3, b"abc")
                .unwrap()
                .0,
            3
        );
        assert_eq!(
            request(duplicate.as_raw_fd(), Operation::Write, 0, 2, b"de")
                .unwrap()
                .0,
            2
        );
        assert_eq!(
            request(client.as_raw_fd(), Operation::Seek, 0, 0, &[])
                .unwrap()
                .0,
            0
        );
        assert_eq!(
            request(duplicate.as_raw_fd(), Operation::Read, 0, 2, &[])
                .unwrap()
                .1,
            b"ab"
        );
        assert_eq!(
            request(client.as_raw_fd(), Operation::Read, 0, 3, &[])
                .unwrap()
                .1,
            b"cde"
        );
        assert_eq!(
            request(client.as_raw_fd(), Operation::Pread, 1, 2, &[])
                .unwrap()
                .1,
            b"bc"
        );
        assert_eq!(
            request(client.as_raw_fd(), Operation::Seek, 0, 1, &[])
                .unwrap()
                .0,
            5
        );
        request(client.as_raw_fd(), Operation::Sync, 0, 0, &[]).unwrap();
        owner.revoke();
        for op in [
            Operation::Read,
            Operation::Write,
            Operation::Pread,
            Operation::Pwrite,
            Operation::Sync,
            Operation::Size,
            Operation::Seek,
        ] {
            assert_eq!(
                request(client.as_raw_fd(), op, 0, 0, &[])
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EPERM)
            );
        }
        drop(client);
        assert!(!owner.is_released());
        drop(duplicate);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !owner.is_released() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(1));
        }
        drop(worker);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcde");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn large_concurrent_duplicate_writes_remain_whole_offset_operations() {
        let path =
            std::env::temp_dir().join(format!("aim-proxy-file-large-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let (_owner, client, worker) = open(file).unwrap();
        let duplicate = client.try_clone().unwrap();
        let first = thread::spawn(move || {
            request(
                client.as_raw_fd(),
                Operation::Write,
                0,
                131072,
                &vec![b'A'; 131072],
            )
            .unwrap()
            .0
        });
        let second = thread::spawn(move || {
            request(
                duplicate.as_raw_fd(),
                Operation::Write,
                0,
                131072,
                &vec![b'B'; 131072],
            )
            .unwrap()
            .0
        });
        assert_eq!(first.join().unwrap(), 131072);
        assert_eq!(second.join().unwrap(), 131072);
        drop(worker);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 262144);
        let (first, second) = bytes.split_at(131072);
        assert!(first.iter().all(|b| *b == first[0]));
        assert!(second.iter().all(|b| *b == second[0]));
        assert_ne!(first[0], second[0]);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn revoke_and_detach_signal_without_waiting_for_inflight_owner_lock() {
        let path = std::env::temp_dir().join(format!("aim-proxy-file-stop-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let (owner, client, worker) = open(file).unwrap();
        let inflight = owner.state.lock().unwrap();
        owner.revoke();
        assert!(owner.is_revoked());
        worker.detach();
        drop(inflight);
        drop(client);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !owner.is_released() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(1));
        }
        std::fs::remove_file(path).unwrap();
    }
}
