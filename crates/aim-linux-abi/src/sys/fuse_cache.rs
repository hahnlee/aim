//! FUSE inode cache backed by a real shared Darwin file (#221).
//! Linux cached FUSE semantics: docs.kernel.org/filesystems/fuse/fuse-io.html.
//! The baseline records only bytes acknowledged by the actual FUSE server;
//! mmap writes remain dirty until their differing bytes are acknowledged.
use super::{
    fuse::SessionKey,
    fuse_client::{self, Open},
};
use crate::errno::{self, EACCES, EINVAL, EIO, Errno};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{FileExt, OpenOptionsExt},
    },
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, MutexGuard, Weak, mpsc},
};
const CHUNK: usize = 128 * 1024;
struct Locked<'a> {
    file: &'a File,
    _local: MutexGuard<'a, ()>,
}
impl Drop for Locked<'_> {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn lock(cache: &Cache) -> Result<Locked<'_>, Errno> {
    let local = cache.gate.lock().unwrap();
    let file = &cache.control;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(Locked {
                file,
                _local: local,
            });
        }
        let error = errno::last();
        if error != crate::errno::EINTR {
            return Err(error);
        }
    }
}
fn io(error: std::io::Error) -> Errno {
    error.raw_os_error().map(errno::from_darwin).unwrap_or(EIO)
}
fn root(session: &std::path::Path) -> Result<PathBuf, Errno> {
    let parent = session.parent().ok_or(EINVAL)?;
    let name = session.file_name().ok_or(EINVAL)?.to_string_lossy();
    let path = parent.join(format!(".{name}.inode-cache"));
    match fs::create_dir(&path) {
        Ok(()) => {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(io)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&path).map_err(io)?.is_dir() {
                return Err(EIO);
            }
        }
        Err(e) => return Err(io(e)),
    }
    Ok(path)
}
fn backing(path: PathBuf) -> Result<File, Errno> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(io)
}
fn size(file: &Open) -> Result<u64, Errno> {
    let attr = fuse_client::getattr(&file.route, file.node, Some(file.fh))?;
    Ok(u64::from_le_bytes(
        attr.get(8..16).ok_or(EIO)?.try_into().unwrap(),
    ))
}
fn read_exact(file: &File, bytes: &mut [u8], mut offset: u64) -> Result<(), Errno> {
    let mut at = 0;
    while at < bytes.len() {
        let n = file.read_at(&mut bytes[at..], offset).map_err(io)?;
        if n == 0 {
            return Err(EIO);
        }
        at += n;
        offset += n as u64;
    }
    Ok(())
}
fn write_all(file: &File, bytes: &[u8], mut offset: u64) -> Result<(), Errno> {
    let mut at = 0;
    while at < bytes.len() {
        let n = file.write_at(&bytes[at..], offset).map_err(io)?;
        if n == 0 {
            return Err(EIO);
        }
        at += n;
        offset += n as u64;
    }
    Ok(())
}
struct Borrowed {
    _lease: OwnedFd,
    file: Arc<Open>,
}
fn borrow(key: &SessionKey, node: u64) -> Result<Borrowed, Errno> {
    let fd = super::fuse::borrow_inode_descriptor(key, node)?;
    let description = super::fuse::description(fd.as_raw_fd())?;
    if description.key != *key || description.node != node || description.directory {
        return Err(EINVAL);
    }
    let policy = description.policy;
    let route = crate::vfs::FuseRoute {
        session: description.key.transport().to_owned(),
        relative: description.relative,
        uid: policy.uid,
        gid: policy.gid,
        allow_other: policy.allow_other,
        default_permissions: policy.default_permissions,
        read_only: policy.read_only,
    };
    let file = Arc::new(Open {
        guest: description.guest,
        route,
        node: description.node,
        fh: description.fh,
        flags: description.flags,
        open_flags: description.open_flags,
        broker_owned: true,
        directory: false,
        offset: Mutex::new(0),
    });
    Ok(Borrowed { _lease: fd, file })
}
/// The independent broker retains the retiring description while this writes
/// actual differing cache bytes. The callback submits directly to that owner,
/// avoiding RPC back into the broker which is retiring the description.
pub fn flush_retiring(
    key: &SessionKey,
    node: u64,
    mut write: impl FnMut(u64, &[u8]) -> Result<u32, Errno>,
) -> Result<(), Errno> {
    let dir = root(key.transport())?;
    let control = backing(dir.join(format!("{node}.state")))?;
    loop {
        if unsafe { libc::flock(control.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let error = errno::last();
        if error != crate::errno::EINTR {
            return Err(error);
        }
    }
    let result = (|| {
        if control.metadata().map_err(io)?.len() != 8 {
            return Ok(());
        }
        let data = backing(dir.join(format!("{node}.pages")))?;
        let baseline = backing(dir.join(format!("{node}.baseline")))?;
        let size = data.metadata().map_err(io)?.len();
        let mut at = 0;
        while at < size {
            let n = (size - at).min(CHUNK as u64) as usize;
            let mut bytes = vec![0; n];
            let mut old = vec![0; n];
            read_exact(&data, &mut bytes, at)?;
            read_exact(&baseline, &mut old, at)?;
            let mut i = 0;
            while i < n {
                if bytes[i] == old[i] {
                    i += 1;
                    continue;
                }
                let first = i;
                i += 1;
                while i < n && bytes[i] != old[i] {
                    i += 1;
                }
                let mut done = first;
                while done < i {
                    let count = write(at + done as u64, &bytes[done..i])? as usize;
                    if count == 0 || count > i - done {
                        return Err(EIO);
                    }
                    write_all(&baseline, &bytes[done..done + count], at + done as u64)?;
                    done += count;
                }
            }
            at += n as u64;
        }
        baseline.sync_data().map_err(io)
    })();
    if let Err(error) = result {
        let errors = backing(dir.join(format!("{node}.errors")))?;
        let sequence = if errors.metadata().map_err(io)?.len() == 12 {
            let mut bytes = [0; 12];
            read_exact(&errors, &mut bytes, 0)?;
            u64::from_le_bytes(bytes[..8].try_into().unwrap())
        } else {
            0
        };
        let mut bytes = Vec::new();
        bytes.extend(sequence.wrapping_add(1).to_le_bytes());
        bytes.extend(error.to_le_bytes());
        write_all(&errors, &bytes, 0)?;
        errors.sync_data().map_err(io)?;
    }
    unsafe {
        libc::flock(control.as_raw_fd(), libc::LOCK_UN);
    }
    result
}
pub struct Cache {
    source: Arc<Open>,
    data: File,
    baseline: File,
    control: File,
    errors: File,
    gate: Mutex<()>,
    _lease: Option<OwnedFd>,
}
impl Cache {
    pub fn fd(&self) -> i32 {
        self.data.as_raw_fd()
    }
    fn open(source: Arc<Open>, fd: Option<i32>, hydrate: bool) -> Result<Arc<Self>, Errno> {
        if source.directory {
            return Err(crate::errno::ENODEV);
        }
        let dir = root(&source.route.session)?;
        let lease = if let Some(fd) = fd {
            let retained = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if retained < 0 {
                return Err(errno::last());
            }
            Some(unsafe { OwnedFd::from_raw_fd(retained) })
        } else {
            None
        };
        let cache = Arc::new(Self {
            data: backing(dir.join(format!("{}.pages", source.node)))?,
            baseline: backing(dir.join(format!("{}.baseline", source.node)))?,
            control: backing(dir.join(format!("{}.state", source.node)))?,
            errors: backing(dir.join(format!("{}.errors", source.node)))?,
            gate: Mutex::new(()),
            source,
            _lease: lease,
        });
        if hydrate {
            cache.hydrate()?;
        }
        Ok(cache)
    }
    fn hydrate(&self) -> Result<(), Errno> {
        let _lock = lock(self)?;
        if self.control.metadata().map_err(io)?.len() == 8 {
            return Ok(());
        }
        let size = size(&self.source)?;
        self.data.set_len(size).map_err(io)?;
        self.baseline.set_len(size).map_err(io)?;
        let mut offset = 0;
        while offset < size {
            let want = (size - offset).min(CHUNK as u64) as u32;
            let bytes = self.source.read(offset, want)?;
            if bytes.len() != want as usize {
                return Err(EIO);
            }
            write_all(&self.data, &bytes, offset)?;
            write_all(&self.baseline, &bytes, offset)?;
            offset += bytes.len() as u64;
        }
        self.data.sync_data().map_err(io)?;
        self.baseline.sync_data().map_err(io)?;
        write_all(&self.control, &size.to_le_bytes(), 0)?;
        self.control.set_len(8).map_err(io)?;
        self.control.sync_data().map_err(io)
    }
    fn flush_locked(&self, offset: u64, length: u64) -> Result<(), Errno> {
        let size = self.data.metadata().map_err(io)?.len();
        let end = offset.saturating_add(length).min(size);
        let mut at = offset;
        let mut borrowed = None;
        while at < end {
            let n = (end - at).min(CHUNK as u64) as usize;
            let mut bytes = vec![0; n];
            let mut baseline = vec![0; n];
            read_exact(&self.data, &mut bytes, at)?;
            read_exact(&self.baseline, &mut baseline, at)?;
            let mut i = 0;
            while i < n {
                if bytes[i] == baseline[i] {
                    i += 1;
                    continue;
                }
                let start = i;
                i += 1;
                while i < n && bytes[i] != baseline[i] {
                    i += 1;
                }
                let mut done = start;
                while done < i {
                    let source = if self.source.flags & 3 == 2 {
                        &self.source
                    } else {
                        if borrowed.is_none() {
                            borrowed = Some(borrow(
                                &SessionKey::from_transport(self.source.route.session.clone()),
                                self.source.node,
                            )?);
                        }
                        let source = &borrowed.as_ref().unwrap().file;
                        if source.flags & 3 != 2 {
                            return Err(crate::errno::EBADF);
                        }
                        source
                    };
                    let written = source.write(at + done as u64, &bytes[done..i])? as usize;
                    if written == 0 {
                        return Err(EIO);
                    }
                    write_all(
                        &self.baseline,
                        &bytes[done..done + written],
                        at + done as u64,
                    )?;
                    done += written;
                }
            }
            at += n as u64;
        }
        self.baseline.sync_data().map_err(io)
    }
    fn flush(&self, offset: u64, length: u64) -> Result<(), Errno> {
        let _lock = lock(self)?;
        let result = self.flush_locked(offset, length);
        if let Err(error) = result {
            self.record_error(error)?;
        }
        result
    }
    fn error_state(&self) -> Result<(u64, Errno), Errno> {
        if self.errors.metadata().map_err(io)?.len() == 0 {
            return Ok((0, 0));
        }
        let mut bytes = [0u8; 12];
        read_exact(&self.errors, &mut bytes, 0)?;
        Ok((
            u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            i32::from_le_bytes(bytes[8..].try_into().unwrap()),
        ))
    }
    fn record_error(&self, error: Errno) -> Result<(), Errno> {
        let (seq, _) = self.error_state()?;
        let mut bytes = Vec::new();
        bytes.extend(seq.wrapping_add(1).to_le_bytes());
        bytes.extend(error.to_le_bytes());
        write_all(&self.errors, &bytes, 0)?;
        self.errors.set_len(12).map_err(io)?;
        self.errors.sync_data().map_err(io)
    }
}
#[derive(Clone)]
struct Mapping {
    start: u64,
    end: u64,
    offset: u64,
    shared: bool,
    cache: Arc<Cache>,
}
static MAPPINGS: LazyLock<Mutex<Vec<Mapping>>> = LazyLock::new(Default::default);
static CACHES: LazyLock<Mutex<BTreeMap<(PathBuf, u64), Weak<Cache>>>> =
    LazyLock::new(Default::default);
struct Cursor {
    file: Weak<Open>,
    sequence: u64,
}
static CURSORS: LazyLock<Mutex<BTreeMap<(PathBuf, u64), Vec<Cursor>>>> =
    LazyLock::new(Default::default);
pub fn register_file(file: &Arc<Open>) -> Result<(), Errno> {
    if file.directory || file.flags & 0x200000 != 0 {
        return Ok(());
    }
    let cache = Cache::open(file.clone(), None, false)?;
    let _lock = lock(&cache)?;
    let sequence = cache.error_state()?.0;
    let mut cursors = CURSORS.lock().unwrap();
    let entries = cursors
        .entry((file.route.session.clone(), file.node))
        .or_default();
    entries.retain(|cursor| cursor.file.strong_count() != 0);
    if !entries.iter().any(|cursor| {
        cursor
            .file
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, file))
    }) {
        entries.push(Cursor {
            file: Arc::downgrade(file),
            sequence,
        });
    }
    Ok(())
}
fn check_error(cache: &Cache, file: &Arc<Open>) -> Result<(), Errno> {
    let (sequence, error) = cache.error_state()?;
    let mut cursors = CURSORS.lock().unwrap();
    let entries = cursors
        .entry((file.route.session.clone(), file.node))
        .or_default();
    let Some(cursor) = entries.iter_mut().find(|cursor| {
        cursor
            .file
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, file))
    }) else {
        entries.push(Cursor {
            file: Arc::downgrade(file),
            sequence,
        });
        return Ok(());
    };
    if sequence != cursor.sequence {
        cursor.sequence = sequence;
        if error != 0 {
            return Err(error);
        }
    }
    Ok(())
}
enum Job {
    Flush(Vec<Mapping>),
    Stop(mpsc::Sender<Result<(), Errno>>),
}
fn word(bytes: &[u8], at: usize) -> Result<u64, Errno> {
    Ok(u64::from_le_bytes(
        bytes.get(at..at + 8).ok_or(EIO)?.try_into().unwrap(),
    ))
}
pub fn drain_notifications(key: &SessionKey) -> Result<(), Errno> {
    let client = super::fuse::Client::from_key(key)?;
    for notification in client.take_notifications()? {
        match notification.code {
            2 => {
                let node = word(&notification.body, 0)?;
                let offset = word(&notification.body, 8)?;
                let len = word(&notification.body, 16)?;
                // A negative signed offset invalidates only inode attributes.
                // VFS fetches attributes from the daemon on every query.
                if (offset as i64) < 0 {
                    continue;
                }
                invalidate(key, node, offset, if len == 0 { u64::MAX } else { len })?;
            }
            // Every VFS name lookup reaches the daemon; this owner retains no
            // dentry/name cache for ENTRY/DELETE to invalidate.
            3 | 6 => {}
            4 => {
                let node = word(&notification.body, 0)?;
                let offset = word(&notification.body, 8)?;
                let count = u32::from_le_bytes(
                    notification
                        .body
                        .get(16..20)
                        .ok_or(EIO)?
                        .try_into()
                        .unwrap(),
                ) as usize;
                let bytes = notification.body.get(24..24 + count).ok_or(EIO)?;
                let borrowed = borrow(key, node)?;
                let cache = Cache::open(borrowed.file.clone(), None, true)?;
                let _lock = lock(&cache)?;
                write_all(&cache.data, bytes, offset)?;
                write_all(&cache.baseline, bytes, offset)?;
            }
            _ => return Err(errno::from_darwin(libc::EOPNOTSUPP)),
        }
    }
    Ok(())
}
fn notify_mappings() -> Result<(), Errno> {
    let keys = MAPPINGS
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.cache.source.route.session.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for key in keys {
        drain_notifications(&SessionKey::from_transport(key))?;
    }
    Ok(())
}
struct Worker {
    sender: mpsc::Sender<Job>,
    thread: std::thread::JoinHandle<()>,
}
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
static FAILED: LazyLock<Mutex<BTreeMap<(PathBuf, u64), Arc<Cache>>>> =
    LazyLock::new(Default::default);
