use super::*;
use std::{
    collections::{BTreeMap, VecDeque},
    os::unix::net::UnixListener,
    sync::{Arc, Condvar, Mutex, mpsc},
};
const ENOTCONN: i32 = 107;
const EPROTO: i32 = 71;
struct Pending {
    reply: mpsc::Sender<Result<Vec<u8>, Errno>>,
    delivered: bool,
    packet: Vec<u8>,
}
struct Device {
    signal: Arc<UnixStream>,
    redirect: Option<(SessionKey, u64, Arc<UnixStream>)>,
}
struct Description {
    node: u64,
    fh: u64,
    flags: Mutex<u32>,
    directory: bool,
    uid: u32,
    gid: u32,
    pid: u32,
    relative: String,
    guest: String,
    open_flags: u32,
    policy: MountPolicy,
    owner: std::sync::Weak<Owner>,
    offset: Mutex<u64>,
}
struct State {
    mounted: bool,
    initialized: Option<Result<(), Errno>>,
    init_reply: Option<Vec<u8>>,
    max_write: u32,
    aborted: bool,
    next: u64,
    queue: VecDeque<Vec<u8>>,
    pending: BTreeMap<u64, Pending>,
    devices: BTreeMap<u64, Device>,
    files: BTreeMap<u64, Arc<Description>>,
    notifications: VecDeque<(i32, Vec<u8>)>,
    inode_locks: BTreeMap<u64, Arc<Mutex<()>>>,
    retiring: BTreeMap<u64, usize>,
    seen: bool,
}
struct Owner {
    key: SessionKey,
    state: Mutex<State>,
    ready: Condvar,
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Errno> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(EINVAL)?
            .try_into()
            .unwrap(),
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Errno> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(EINVAL)?
            .try_into()
            .unwrap(),
    ))
}
fn request_packet(
    op: u32,
    node: u64,
    unique: u64,
    payload: &[u8],
    uid: u32,
    gid: u32,
    pid: u32,
) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(((HEADER_SIZE + payload.len()) as u32).to_le_bytes());
    data.extend(op.to_le_bytes());
    data.extend(unique.to_le_bytes());
    data.extend(node.to_le_bytes());
    data.extend(uid.to_le_bytes());
    data.extend(gid.to_le_bytes());
    data.extend(pid.to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend(payload);
    data
}
impl Owner {
    fn signal(state: &State) {
        for device in state.devices.values() {
            let _ = unsafe {
                libc::send(
                    device.signal.as_raw_fd(),
                    [1u8].as_ptr().cast(),
                    1,
                    libc::MSG_DONTWAIT,
                )
            };
        }
    }
    fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        state.aborted = true;
        state.queue.clear();
        for (_, pending) in std::mem::take(&mut state.pending) {
            let _ = pending.reply.send(Err(ENOTCONN));
        }
        Self::signal(&state);
        self.ready.notify_all();
    }
    fn enqueue(
        &self,
        op: u32,
        node: u64,
        payload: &[u8],
        uid: u32,
        gid: u32,
        pid: u32,
        no_reply: bool,
    ) -> Result<(u64, Option<mpsc::Receiver<Result<Vec<u8>, Errno>>>), Errno> {
        let mut state = self.state.lock().unwrap();
        if state.aborted {
            return Err(ENOTCONN);
        }
        if !state.mounted {
            return Err(ENODEV);
        }
        let unique = state.next;
        state.next = state.next.checked_add(1).ok_or(EIO)?;
        let packet = request_packet(op, node, unique, payload, uid, gid, pid);
        let channel = if no_reply {
            None
        } else {
            let (send, receive) = mpsc::channel();
            state.pending.insert(
                unique,
                Pending {
                    reply: send,
                    delivered: false,
                    packet: packet.clone(),
                },
            );
            Some(receive)
        };
        state.queue.push_back(packet);
        Self::signal(&state);
        self.ready.notify_all();
        Ok((unique, channel))
    }
    fn mount(self: &Arc<Self>) -> Result<(), Errno> {
        {
            let mut state = self.state.lock().unwrap();
            if state.aborted {
                return Err(ENOTCONN);
            }
            if state.mounted {
                return Ok(());
            }
            state.mounted = true;
        }
        let mut init = Vec::new();
        init.extend(7u32.to_le_bytes());
        init.extend(39u32.to_le_bytes());
        init.extend((128 * 1024u32).to_le_bytes());
        init.extend((1u32 | 32).to_le_bytes());
        let (_, receive) = self.enqueue(
            INIT,
            0,
            &init,
            0,
            0,
            unsafe { libc::getpid() } as u32,
            false,
        )?;
        let owner = self.clone();
        std::thread::spawn(move || {
            let result = receive
                .unwrap()
                .recv()
                .unwrap_or(Err(ENOTCONN))
                .and_then(|body| {
                    if body.len() < 24
                        || u32_at(&body, 0)? != 7
                        || u32_at(&body, 4)? < 6
                        || u32_at(&body, 20)? == 0
                    {
                        return Err(EPROTO);
                    }
                    Ok(body)
                });
            let mut state = owner.state.lock().unwrap();
            match result {
                Ok(body) => {
                    state.max_write = u32_at(&body, 20)
                        .unwrap()
                        .min((MAX_MESSAGE - HEADER_SIZE - 40) as u32);
                    state.init_reply = Some(body);
                    state.initialized = Some(Ok(()));
                }
                Err(error) => state.initialized = Some(Err(error)),
            };
            owner.ready.notify_all();
        });
        Ok(())
    }
    fn submit(&self, body: &[u8], no_reply: bool, peer: Option<RawFd>) -> Result<Vec<u8>, Errno> {
        if body.len() < 24 {
            return Err(EINVAL);
        }
        let op = u32_at(body, 0)?;
        let node = u64_at(body, 4)?;
        let uid = u32_at(body, 12)?;
        let gid = u32_at(body, 16)?;
        let pid = u32_at(body, 20)?;
        if body.len() + HEADER_SIZE > MAX_MESSAGE {
            return Err(EINVAL);
        }
        if no_reply && !matches!(op, FORGET | BATCH_FORGET) {
            return Err(EINVAL);
        }
        if op != INIT {
            let mut state = self.state.lock().unwrap();
            while state.initialized.is_none() && !state.aborted {
                state = self
                    .ready
                    .wait_timeout(state, Duration::from_millis(50))
                    .unwrap()
                    .0;
                if peer.is_some_and(peer_closed) {
                    return Err(errno::EINTR);
                }
            }
            if state.aborted {
                return Err(ENOTCONN);
            }
            state.initialized.unwrap()?;
        }
        let (unique, receive) = self.enqueue(op, node, &body[24..], uid, gid, pid, no_reply)?;
        let Some(receive) = receive else {
            return Ok(Vec::new());
        };
        loop {
            match receive.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ENOTCONN),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if peer.is_some_and(peer_closed) {
                        let mut state = self.state.lock().unwrap();
                        let sent = state
                            .pending
                            .remove(&unique)
                            .is_some_and(|pending| pending.delivered);
                        state
                            .queue
                            .retain(|packet| u64_at(packet, 8).ok() != Some(unique));
                        drop(state);
                        if sent {
                            let _ = self.enqueue(36, 0, &unique.to_le_bytes(), uid, gid, pid, true);
                        }
                        return Err(errno::EINTR);
                    }
                }
            }
        }
    }
    fn reply_device(&self, body: &[u8]) -> Result<Vec<u8>, Errno> {
        if body.len() < OUT_HEADER_SIZE || u32_at(body, 0)? as usize != body.len() {
            return Err(EINVAL);
        }
        let error = u32_at(body, 4)? as i32;
        let unique = u64_at(body, 8)?;
        if unique == 0 {
            if !(1..=6).contains(&error) {
                return Err(EINVAL);
            }
            let notification = &body[16..];
            let valid = match error {
                1 => notification.len() == 8,
                2 => notification.len() == 24,
                3 => {
                    notification.len() >= 16
                        && u32_at(notification, 8)? as usize == notification.len() - 16
                }
                4 => {
                    notification.len() >= 24
                        && u32_at(notification, 16)? as usize == notification.len() - 24
                }
                5 => notification.len() == 32,
                6 => {
                    notification.len() >= 24
                        && u32_at(notification, 16)? as usize == notification.len() - 24
                }
                _ => false,
            };
            if !valid {
                return Err(EINVAL);
            }
            // Preserve kernel notifications for the routing/cache owner; never treat them as replies.
            self.state
                .lock()
                .unwrap()
                .notifications
                .push_back((error, body[16..].to_vec()));
            return Ok((body.len() as u64).to_le_bytes().to_vec());
        }
        if error > 0 || error <= -4096 || error != 0 && body.len() != 16 {
            return Err(EINVAL);
        }
        let mut state = self.state.lock().unwrap();
        if state.aborted {
            return Err(errno::ENOENT);
        }
        let Some(pending) = state.pending.get(&unique) else {
            return Err(errno::ENOENT);
        };
        if !pending.delivered {
            return Err(errno::ENOENT);
        }
        if error == 0 {
            validate_reply(&pending.packet, body.len() - 16)?;
        }
        let pending = state.pending.remove(&unique).unwrap();
        drop(state);
        pending
            .reply
            .send(if error == 0 {
                Ok(body[16..].to_vec())
            } else {
                Err(-error)
            })
            .map_err(|_| errno::ENOENT)?;
        Ok((body.len() as u64).to_le_bytes().to_vec())
    }
    fn read_device(&self, stream: &UnixStream, number: u64, body: &[u8]) -> Result<(), Errno> {
        if body.len() != 9 {
            return Err(EINVAL);
        }
        let size = u64_at(body, 0)? as usize;
        let nonblock = body[8] != 0;
        let packet = {
            let mut state = self.state.lock().unwrap();
            if !state.devices.contains_key(&number) {
                return Err(EBADF);
            }
            if !state.mounted {
                return Err(errno::EPERM);
            }
            while state.queue.is_empty() && !state.aborted {
                if nonblock {
                    return Err(errno::EAGAIN);
                }
                state = self
                    .ready
                    .wait_timeout(state, Duration::from_millis(50))
                    .unwrap()
                    .0;
                if peer_closed(stream.as_raw_fd()) {
                    return Err(errno::EINTR);
                }
            }
            if state.aborted {
                return Err(ENODEV);
            }
            let packet = state.queue.pop_front().ok_or(EIO)?;
            if let Some(pending) = state.pending.get_mut(&u64_at(&packet, 8)?) {
                pending.delivered = true;
            }
            packet
        };
        let unique = u64_at(&packet, 8)?;
        if size < packet.len() {
            if let Some(pending) = self.state.lock().unwrap().pending.remove(&unique) {
                let _ = pending.reply.send(Err(EIO));
            }
            return Err(EINVAL);
        }
        if let Err(error) = reply(stream.as_raw_fd(), Ok(&packet)) {
            let mut state = self.state.lock().unwrap();
            if let Some(pending) = state.pending.get_mut(&unique) {
                pending.delivered = false;
            }
            state.queue.push_front(packet);
            Self::signal(&state);
            self.ready.notify_all();
            return Err(error);
        }
        let mut acknowledged = [0];
        if read_exact_fd(stream.as_raw_fd(), &mut acknowledged).is_err() || acknowledged[0] != 1 {
            let mut state = self.state.lock().unwrap();
            if let Some(pending) = state.pending.get_mut(&unique) {
                pending.delivered = false;
            }
            state.queue.push_front(packet);
            Self::signal(&state);
            self.ready.notify_all();
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending.get_mut(&unique) {
            pending.delivered = true;
        }
        if !state.queue.is_empty() {
            Self::signal(&state);
        }
        Ok(())
    }
    fn redirect(&self, number: u64) -> Result<Option<(SessionKey, u64)>, Errno> {
        let state = self.state.lock().unwrap();
        let device = state.devices.get(&number).ok_or(EBADF)?;
        Ok(device
            .redirect
            .as_ref()
            .map(|(key, id, _)| (key.clone(), *id)))
    }
}
fn validate_reply(packet: &[u8], length: usize) -> Result<(), Errno> {
    let op = u32_at(packet, 4)?;
    let fixed = match op {
        1 | 6 | 8 | 9 | 13 => Some(128),
        3 | 4 => Some(104),
        14 | 27 => Some(16),
        16 => Some(8),
        17 => Some(80),
        18 | 20 | 25 | 29 | 30 | 34 | 38 | 10 | 11 | 12 | 45 => Some(0),
        35 => Some(144),
        _ => None,
    };
    if fixed.is_some_and(|expected| length != expected) {
        return Err(EINVAL);
    }
    if matches!(op, 15 | 28 | 44) && length > u32_at(packet, 56)? as usize {
        return Err(EINVAL);
    }
    Ok(())
}
fn peer_closed(fd: RawFd) -> bool {
    let mut byte = 0u8;
    let count = unsafe {
        libc::recv(
            fd,
            (&mut byte as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    count == 0
}
fn read_frame(stream: &UnixStream) -> Result<(u32, u64, Vec<u8>), Errno> {
    let mut header = [0; 24];
    read_exact_fd(stream.as_raw_fd(), &mut header)?;
    if &header[..8] != b"AIMFUSE1" {
        return Err(EINVAL);
    }
    let op = u32_at(&header, 8)?;
    let number = u64_at(&header, 12)?;
    let len = u32_at(&header, 20)? as usize;
    if len > MAX_MESSAGE {
        return Err(EINVAL);
    }
    let mut body = vec![0; len];
    read_exact_fd(stream.as_raw_fd(), &mut body)?;
    Ok((op, number, body))
}
fn clone_device(owner: &Arc<Owner>, number: u64, body: &[u8]) -> Result<Vec<u8>, Errno> {
    let target = SessionKey(PathBuf::from(
        String::from_utf8(body.to_vec()).map_err(|_| EINVAL)?,
    ));
    if target == owner.key {
        return Err(EINVAL);
    }
    {
        let state = owner.state.lock().unwrap();
        if state.mounted || state.devices.get(&number).ok_or(EBADF)?.redirect.is_some() {
            return Err(EINVAL);
        }
    }
    let fd = open_on_key(&target, 0, OPEN_DEVICE, &[])?;
    let (_, source_id) = descriptor(fd)?;
    use std::os::fd::FromRawFd;
    let source = Arc::new(unsafe { UnixStream::from_raw_fd(fd) });
    let signal = {
        let mut state = owner.state.lock().unwrap();
        let device = state.devices.get_mut(&number).ok_or(EBADF)?;
        device.redirect = Some((target.clone(), source_id, source.clone()));
        device.signal.clone()
    };
    std::thread::spawn(move || {
        let mut byte = [0];
        while read_exact_fd(source.as_raw_fd(), &mut byte).is_ok() {
            if write_all_fd(signal.as_raw_fd(), &byte).is_err() {
                break;
            }
        }
    });
    Ok(Vec::new())
}
struct Retiring {
    node: u64,
    fh: u64,
    flags: u32,
    directory: bool,
    read_only: bool,
    uid: u32,
    gid: u32,
    pid: u32,
}
impl Drop for Description {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let retiring = Retiring {
                node: self.node,
                fh: self.fh,
                flags: *self.flags.lock().unwrap(),
                directory: self.directory,
                read_only: self.policy.read_only,
                uid: self.uid,
                gid: self.gid,
                pid: self.pid,
            };
            *owner
                .state
                .lock()
                .unwrap()
                .retiring
                .entry(self.node)
                .or_default() += 1;
            std::thread::spawn(move || retire(owner, retiring));
        }
    }
}
fn retire(owner: Arc<Owner>, description: Retiring) {
    let node = description.node;
    let inode = owner
        .state
        .lock()
        .unwrap()
        .inode_locks
        .entry(node)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _inode = inode.lock().unwrap();
    if !description.directory && description.flags & 0x200000 == 0 {
        loop {
            let writer = if description.flags & 3 == 2 {
                None
            } else {
                owner
                    .state
                    .lock()
                    .unwrap()
                    .files
                    .values()
                    .find(|file| {
                        file.node == node && !file.directory && *file.flags.lock().unwrap() & 3 == 2
                    })
                    .cloned()
            };
            let (fh, flags, uid, gid, pid) = writer
                .as_ref()
                .map(|file| {
                    (
                        file.fh,
                        *file.flags.lock().unwrap(),
                        file.uid,
                        file.gid,
                        file.pid,
                    )
                })
                .unwrap_or((
                    description.fh,
                    description.flags,
                    description.uid,
                    description.gid,
                    description.pid,
                ));
            let result =
                super::super::fuse_cache::flush_retiring(&owner.key, node, |offset, bytes| {
                    if description.read_only {
                        return Err(errno::EROFS);
                    }
                    if flags & 3 != 2 {
                        return Err(EBADF);
                    }
                    let maximum = owner.state.lock().unwrap().max_write as usize;
                    if maximum == 0 {
                        return Err(EPROTO);
                    }
                    let bytes = &bytes[..bytes.len().min(maximum)];
                    let mut payload = Vec::new();
                    payload.extend(fh.to_le_bytes());
                    payload.extend(offset.to_le_bytes());
                    payload.extend((bytes.len() as u32).to_le_bytes());
                    payload.extend(1u32.to_le_bytes());
                    payload.extend(0u64.to_le_bytes());
                    payload.extend(flags.to_le_bytes());
                    payload.extend(0u32.to_le_bytes());
                    payload.extend(bytes);
                    let result = owner.submit(
                        &request_body(16, node, &payload, uid, gid, pid),
                        false,
                        None,
                    )?;
                    let count = u32_at(&result, 0)?;
                    if count as usize > bytes.len() {
                        return Err(EIO);
                    }
                    Ok(count)
                });
            match result {
                Ok(()) => break,
                Err(error) => {
                    eprintln!("FUSE retiring inode writeback failed: {error}");
                    if owner.state.lock().unwrap().aborted {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    if description.flags & 0x200000 == 0 {
        let mut payload = Vec::new();
        payload.extend(description.fh.to_le_bytes());
        payload.extend(description.flags.to_le_bytes());
        payload.extend(0u32.to_le_bytes());
        payload.extend(0u64.to_le_bytes());
        if let Err(error) = owner.submit(
            &request_body(
                if description.directory { 29 } else { 18 },
                node,
                &payload,
                description.uid,
                description.gid,
                description.pid,
            ),
            false,
            None,
        ) {
            if error != ENOTCONN {
                eprintln!("FUSE RELEASE failed: {error}");
            }
        }
    }
    let last = {
        let state = owner.state.lock().unwrap();
        !state.files.values().any(|file| file.node == node)
            && state.retiring.get(&node).copied() == Some(1)
    };
    if last {
        if let Err(error) = super::super::fuse_cache::evict_inode(&owner.key, node) {
            if error != errno::EBUSY {
                eprintln!("FUSE inode cache eviction failed: {error}");
            }
        }
    }
    if let Err(error) = owner.enqueue(
        FORGET,
        node,
        &1u64.to_le_bytes(),
        description.uid,
        description.gid,
        description.pid,
        true,
    ) {
        if error != ENOTCONN {
            eprintln!("FUSE FORGET failed: {error}");
        }
    }
    let mut state = owner.state.lock().unwrap();
    if let Some(count) = state.retiring.get_mut(&node) {
        *count -= 1;
        if *count == 0 {
            state.retiring.remove(&node);
        }
    }
}

fn file_io(
    owner: &Arc<Owner>,
    description: &Description,
    body: &[u8],
    peer: RawFd,
) -> Result<Vec<u8>, Errno> {
    if body.len() < 26 {
        return Err(EINVAL);
    }
    let write = body[0] != 0;
    let explicit = body[1] != 0;
    let flags = *description.flags.lock().unwrap();
    if flags & 0x200000 != 0 {
        return Err(EBADF);
    }
    if write && flags & 3 == 0 || !write && flags & 3 == 1 {
        return Err(EBADF);
    }
    let mut current = description.offset.lock().unwrap();
    let mut offset = if explicit { u64_at(body, 2)? } else { *current };
    let size = u32_at(body, 10)?;
    if write && body.len() != 26 + size as usize {
        return Err(EINVAL);
    }
    if description.directory && write {
        return Err(21);
    }
    if write && description.policy.read_only {
        return Err(30);
    }
    let uid = u32_at(body, 14)?;
    let gid = u32_at(body, 18)?;
    let pid = u32_at(body, 22)?;
    if !description.policy.allow_other && uid != description.policy.uid && uid != 0 {
        return Err(errno::EACCES);
    }
    let inode = owner
        .state
        .lock()
        .unwrap()
        .inode_locks
        .entry(description.node)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _write_guard = if write {
        Some(inode.lock().unwrap())
    } else {
        None
    };
    if write && flags & 0x400 != 0 {
        let mut input = Vec::new();
        input.extend(1u32.to_le_bytes());
        input.extend(0u32.to_le_bytes());
        input.extend(description.fh.to_le_bytes());
        let attr = owner.submit(
            &request_body(3, description.node, &input, uid, gid, pid),
            false,
            Some(peer),
        )?;
        offset = u64_at(&attr, 24)?;
    }
    let opcode = if write {
        16
    } else if description.directory {
        28
    } else {
        15
    };
    let maximum = {
        let mut state = owner.state.lock().unwrap();
        while state.initialized.is_none() && !state.aborted {
            state = owner
                .ready
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
            if peer_closed(peer) {
                return Err(errno::EINTR);
            }
        }
        if state.aborted {
            return Err(ENOTCONN);
        }
        state.initialized.unwrap()?;
        state.max_write as usize
    };
    if maximum == 0 {
        return Err(EPROTO);
    }
    let mut response = Vec::new();
    let mut total = 0usize;
    let mut cookie = offset;
    while total < (size as usize) {
        let chunk = (size as usize - total).min(maximum);
        let position = if description.directory {
            cookie
        } else {
            offset.checked_add(total as u64).ok_or(EINVAL)?
        };
        let mut payload = Vec::new();
        payload.extend(description.fh.to_le_bytes());
        payload.extend(position.to_le_bytes());
        payload.extend((chunk as u32).to_le_bytes());
        payload.extend(0u32.to_le_bytes());
        payload.extend(0u64.to_le_bytes());
        payload.extend(flags.to_le_bytes());
        payload.extend(0u32.to_le_bytes());
        if write {
            payload.extend(&body[26 + total..26 + total + chunk]);
        }
        let part = match owner.submit(
            &request_body(opcode, description.node, &payload, uid, gid, pid),
            false,
            Some(peer),
        ) {
            Ok(part) => part,
            Err(error) => {
                if total == 0 {
                    return Err(error);
                }
                break;
            }
        };
        let advanced = if write {
            if part.len() != 8 {
                return Err(EIO);
            }
            let count = u32_at(&part, 0)? as usize;
            if count > chunk {
                return Err(EIO);
            }
            count
        } else {
            if part.len() > chunk {
                return Err(EIO);
            }
            part.len()
        };
        if description.directory {
            let mut at = 0;
            while at < part.len() {
                let length = u32_at(&part, at + 16)? as usize;
                let step = (24usize.checked_add(length).ok_or(EIO)? + 7) & !7;
                if length == 0 || at + step > part.len() {
                    return Err(EIO);
                }
                cookie = u64_at(&part, at + 8)?;
                at += step;
            }
        }
        if !write {
            response.extend(part);
        }
        total += advanced;
        if advanced < chunk {
            break;
        }
    }
    if write {
        response.extend((total as u32).to_le_bytes());
        response.extend(0u32.to_le_bytes());
    }
    if !explicit {
        *current = if description.directory {
            cookie
        } else {
            offset.checked_add(total as u64).ok_or(EINVAL)?
        };
    }
    let mut output = offset.to_le_bytes().to_vec();
    output.extend(response);
    Ok(output)
}

fn handle(owner: Arc<Owner>, stream: UnixStream) {
    let result = (|| -> Result<(), Errno> {
        let (op, number, body) = read_frame(&stream)?;
        if matches!(op, OPEN_DEVICE | OPEN_FILE | BORROW_FILE) {
            if number == 0 {
                return Err(EINVAL);
            }
            let signal = Arc::new(stream.try_clone().map_err(io_error)?);
            if op == OPEN_DEVICE {
                let mut state = owner.state.lock().unwrap();
                if state.aborted {
                    return Err(ENOTCONN);
                }
                if state.devices.contains_key(&number) {
                    return Err(EINVAL);
                }
                state.seen = true;
                state.devices.insert(
                    number,
                    Device {
                        signal: signal.clone(),
                        redirect: None,
                    },
                );
            } else if op == BORROW_FILE {
                if body.len() != 8 {
                    return Err(EINVAL);
                }
                let node = u64_at(&body, 0)?;
                let mut state = owner.state.lock().unwrap();
                let file = state
                    .files
                    .values()
                    .filter(|file| {
                        file.node == node
                            && !file.directory
                            && *file.flags.lock().unwrap() & 0x200000 == 0
                    })
                    .min_by_key(|file| match *file.flags.lock().unwrap() & 3 {
                        2 => 0,
                        0 => 1,
                        _ => 2,
                    })
                    .cloned()
                    .ok_or(errno::ENOENT)?;
                if state.files.contains_key(&number) {
                    return Err(EINVAL);
                }
                state.files.insert(number, file);
            } else {
                if body.len() < 48 {
                    return Err(EINVAL);
                }
                let mut at = 48;
                let relative = read_text(&body, &mut at)?;
                let guest = read_text(&body, &mut at)?;
                if at != body.len() {
                    return Err(EINVAL);
                }
                let policy = MountPolicy {
                    uid: u32_at(&body, 37)?,
                    gid: u32_at(&body, 41)?,
                    allow_other: body[45] != 0,
                    default_permissions: body[46] != 0,
                    read_only: body[47] != 0,
                };
                let description = Arc::new(Description {
                    node: u64_at(&body, 0)?,
                    fh: u64_at(&body, 8)?,
                    flags: Mutex::new(u32_at(&body, 16)? & !0x80000),
                    directory: body[20] != 0,
                    uid: u32_at(&body, 21)?,
                    gid: u32_at(&body, 25)?,
                    pid: u32_at(&body, 29)?,
                    open_flags: u32_at(&body, 33)?,
                    policy,
                    relative,
                    guest,
                    owner: Arc::downgrade(&owner),
                    offset: Mutex::new(0),
                });
                let inode = owner
                    .state
                    .lock()
                    .unwrap()
                    .inode_locks
                    .entry(description.node)
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone();
                let _inode = inode.lock().unwrap();
                let mut state = owner.state.lock().unwrap();
                if state.aborted {
                    return Err(ENOTCONN);
                }
                if state.files.contains_key(&number) {
                    return Err(EINVAL);
                }
                state.files.insert(number, description);
                state.seen = true;
            }
            reply(stream.as_raw_fd(), Ok(&[]))?;
            if op == OPEN_DEVICE {
                let state = owner.state.lock().unwrap();
                if !state.queue.is_empty() {
                    Owner::signal(&state);
                }
            }
            let mut byte = [0];
            let _ = read_exact_fd(stream.as_raw_fd(), &mut byte);
            if op == OPEN_DEVICE {
                let redirect = owner
                    .state
                    .lock()
                    .unwrap()
                    .devices
                    .remove(&number)
                    .and_then(|device| device.redirect);
                if let Some((_, _, socket)) = redirect {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
            } else {
                let description = owner.state.lock().unwrap().files.remove(&number);
                drop(description);
            }
            let local = socket_path(stream.as_raw_fd(), true)?;
            let _ = fs::remove_file(local);
            let no_devices = owner.state.lock().unwrap().devices.is_empty();
            if no_devices {
                owner.abort();
            }
            return Ok(());
        }
        if matches!(op, DEVICE_READ | DEVICE_WRITE | DEVICE_POLL) {
            if let Some((key, id)) = owner.redirect(number)? {
                if op == DEVICE_READ {
                    let source = connect(&key)?;
                    send_frame(source.as_raw_fd(), op, id, &body)?;
                    let value = read_reply(source.as_raw_fd())?;
                    reply(stream.as_raw_fd(), Ok(&value))?;
                    let mut ack = [0];
                    read_exact_fd(stream.as_raw_fd(), &mut ack)?;
                    write_all_fd(source.as_raw_fd(), &ack)?;
                } else {
                    let value = rpc(&key, op, id, &body)?;
                    reply(stream.as_raw_fd(), Ok(&value))?;
                }
                return Ok(());
            }
        }
        if op == DEVICE_READ {
            return owner.read_device(&stream, number, &body);
        }
        let value = match op {
            IDENTIFY => {
                if number != 0 && !owner.state.lock().unwrap().files.contains_key(&number) {
                    if let Some((target, _)) = owner.redirect(number)? {
                        target.0.to_str().ok_or(EINVAL)?.as_bytes().to_vec()
                    } else {
                        owner.key.0.to_str().ok_or(EINVAL)?.as_bytes().to_vec()
                    }
                } else {
                    owner.key.0.to_str().ok_or(EINVAL)?.as_bytes().to_vec()
                }
            }
            DEVICE_WRITE => owner.reply_device(&body)?,
            DEVICE_POLL => {
                let state = owner.state.lock().unwrap();
                let mask: u16 = if state.aborted {
                    8
                } else {
                    4 | if state.queue.is_empty() { 0 } else { 1 }
                };
                mask.to_le_bytes().to_vec()
            }
            REQUEST => {
                if u32_at(&body, 0)? == INIT {
                    let mut state = owner.state.lock().unwrap();
                    while state.initialized.is_none() && !state.aborted {
                        drop(state);
                        owner.mount()?;
                        state = owner.state.lock().unwrap();
                        state = owner
                            .ready
                            .wait_timeout(state, Duration::from_millis(50))
                            .unwrap()
                            .0;
                    }
                    if state.aborted {
                        return Err(ENOTCONN);
                    }
                    state.initialized.unwrap()?;
                    let body = state.init_reply.clone().ok_or(EIO)?;
                    drop(state);
                    reply(stream.as_raw_fd(), Ok(&body))?;
                    return Ok(());
                }
                owner.mount()?;
                owner.submit(
                    &body,
                    matches!(u32_at(&body, 0)?, FORGET | BATCH_FORGET),
                    Some(stream.as_raw_fd()),
                )?
            }
            NO_REPLY => {
                owner.mount()?;
                owner.submit(&body, true, Some(stream.as_raw_fd()))?
            }
            MOUNT => {
                owner.mount()?;
                Vec::new()
            }
            ABORT => {
                owner.abort();
                Vec::new()
            }
            CLONE => clone_device(&owner, number, &body)?,
            NOTIFICATIONS => {
                let mut state = owner.state.lock().unwrap();
                let mut output = (state.notifications.len() as u32).to_le_bytes().to_vec();
                for (code, body) in state.notifications.drain(..) {
                    output.extend(code.to_le_bytes());
                    output.extend((body.len() as u32).to_le_bytes());
                    output.extend(body);
                }
                output
            }
            FILE_DESCRIBE => {
                let state = owner.state.lock().unwrap();
                let file = state.files.get(&number).ok_or(EBADF)?;
                let mut out = Vec::new();
                out.extend(file.node.to_le_bytes());
                out.extend(file.fh.to_le_bytes());
                out.extend(file.flags.lock().unwrap().to_le_bytes());
                out.push(file.directory as u8);
                out.extend(file.open_flags.to_le_bytes());
                out.extend(file.policy.uid.to_le_bytes());
                out.extend(file.policy.gid.to_le_bytes());
                out.push(file.policy.allow_other as u8);
                out.push(file.policy.default_permissions as u8);
                out.push(file.policy.read_only as u8);
                text(&mut out, &file.relative);
                text(&mut out, &file.guest);
                out
            }
            FILE_OFFSET => {
                let file = owner
                    .state
                    .lock()
                    .unwrap()
                    .files
                    .get(&number)
                    .cloned()
                    .ok_or(EBADF)?;
                let mut offset = file.offset.lock().unwrap();
                if !body.is_empty() {
                    if body.len() != 8 {
                        return Err(EINVAL);
                    }
                    *offset = u64_at(&body, 0)?;
                }
                offset.to_le_bytes().to_vec()
            }
            FILE_FLAGS => {
                let file = owner
                    .state
                    .lock()
                    .unwrap()
                    .files
                    .get(&number)
                    .cloned()
                    .ok_or(EBADF)?;
                let mut flags = file.flags.lock().unwrap();
                if !body.is_empty() {
                    if body.len() != 4 {
                        return Err(EINVAL);
                    }
                    let mutable = 0x400 | 0x800 | 0x2000 | 0x4000 | 0x40000;
                    *flags = (*flags & !mutable) | (u32_at(&body, 0)? & mutable);
                }
                flags.to_le_bytes().to_vec()
            }
            FILE_IO => {
                let file = owner
                    .state
                    .lock()
                    .unwrap()
                    .files
                    .get(&number)
                    .cloned()
                    .ok_or(EBADF)?;
                file_io(&owner, &file, &body, stream.as_raw_fd())?
            }
            _ => return Err(EINVAL),
        };
        reply(stream.as_raw_fd(), Ok(&value))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = reply(stream.as_raw_fd(), Err(error));
    }
}
pub fn serve(path: &Path) -> Result<(), Errno> {
    let listener = UnixListener::bind(path).map_err(io_error)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    listener.set_nonblocking(true).map_err(io_error)?;
    let owner = Arc::new(Owner {
        key: SessionKey(path.to_owned()),
        state: Mutex::new(State {
            mounted: false,
            initialized: None,
            init_reply: None,
            max_write: 0,
            aborted: false,
            next: 1,
            queue: VecDeque::new(),
            pending: BTreeMap::new(),
            devices: BTreeMap::new(),
            files: BTreeMap::new(),
            notifications: VecDeque::new(),
            inode_locks: BTreeMap::new(),
            retiring: BTreeMap::new(),
            seen: false,
        }),
        ready: Condvar::new(),
    });
    let started = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).map_err(io_error)?;
                let owner = owner.clone();
                no_sigpipe(stream.as_raw_fd());
                std::thread::spawn(move || handle(owner, stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_error(error)),
        }
        {
            let state = owner.state.lock().unwrap();
            if state.seen
                && state.devices.is_empty()
                && state.files.is_empty()
                && state.retiring.is_empty()
                || !state.seen && started.elapsed() > Duration::from_secs(10)
            {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    owner.abort();
    let _ = fs::remove_file(path);
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}