fn mapped_shared() -> Vec<Mapping> {
    MAPPINGS
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.shared && m.cache.source.flags & 3 == 2)
        .cloned()
        .collect()
}
fn flush_mappings(maps: Vec<Mapping>) -> Result<(), Errno> {
    let mut error = None;
    for m in maps {
        if let Err(failure) = m.cache.flush(m.offset, m.end - m.start) {
            FAILED.lock().unwrap().insert(
                (m.cache.source.route.session.clone(), m.cache.source.node),
                m.cache.clone(),
            );
            error.get_or_insert(failure);
        }
    }
    error.map_or(Ok(()), Err)
}
fn retry_failed() -> Result<(), Errno> {
    let caches = FAILED.lock().unwrap().values().cloned().collect::<Vec<_>>();
    let mut error = None;
    for cache in caches {
        match cache.flush(0, u64::MAX) {
            Ok(()) => {
                FAILED
                    .lock()
                    .unwrap()
                    .remove(&(cache.source.route.session.clone(), cache.source.node));
            }
            Err(failure) => {
                error.get_or_insert(failure);
            }
        }
    }
    error.map_or(Ok(()), Err)
}
fn start_worker() -> Result<mpsc::Sender<Job>, Errno> {
    let mut slot = WORKER.lock().unwrap();
    if let Some(worker) = slot.as_ref() {
        return Ok(worker.sender.clone());
    }
    let (sender, receiver) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("fuse-writeback".into())
        .spawn(move || {
            let mut next_flush = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(Job::Flush(maps)) => {
                        if let Err(error) = flush_mappings(maps) {
                            eprintln!("FUSE asynchronous writeback failed: {error}");
                        }
                    }
                    Ok(Job::Stop(reply)) => {
                        let result = flush_mappings(mapped_shared()).and(retry_failed());
                        let _ = reply.send(result);
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if std::time::Instant::now() >= next_flush {
                            if let Err(error) = flush_mappings(mapped_shared()) {
                                eprintln!("FUSE background writeback failed: {error}");
                            }
                            if let Err(error) = retry_failed() {
                                eprintln!("FUSE retained dirty inode retry failed: {error}");
                            }
                            next_flush =
                                std::time::Instant::now() + std::time::Duration::from_secs(5);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if let Err(error) = notify_mappings() {
                    eprintln!("FUSE cache notification failed: {error}");
                }
            }
        })
        .map_err(io)?;
    *slot = Some(Worker {
        sender: sender.clone(),
        thread,
    });
    Ok(sender)
}
pub fn schedule_range(start: u64, length: u64) -> Result<(), Errno> {
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let maps = MAPPINGS
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.shared && m.start < end && m.end > start)
        .map(|m| {
            let lo = m.start.max(start);
            let hi = m.end.min(end);
            Mapping {
                start: lo,
                end: hi,
                offset: m.offset + lo - m.start,
                ..m.clone()
            }
        })
        .collect::<Vec<_>>();
    if maps.is_empty() {
        return Ok(());
    }
    start_worker()?.send(Job::Flush(maps)).map_err(|_| EIO)
}
/// Drain queued WRITE requests before final mapping/descriptor ownership drops.
/// The caller reports retained errors; they are also persisted for later fsync.
pub fn shutdown_writeback() -> Result<(), Errno> {
    let worker = WORKER.lock().unwrap().take();
    let Some(worker) = worker else {
        return flush_mappings(mapped_shared()).and(retry_failed());
    };
    let (sender, receiver) = mpsc::channel();
    worker.sender.send(Job::Stop(sender)).map_err(|_| EIO)?;
    let result = receiver.recv().map_err(|_| EIO)?;
    worker.thread.join().map_err(|_| EIO)?;
    result
}
fn cached(file: &Arc<Open>) -> Result<Arc<Cache>, Errno> {
    let key = (file.route.session.clone(), file.node);
    if let Some(cache) = CACHES.lock().unwrap().get(&key).and_then(Weak::upgrade) {
        return Ok(cache);
    }
    Cache::open(file.clone(), None, true)
}
pub fn prepare(
    file: Arc<Open>,
    fd: i32,
    shared: bool,
    writable: bool,
) -> Result<Arc<Cache>, Errno> {
    if file.flags & 0o10000000 != 0 {
        return Err(crate::errno::EBADF);
    }
    if file.flags & 3 == 1 || shared && writable && file.flags & 3 != 2 {
        return Err(EACCES);
    }
    if file.open_flags & 1 != 0 {
        return Err(crate::errno::ENODEV);
    }
    register_file(&file)?;
    let cache = Cache::open(file, Some(fd), true)?;
    let key = (cache.source.route.session.clone(), cache.source.node);
    let mut caches = CACHES.lock().unwrap();
    let current = caches.get(&key).and_then(Weak::upgrade);
    if current.is_none_or(|current| current.source.flags & 3 != 2 || cache.source.flags & 3 == 2) {
        caches.insert(key, Arc::downgrade(&cache));
    }
    Ok(cache)
}
pub fn note(
    cache: Arc<Cache>,
    start: u64,
    length: u64,
    offset: u64,
    shared: bool,
) -> Result<(), Errno> {
    start_worker()?;
    MAPPINGS.lock().unwrap().push(Mapping {
        start,
        end: start + length,
        offset,
        shared,
        cache,
    });
    Ok(())
}
pub fn flush_range(start: u64, length: u64) -> Result<(), Errno> {
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let maps: Vec<_> = MAPPINGS
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.shared && m.start < end && m.end > start)
        .cloned()
        .collect();
    for m in maps {
        let lo = m.start.max(start);
        let hi = m.end.min(end);
        m.cache.flush(m.offset + lo - m.start, hi - lo)?;
        let _lock = lock(&m.cache)?;
        check_error(&m.cache, &m.cache.source)?;
    }
    Ok(())
}
pub fn forget(start: u64, length: u64) {
    let end = start.saturating_add(length);
    let mut maps = MAPPINGS.lock().unwrap();
    let old = std::mem::take(&mut *maps);
    for m in old {
        if m.start >= end || m.end <= start {
            maps.push(m);
            continue;
        }
        if m.start < start {
            maps.push(Mapping {
                end: start,
                ..m.clone()
            });
        }
        if m.end > end {
            maps.push(Mapping {
                start: end,
                offset: m.offset + end - m.start,
                ..m
            });
        }
    }
}
pub fn flush_file(file: &Arc<Open>) -> Result<(), Errno> {
    if file.directory || file.flags & 0x200000 != 0 {
        return Ok(());
    }
    register_file(file)?;
    let cache = CACHES
        .lock()
        .unwrap()
        .get(&(file.route.session.clone(), file.node))
        .and_then(Weak::upgrade);
    let cache = match cache {
        Some(cache) => cache,
        None => Cache::open(file.clone(), None, false)?,
    };
    let _lock = lock(&cache)?;
    if cache.control.metadata().map_err(io)?.len() == 8 {
        if let Err(error) = cache.flush_locked(0, u64::MAX) {
            cache.record_error(error)?;
            return Err(error);
        }
    }
    check_error(&cache, file)
}
pub fn read_cached(file: &Arc<Open>, offset: u64, size: u32) -> Result<Vec<u8>, Errno> {
    let cache = cached(file)?;
    let _lock = lock(&cache)?;
    let length = cache.data.metadata().map_err(io)?.len();
    let count = length.saturating_sub(offset).min(size as u64) as usize;
    let mut bytes = vec![0; count];
    read_exact(&cache.data, &mut bytes, offset)?;
    Ok(bytes)
}
pub fn write_through(file: &Arc<Open>, offset: u64, bytes: &[u8]) -> Result<u32, Errno> {
    let cache = Cache::open(file.clone(), None, false)?;
    let _lock = lock(&cache)?;
    let initialized = cache.control.metadata().map_err(io)?.len() == 8;
    if initialized {
        cache.flush_locked(0, u64::MAX)?;
    }
    let n = file.write(offset, bytes)?;
    if initialized {
        let bytes = &bytes[..n as usize];
        write_all(&cache.data, bytes, offset)?;
        write_all(&cache.baseline, bytes, offset)?;
    }
    Ok(n)
}
/// Apply only the bytes already acknowledged by the actual server WRITE.
/// An inode without populated cached pages needs no synthetic READ for writes.
pub fn update_after_write(file: &Arc<Open>, offset: u64, bytes: &[u8]) -> Result<(), Errno> {
    let cache = Cache::open(file.clone(), None, false)?;
    let _lock = lock(&cache)?;
    if cache.control.metadata().map_err(io)?.len() == 8 {
        write_all(&cache.data, bytes, offset)?;
        write_all(&cache.baseline, bytes, offset)?;
    }
    Ok(())
}
/// Server notification ordering: acknowledge dirty data before replacing clean
/// cached bytes. No retained genuine handle means refresh is unavailable.
pub fn invalidate(key: &SessionKey, node: u64, offset: u64, len: u64) -> Result<(), Errno> {
    let cache = CACHES
        .lock()
        .unwrap()
        .get(&(key.transport().to_owned(), node))
        .and_then(Weak::upgrade);
    if let Some(cache) = cache {
        return invalidate_cache(&cache, offset, len);
    }
    let directory = root(key.transport())?;
    let state = directory.join(format!("{node}.state"));
    match fs::symlink_metadata(state) {
        Ok(metadata) if metadata.len() == 8 => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io(error)),
    }
    let borrowed = borrow(key, node)?;
    invalidate_with_open(key, node, borrowed.file.clone(), offset, len)
}
/// An actual active Open supplied by VFS also refreshes a cache populated by
/// another process; it never treats absence of a local registry entry as proof
/// that the shared inode has no cached pages.
pub fn invalidate_range(file: &Arc<Open>, offset: u64, len: u64) -> Result<(), Errno> {
    let cache = CACHES
        .lock()
        .unwrap()
        .get(&(file.route.session.clone(), file.node))
        .and_then(Weak::upgrade);
    let cache = match cache {
        Some(cache) => cache,
        None => Cache::open(file.clone(), None, false)?,
    };
    invalidate_cache(&cache, offset, len)
}
/// The notification broker supplies an actual retained file description when
/// the mapping lives in another process. Validate its inode/session provenance
/// before issuing any READ or WRITE against that genuine handle.
pub fn invalidate_with_open(
    key: &SessionKey,
    node: u64,
    file: Arc<Open>,
    offset: u64,
    len: u64,
) -> Result<(), Errno> {
    if file.route.session != key.transport() || file.node != node || file.directory {
        return Err(EINVAL);
    }
    invalidate_range(&file, offset, len)
}
/// Called by the broker only after genuine last-FORGET and final description
/// retirement. Active VMA leases prevent this boundary from being reached.
pub fn evict_inode(key: &SessionKey, node: u64) -> Result<(), Errno> {
    if CACHES
        .lock()
        .unwrap()
        .get(&(key.transport().to_owned(), node))
        .and_then(Weak::upgrade)
        .is_some()
    {
        return Err(crate::errno::EBUSY);
    }
    let dir = root(key.transport())?;
    let control = backing(dir.join(format!("{node}.state")))?;
    loop {
        if unsafe { libc::flock(control.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let error = errno::last();
        if error != crate::errno::EINTR {
            return Err(error);
        }
    }
    let mut result = Ok(());
    let data = dir.join(format!("{node}.pages"));
    let baseline = dir.join(format!("{node}.baseline"));
    if data.exists() && baseline.exists() {
        let data = File::open(data).map_err(io)?;
        let baseline = File::open(baseline).map_err(io)?;
        let size = data.metadata().map_err(io)?.len();
        if baseline.metadata().map_err(io)?.len() != size {
            return Err(crate::errno::EBUSY);
        }
        let mut at = 0;
        while at < size {
            let n = (size - at).min(CHUNK as u64) as usize;
            let mut current = vec![0; n];
            let mut written = vec![0; n];
            read_exact(&data, &mut current, at)?;
            read_exact(&baseline, &mut written, at)?;
            if current != written {
                return Err(crate::errno::EBUSY);
            }
            at += n as u64;
        }
    }
    for suffix in ["pages", "baseline", "errors", "state"] {
        match fs::remove_file(dir.join(format!("{node}.{suffix}"))) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                result = Err(io(error));
                break;
            }
        }
    }
    unsafe {
        libc::flock(control.as_raw_fd(), libc::LOCK_UN);
    }
    result
}
fn invalidate_cache(cache: &Cache, offset: u64, len: u64) -> Result<(), Errno> {
    let _lock = lock(&cache)?;
    if cache.control.metadata().map_err(io)?.len() != 8 {
        return Ok(());
    }
    cache.flush_locked(offset, len)?;
    let size = size(&cache.source)?;
    let end = offset.saturating_add(len).min(size);
    let mut at = offset;
    while at < end {
        let n = (end - at).min(CHUNK as u64) as u32;
        let bytes = cache.source.read(at, n)?;
        if bytes.len() != n as usize {
            return Err(EIO);
        }
        write_all(&cache.data, &bytes, at)?;
        write_all(&cache.baseline, &bytes, at)?;
        at += n as u64;
    }
    cache.data.set_len(size).map_err(io)?;
    cache.baseline.set_len(size).map_err(io)?;
    write_all(&cache.control, &size.to_le_bytes(), 0)?;
    Ok(())
}
pub struct Remap(Mapping);
pub fn remap_owner(start: u64, length: u64) -> Result<Option<Remap>, Errno> {
    let maps = MAPPINGS.lock().unwrap();
    let Some(mapping) = maps.iter().find(|m| m.start <= start && start < m.end) else {
        return Ok(None);
    };
    if length != 0 && start.checked_add(length).ok_or(EINVAL)? > mapping.end {
        return Err(crate::errno::EFAULT);
    }
    Ok(Some(Remap(Mapping {
        start,
        offset: mapping.offset + start - mapping.start,
        ..mapping.clone()
    })))
}
pub fn remapped(owner: Remap, old: u64, old_len: u64, new: u64, new_len: u64, keep_old: bool) {
    forget(new, new_len);
    if !keep_old && old_len != 0 {
        forget(old, old_len);
    }
    MAPPINGS.lock().unwrap().push(Mapping {
        start: new,
        end: new + new_len,
        offset: owner.0.offset,
        shared: owner.0.shared,
        cache: owner.0.cache,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    struct Server {
        key: SessionKey,
        device: Option<OwnedFd>,
        broker: Option<std::thread::JoinHandle<Result<(), Errno>>>,
        daemon: Option<std::thread::JoinHandle<()>>,
        bytes: Arc<Mutex<Vec<u8>>>,
        fail: Arc<AtomicBool>,
        writes: Arc<AtomicU64>,
        released: Arc<AtomicBool>,
    }
    fn respond(fd: i32, request: &[u8], error: i32, body: &[u8]) {
        let mut out = Vec::new();
        out.extend(((16 + body.len()) as u32).to_le_bytes());
        out.extend(error.to_le_bytes());
        out.extend(&request[8..16]);
        out.extend(body);
        assert_eq!(
            super::super::fuse::write_device(fd, &out).unwrap(),
            out.len()
        );
    }
    fn next(fd: i32) -> Result<Vec<u8>, Errno> {
        let mut bytes = vec![0; super::super::fuse::MAX_MESSAGE];
        let count = super::super::fuse::read_device(fd, &mut bytes, false)?;
        bytes.truncate(count);
        Ok(bytes)
    }
    fn device(key: &SessionKey) -> OwnedFd {
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        assert!(fd >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let address = |path: &std::path::Path| {
            let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
            address.sun_family = libc::AF_UNIX as _;
            address.sun_len = std::mem::size_of_val(&address) as u8;
            assert!(path.as_os_str().as_bytes().len() < address.sun_path.len());
            for (dst, src) in address.sun_path.iter_mut().zip(path.as_os_str().as_bytes()) {
                *dst = *src as _;
            }
            address
        };
        let local = address(&key.transport().with_extension("d1"));
        let remote = address(key.transport());
        assert_eq!(
            unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    (&local as *const libc::sockaddr_un).cast(),
                    std::mem::size_of_val(&local) as _,
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                libc::connect(
                    fd.as_raw_fd(),
                    (&remote as *const libc::sockaddr_un).cast(),
                    std::mem::size_of_val(&remote) as _,
                )
            },
            0
        );
        let duplicate = unsafe { libc::dup(fd.as_raw_fd()) };
        assert!(duplicate >= 0);
        let mut stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(duplicate) };
        let mut frame = Vec::new();
        frame.extend(b"AIMFUSE1");
        frame.extend(9u32.to_le_bytes());
        frame.extend(1u64.to_le_bytes());
        frame.extend(0u32.to_le_bytes());
        stream.write_all(&frame).unwrap();
        let mut reply = [0; 8];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(i32::from_le_bytes(reply[..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(reply[4..].try_into().unwrap()), 0);
        let marker = libc::linger {
            l_onoff: 0,
            l_linger: super::super::fuse::DEVICE_MARKER,
        };
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_LINGER,
                    (&marker as *const libc::linger).cast(),
                    std::mem::size_of_val(&marker) as _,
                )
            },
            0
        );
        fd
    }
    impl Server {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "fc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&dir).unwrap();
            let key = SessionKey::from_transport(dir.join("c.sock"));
            let path = key.transport().to_owned();
            let broker = std::thread::spawn(move || super::super::fuse::serve_broker(&path));
            while !key.transport().exists() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let device = device(&key);
            let fd = device.as_raw_fd();
            super::super::fuse::Client::from_key(&key)
                .unwrap()
                .mount()
                .unwrap();
            let init = next(fd).unwrap();
            assert_eq!(u32::from_le_bytes(init[4..8].try_into().unwrap()), 26);
            let mut reply = vec![0; 64];
            reply[..4].copy_from_slice(&7u32.to_le_bytes());
            reply[4..8].copy_from_slice(&39u32.to_le_bytes());
            reply[20..24].copy_from_slice(&(CHUNK as u32).to_le_bytes());
            respond(fd, &init, 0, &reply);
            let bytes = Arc::new(Mutex::new(vec![b'a'; 16384]));
            let fail = Arc::new(AtomicBool::new(false));
            let writes = Arc::new(AtomicU64::new(0));
            let released = Arc::new(AtomicBool::new(false));
            let data = bytes.clone();
            let failures = fail.clone();
            let written = writes.clone();
            let release = released.clone();
            let daemon = std::thread::spawn(move || {
                while let Ok(packet) = next(fd) {
                    match u32::from_le_bytes(packet[4..8].try_into().unwrap()) {
                        3 => {
                            let mut body = vec![0; 104];
                            body[24..32].copy_from_slice(
                                &(data.lock().unwrap().len() as u64).to_le_bytes(),
                            );
                            respond(fd, &packet, 0, &body);
                        }
                        15 => {
                            assert_eq!(word(&packet, 40).unwrap(), 71);
                            let offset = word(&packet, 48).unwrap() as usize;
                            let size =
                                u32::from_le_bytes(packet[56..60].try_into().unwrap()) as usize;
                            let bytes = data.lock().unwrap();
                            respond(
                                fd,
                                &packet,
                                0,
                                &bytes[offset..(offset + size).min(bytes.len())],
                            );
                        }
                        16 => {
                            assert_eq!(word(&packet, 40).unwrap(), 71);
                            written.fetch_add(1, Ordering::SeqCst);
                            if failures.load(Ordering::SeqCst) {
                                respond(fd, &packet, -EIO, &[]);
                            } else {
                                let offset = word(&packet, 48).unwrap() as usize;
                                let size =
                                    u32::from_le_bytes(packet[56..60].try_into().unwrap()) as usize;
                                data.lock().unwrap()[offset..offset + size]
                                    .copy_from_slice(&packet[80..80 + size]);
                                let mut body = [0; 8];
                                body[..4].copy_from_slice(&(size as u32).to_le_bytes());
                                respond(fd, &packet, 0, &body);
                            }
                        }
                        18 => {
                            respond(fd, &packet, 0, &[]);
                            release.store(true, Ordering::SeqCst);
                        }
                        2 => {}
                        20 | 25 => respond(fd, &packet, 0, &[]),
                        opcode => panic!("unexpected FUSE opcode {opcode}"),
                    }
                }
            });
            Self {
                key,
                device: Some(device),
                broker: Some(broker),
                daemon: Some(daemon),
                bytes,
                fail,
                writes,
                released,
            }
        }
        fn file(&self) -> (OwnedFd, Arc<Open>) {
            let policy = super::super::fuse::MountPolicy {
                uid: 0,
                gid: 0,
                allow_other: true,
                default_permissions: false,
                read_only: false,
            };
            let fd = super::super::fuse::open_description(
                &self.key, 5, 71, 2, false, "f", "/test/f", 0, &policy, 0, 0, 1,
            )
            .unwrap();
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let file = Arc::new(Open {
                guest: "/test/f".into(),
                route: crate::vfs::FuseRoute {
                    session: self.key.transport().to_owned(),
                    relative: "f".into(),
                    uid: 0,
                    gid: 0,
                    allow_other: true,
                    default_permissions: false,
                    read_only: false,
                },
                node: 5,
                fh: 71,
                flags: 2,
                open_flags: 0,
                broker_owned: true,
                directory: false,
                offset: Mutex::new(0),
            });
            (fd, file)
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = shutdown_writeback();
            super::super::fuse::abort(&self.key).unwrap();
            self.device.take();
            self.daemon.take().unwrap().join().unwrap();
            self.broker.take().unwrap().join().unwrap().unwrap();
            fs::remove_dir_all(self.key.transport().parent().unwrap()).unwrap();
        }
    }
    fn wait(condition: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !condition() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn actual_server_async_writeback_retains_closed_fd_and_private_cow() {
        let server = Server::new();
        let (fd, file) = server.file();
        let cache = prepare(file.clone(), fd.as_raw_fd(), true, true).unwrap();
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                3,
                libc::MAP_SHARED,
                cache.fd(),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        note(cache.clone(), mapping as u64, 16384, 0, true).unwrap();
        let private = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                3,
                libc::MAP_PRIVATE,
                cache.fd(),
                0,
            )
        };
        assert_ne!(private, libc::MAP_FAILED);
        note(cache.clone(), private as u64, 16384, 0, false).unwrap();
        drop(fd);
        assert!(!server.released.load(Ordering::SeqCst));
        unsafe {
            *(private as *mut u8) = b'p';
            *(mapping as *mut u8).add(17) = b'z';
        }
        schedule_range(mapping as u64, 16384).unwrap();
        wait(|| server.bytes.lock().unwrap()[17] == b'z');
        assert_eq!(server.bytes.lock().unwrap()[0], b'a');
        shutdown_writeback().unwrap();
        forget(mapping as u64, 16384);
        forget(private as u64, 16384);
        unsafe {
            libc::munmap(mapping, 16384);
            libc::munmap(private, 16384);
        }
        drop(cache);
        wait(|| server.released.load(Ordering::SeqCst));
    }
    #[test]
    fn actual_server_writeback_error_is_reported_after_retry_and_acknowledged_once() {
        let server = Server::new();
        let (fd, file) = server.file();
        let cache = prepare(file.clone(), fd.as_raw_fd(), true, true).unwrap();
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                3,
                libc::MAP_SHARED,
                cache.fd(),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        note(cache.clone(), mapping as u64, 16384, 0, true).unwrap();
        server.fail.store(true, Ordering::SeqCst);
        unsafe {
            *(mapping as *mut u8) = b'z';
        }
        schedule_range(mapping as u64, 16384).unwrap();
        wait(|| cache.error_state().unwrap().0 > 0);
        assert!(server.writes.load(Ordering::SeqCst) > 0);
        assert_eq!(server.bytes.lock().unwrap()[0], b'a');
        server.fail.store(false, Ordering::SeqCst);
        assert_eq!(flush_file(&file), Err(EIO));
        assert_eq!(server.bytes.lock().unwrap()[0], b'z');
        assert_eq!(flush_file(&file), Ok(()));
        shutdown_writeback().unwrap();
        forget(mapping as u64, 16384);
        unsafe {
            libc::munmap(mapping, 16384);
        }
        drop(cache);
        drop(fd);
        wait(|| server.released.load(Ordering::SeqCst));
    }
    #[test]
    fn actual_notifications_refresh_pages_and_last_forget_evicts_old_inode() {
        let server = Server::new();
        let (fd, file) = server.file();
        let cache = prepare(file, fd.as_raw_fd(), true, true).unwrap();
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                3,
                libc::MAP_SHARED,
                cache.fd(),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        note(cache.clone(), mapping as u64, 16384, 0, true).unwrap();
        let notify = |node: u64, offset: i64, len: u64| {
            let mut packet = Vec::new();
            packet.extend(40u32.to_le_bytes());
            packet.extend(2i32.to_le_bytes());
            packet.extend(0u64.to_le_bytes());
            packet.extend(node.to_le_bytes());
            packet.extend(offset.to_le_bytes());
            packet.extend(len.to_le_bytes());
            assert_eq!(
                super::super::fuse::write_device(
                    server.device.as_ref().unwrap().as_raw_fd(),
                    &packet
                )
                .unwrap(),
                40
            );
        };
        // Attribute-only and uncached-inode notifications require no guessed
        // handle, including transient LOOKUPs which never produced a file fd.
        notify(999, -1, 0);
        notify(999, 0, 16384);
        drain_notifications(&server.key).unwrap();
        server.bytes.lock().unwrap()[3] = b'n';
        notify(5, 0, 16384);
        drain_notifications(&server.key).unwrap();
        wait(|| unsafe { *(mapping as *const u8).add(3) == b'n' });
        assert_eq!(server.writes.load(Ordering::SeqCst), 0);
        assert_eq!(evict_inode(&server.key, 5), Err(crate::errno::EBUSY));
        shutdown_writeback().unwrap();
        forget(mapping as u64, 16384);
        unsafe {
            libc::munmap(mapping, 16384);
        }
        drop(cache);
        drop(fd);
        wait(|| server.released.load(Ordering::SeqCst));
        evict_inode(&server.key, 5).unwrap();
        server.bytes.lock().unwrap()[3] = b'g';
        let (fd, file) = server.file();
        let fresh = prepare(file, fd.as_raw_fd(), false, false).unwrap();
        let mut bytes = [0; 4];
        read_exact(&fresh.data, &mut bytes, 0).unwrap();
        assert_eq!(bytes, b"aaag"[0..4]);
        drop(fresh);
        server.released.store(false, Ordering::SeqCst);
        drop(fd);
        wait(|| server.released.load(Ordering::SeqCst));
    }
    #[test]
    #[ignore = "subprocess helper; invoked with its actual broker key by the SIGKILL test"]
    fn sigkill_mmap_writer_helper() {
        let path = std::env::args()
            .find_map(|arg| arg.strip_prefix("fuse-child-input=").map(str::to_owned))
            .expect("actual broker key CLI argument");
        let key = SessionKey::from_transport(path.into());
        let policy = super::super::fuse::MountPolicy {
            uid: 0,
            gid: 0,
            allow_other: true,
            default_permissions: false,
            read_only: false,
        };
        let raw = super::super::fuse::open_description(
            &key,
            5,
            71,
            2,
            false,
            "f",
            "/test/f",
            0,
            &policy,
            0,
            0,
            std::process::id(),
        )
        .unwrap();
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let file = Arc::new(Open {
            guest: "/test/f".into(),
            route: crate::vfs::FuseRoute {
                session: key.transport().to_owned(),
                relative: "f".into(),
                uid: 0,
                gid: 0,
                allow_other: true,
                default_permissions: false,
                read_only: false,
            },
            node: 5,
            fh: 71,
            flags: 2,
            open_flags: 0,
            broker_owned: true,
            directory: false,
            offset: Mutex::new(0),
        });
        let cache = prepare(file, fd.as_raw_fd(), true, true).unwrap();
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                3,
                libc::MAP_SHARED,
                cache.fd(),
                0,
            )
        };
        assert_ne!(mapping, libc::MAP_FAILED);
        note(cache, mapping as u64, 16384, 0, true).unwrap();
        unsafe {
            *(mapping as *mut u8).add(99) = b'k';
        }
        println!("AIM_FUSE_DIRTY");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }
    #[test]
    fn actual_broker_writes_dirty_shared_inode_after_writer_sigkill_before_release() {
        use std::io::BufRead;
        use std::os::unix::process::ExitStatusExt;
        let server = Server::new();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sys::fuse_cache::tests::sigkill_mmap_writer_helper",
                "--ignored",
                "--nocapture",
            ])
            .arg(format!(
                "fuse-child-input={}",
                server.key.transport().display()
            ))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "writer helper exited without dirty mapping"
            );
            if line.contains("AIM_FUSE_DIRTY") {
                break;
            }
        }
        assert_eq!(server.bytes.lock().unwrap()[99], b'a');
        assert_eq!(server.writes.load(Ordering::SeqCst), 0);
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGKILL) }, 0);
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
        wait(|| server.released.load(Ordering::SeqCst));
        assert_eq!(server.bytes.lock().unwrap()[99], b'k');
        assert!(server.writes.load(Ordering::SeqCst) > 0);
    }
}
