//! Cross-process inode admission and open-description writer leases.
//! Admission is always acquired before the writer lock. Only final descriptor
//! close releases a lease: explicitly unlocking would revoke surviving aliases.
//!
//! A lease directory holds lock files by device, kind and shard
//! (`<dev>.writers.<shard>`, ...). An inode's lock of a kind is the byte at
//! its slot (a hash of its whole identity, so a reused inode number is
//! another inode) of the file of its shard (the slot's top byte), held as
//! an open-file-description lock: each lease opens the file for itself,
//! its lock goes with that open file (aliases, fork, SCM_RIGHTS) and ends
//! at its final close, as a flock would. The open file's position is the
//! slot, which tells what it holds. The kernel scans a file's locks on
//! every lock operation, so the 256 shards keep each list to a share of
//! the device's held leases. Exclusion is by slot, so two inodes whose
//! 62-bit slots collide (a chance of 2^-62 a pair) share their locks: a
//! writer of one is refused while the other is denied, and an admission
//! or offset lock of one waits for the other's.
use crate::private_fd::PrivateFd;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub generation: u32,
    pub birth_seconds: i64,
    pub birth_nanoseconds: i64,
}
impl Identity {
    pub fn from_fd(fd: BorrowedFd<'_>) -> io::Result<Self> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            dev: stat.st_dev as u32 as u64,
            ino: stat.st_ino,
            generation: stat.st_gen,
            birth_seconds: stat.st_birthtime,
            birth_nanoseconds: stat.st_birthtime_nsec,
        })
    }
    /// The lock byte of this identity in its device's lock files: the
    /// first 62 bits of a SHA-256 of all but the device.
    fn slot(self) -> i64 {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(&self.to_bytes()[8..]);
        (u64::from_le_bytes(digest[..8].try_into().unwrap()) >> 2) as i64
    }
    pub fn name(self) -> String {
        format!(
            "{:x}-{:x}-{:x}-{:x}-{:x}",
            self.dev, self.ino, self.generation, self.birth_seconds, self.birth_nanoseconds
        )
    }
    pub fn to_bytes(self) -> [u8; 36] {
        let mut bytes = [0; 36];
        bytes[..8].copy_from_slice(&self.dev.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.ino.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.generation.to_le_bytes());
        bytes[20..28].copy_from_slice(&self.birth_seconds.to_le_bytes());
        bytes[28..36].copy_from_slice(&self.birth_nanoseconds.to_le_bytes());
        bytes
    }
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != 36 {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let identity = Self {
            dev: u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            ino: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            generation: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            birth_seconds: i64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            birth_nanoseconds: i64::from_le_bytes(bytes[28..36].try_into().unwrap()),
        };
        if identity.dev > u32::MAX as u64
            || !(0..1_000_000_000).contains(&identity.birth_nanoseconds)
        {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        Ok(identity)
    }
}
struct Inner {
    source: PrivateFd,
    directory: Arc<LeaseDirectory>,
    identity: Identity,
    offset: OffsetFile,
}
#[derive(Clone)]
pub struct Inode(Arc<Inner>);
/// A lease directory, opened once per process and shared by its inode
/// handles while the path still names it.
struct LeaseDirectory {
    /// As the caller names it.
    given: PathBuf,
    /// Canonical.
    path: PathBuf,
    descriptor: PrivateFd,
    node: (libc::dev_t, libc::ino_t),
}
static DIRECTORIES: Mutex<Vec<Arc<LeaseDirectory>>> = Mutex::new(Vec::new());
pub struct Admission {
    inode: Inode,
    _metadata: PrivateFd,
}
/// The offset lock file, opened once per handle instead of for each
/// position-dependent operation: its flock excludes other processes and
/// other handles of the inode, which have their own open file, and `held`
/// excludes this handle's threads, which share the one open file and so its
/// flock.
struct OffsetFile {
    descriptor: std::sync::OnceLock<PrivateFd>,
    held: Mutex<bool>,
    released: Condvar,
}
/// Serializes position-dependent operations until dropped.
pub struct OffsetLock {
    inode: Inode,
}
pub struct WriterLease {
    inode: Inode,
    descriptor: PrivateFd,
}
pub struct ExclusiveLease {
    inode: Inode,
    descriptor: PrivateFd,
}
pub type EnableSlot = ExclusiveLease;
fn duplicate(fd: BorrowedFd<'_>) -> io::Result<PrivateFd> {
    PrivateFd::allocate(|| {
        let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(result) })
        }
    })
}
/// The lease directory `given` names, reusing this process's open one while
/// `given` still names it: an open of a file then costs one lstat here
/// instead of creating, canonicalizing and opening the directory.
fn lease_directory(given: &Path) -> io::Result<Arc<LeaseDirectory>> {
    let node = match fs::symlink_metadata(given) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        Ok(metadata) => {
            use std::os::unix::fs::MetadataExt;
            Some((metadata.dev() as libc::dev_t, metadata.ino() as libc::ino_t))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut directories = DIRECTORIES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = directories
        .iter()
        .find(|known| known.given == given && Some(known.node) == node)
    {
        return Ok(known.clone());
    }
    fs::create_dir_all(given)?;
    if fs::symlink_metadata(given)?.file_type().is_symlink() {
        return Err(io::Error::from_raw_os_error(libc::ELOOP));
    }
    let path = fs::canonicalize(given)?;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let descriptor = PrivateFd::allocate(|| {
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    })?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(descriptor.as_raw_fd(), &mut stat) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let directory = Arc::new(LeaseDirectory {
        given: given.to_path_buf(),
        path,
        descriptor,
        node: (stat.st_dev, stat.st_ino),
    });
    directories.retain(|known| known.given != given && Arc::strong_count(known) > 1);
    directories.push(directory.clone());
    Ok(directory)
}
/// Darwin's open-file-description locks (sys/fcntl.h, private).
const F_OFD_SETLK: i32 = 90;
const F_OFD_SETLKW: i32 = 91;

/// `flock(fd, operation)` of the byte at `slot`: LOCK_SH, LOCK_EX or
/// LOCK_UN, with LOCK_NB answering EBUSY where another open file holds it.
fn locked(fd: BorrowedFd<'_>, slot: i64, operation: i32) -> io::Result<()> {
    let mut range: libc::flock = unsafe { std::mem::zeroed() };
    range.l_type = match operation & !libc::LOCK_NB {
        libc::LOCK_SH => libc::F_RDLCK,
        libc::LOCK_EX => libc::F_WRLCK,
        _ => libc::F_UNLCK,
    };
    range.l_whence = libc::SEEK_SET as i16;
    range.l_start = slot;
    range.l_len = 1;
    let command = if operation & libc::LOCK_NB != 0 {
        F_OFD_SETLK
    } else {
        F_OFD_SETLKW
    };
    loop {
        if unsafe { libc::fcntl(fd.as_raw_fd(), command, &range) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN | libc::EACCES) if command == F_OFD_SETLK => {
                return Err(io::Error::from_raw_os_error(libc::EBUSY));
            }
            _ => return Err(error),
        }
    }
}
impl Inode {
    pub fn open(directory: &Path, source: BorrowedFd<'_>) -> io::Result<Self> {
        let source = duplicate(source)?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(source.as_raw_fd(), &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let identity = Identity::from_fd(source.as_fd())?;
        let directory = lease_directory(directory)?;
        Ok(Self(Arc::new(Inner {
            source,
            directory,
            identity,
            offset: OffsetFile {
                descriptor: std::sync::OnceLock::new(),
                held: Mutex::new(false),
                released: Condvar::new(),
            },
        })))
    }
    pub fn identity(&self) -> Identity {
        self.0.identity
    }
    pub fn source(&self) -> BorrowedFd<'_> {
        self.0.source.as_fd()
    }
    pub fn directory(&self) -> &Path {
        &self.0.directory.path
    }
    /// An open file of the `kind` lock file of this inode's device and
    /// shard, at its slot.
    fn lock_file(&self, kind: &str) -> io::Result<PrivateFd> {
        let identity = self.identity();
        let name = std::ffi::CString::new(format!(
            "{:x}.{kind}.{:02x}",
            identity.dev,
            identity.slot() >> 54
        ))
        .unwrap();
        let fd = PrivateFd::allocate(|| {
            // Opens racing an O_CREAT of the same new name can fail with
            // ENOENT on Darwin: only one creates it, the others open it.
            let directory = self.0.directory.descriptor.as_raw_fd();
            let flags = libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let mut fd = unsafe { libc::openat(directory, name.as_ptr(), flags) };
            if fd < 0 && io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
                fd = unsafe {
                    libc::openat(
                        directory,
                        name.as_ptr(),
                        flags | libc::O_CREAT | libc::O_EXCL,
                        0o600,
                    )
                };
                if fd < 0 && io::Error::last_os_error().kind() == io::ErrorKind::AlreadyExists {
                    fd = unsafe { libc::openat(directory, name.as_ptr(), flags) };
                }
            }
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        })?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        if unsafe { libc::lseek(fd.as_raw_fd(), self.slot(), libc::SEEK_SET) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }
    fn slot(&self) -> i64 {
        self.identity().slot()
    }
    pub fn admission(&self) -> io::Result<Admission> {
        if Identity::from_fd(self.source())? != self.identity() {
            return Err(io::Error::from_raw_os_error(libc::ESTALE));
        }
        let metadata = self.lock_file("admission")?;
        locked(metadata.as_fd(), self.slot(), libc::LOCK_EX)?;
        Ok(Admission {
            inode: self.clone(),
            _metadata: metadata,
        })
    }
    /// Open the offset lock file now rather than at the first
    /// position-dependent operation, so that running out of descriptors
    /// fails the open of the file, not a later read or write.
    pub fn keep_offset_lock(&self) -> io::Result<()> {
        self.offset_file().map(|_| ())
    }
    fn offset_file(&self) -> io::Result<&PrivateFd> {
        if let Some(descriptor) = self.0.offset.descriptor.get() {
            return Ok(descriptor);
        }
        let opened = self.lock_file("offset")?;
        Ok(self.0.offset.descriptor.get_or_init(|| opened))
    }
    pub fn offset_lock(&self) -> io::Result<OffsetLock> {
        if Identity::from_fd(self.source())? != self.identity() {
            return Err(io::Error::from_raw_os_error(libc::ESTALE));
        }
        let descriptor = self.offset_file()?;
        let offset = &self.0.offset;
        let mut held = offset.held.lock().unwrap_or_else(|e| e.into_inner());
        while *held {
            held = offset
                .released
                .wait(held)
                .unwrap_or_else(|e| e.into_inner());
        }
        *held = true;
        drop(held);
        if let Err(error) = locked(descriptor.as_fd(), self.slot(), libc::LOCK_EX) {
            *offset.held.lock().unwrap_or_else(|e| e.into_inner()) = false;
            offset.released.notify_one();
            return Err(error);
        }
        Ok(OffsetLock {
            inode: self.clone(),
        })
    }
    /// Adopt the actual carrier under admission and verify that it is an
    /// open file of the writers lock file at this inode's slot.
    pub fn adopt_writer(&self, carrier: OwnedFd) -> io::Result<WriterLease> {
        self.adopt_private_writer(PrivateFd::adopt(carrier)?)
    }
    pub fn adopt_private_writer(&self, carrier: PrivateFd) -> io::Result<WriterLease> {
        let admission = self.admission()?;
        let expected = self.lock_file("writers")?;
        if Identity::from_fd(carrier.as_fd())? != Identity::from_fd(expected.as_fd())?
            || unsafe { libc::lseek(carrier.as_raw_fd(), 0, libc::SEEK_CUR) } != self.slot()
        {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let denial = admission.inode.lock_file("denials")?;
        locked(denial.as_fd(), self.slot(), libc::LOCK_EX | libc::LOCK_NB)?;
        locked(carrier.as_fd(), self.slot(), libc::LOCK_SH | libc::LOCK_NB)?;
        Ok(WriterLease {
            inode: admission.inode.clone(),
            descriptor: carrier,
        })
    }
}
impl Admission {
    pub fn identity(&self) -> Identity {
        self.inode.identity()
    }
    pub fn source(&self) -> BorrowedFd<'_> {
        self.inode.source()
    }
    pub fn writer(&self) -> io::Result<WriterLease> {
        let denial = self.inode.lock_file("denials")?;
        locked(
            denial.as_fd(),
            self.inode.slot(),
            libc::LOCK_EX | libc::LOCK_NB,
        )?;
        let descriptor = self.inode.lock_file("writers")?;
        locked(
            descriptor.as_fd(),
            self.inode.slot(),
            libc::LOCK_SH | libc::LOCK_NB,
        )?;
        Ok(WriterLease {
            inode: self.inode.clone(),
            descriptor,
        })
    }
    /// Linux deny_write_access permits concurrent readers while denying writers.
    /// Admission serializes the probe and retained denial with writer admission.
    pub fn deny_writers(&self) -> io::Result<ExclusiveLease> {
        let writers = self.inode.lock_file("writers")?;
        locked(
            writers.as_fd(),
            self.inode.slot(),
            libc::LOCK_EX | libc::LOCK_NB,
        )?;
        let descriptor = self.inode.lock_file("denials")?;
        locked(
            descriptor.as_fd(),
            self.inode.slot(),
            libc::LOCK_SH | libc::LOCK_NB,
        )?;
        Ok(ExclusiveLease {
            inode: self.inode.clone(),
            descriptor,
        })
    }
    pub fn enable_slot(&self) -> io::Result<EnableSlot> {
        let descriptor = self.inode.lock_file("enable")?;
        locked(
            descriptor.as_fd(),
            self.inode.slot(),
            libc::LOCK_EX | libc::LOCK_NB,
        )?;
        Ok(ExclusiveLease {
            inode: self.inode.clone(),
            descriptor,
        })
    }
    pub fn exclusive(&self) -> io::Result<ExclusiveLease> {
        let descriptor = self.inode.lock_file("writers")?;
        locked(
            descriptor.as_fd(),
            self.inode.slot(),
            libc::LOCK_EX | libc::LOCK_NB,
        )?;
        Ok(ExclusiveLease {
            inode: self.inode.clone(),
            descriptor,
        })
    }
}
impl WriterLease {
    pub fn identity(&self) -> Identity {
        self.inode.identity()
    }
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inode: self.inode.clone(),
            descriptor: self.descriptor.try_clone()?,
        })
    }
    pub fn into_private_fd(self) -> PrivateFd {
        self.descriptor
    }
    pub fn into_fd(self) -> io::Result<OwnedFd> {
        self.descriptor.into_fd()
    }
}
impl Drop for OffsetLock {
    fn drop(&mut self) {
        let offset = &self.inode.0.offset;
        let mut held = offset.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(descriptor) = offset.descriptor.get() {
            // An unlock of a lock this open file holds cannot block.
            if let Err(error) = locked(descriptor.as_fd(), self.inode.slot(), libc::LOCK_UN) {
                eprintln!(
                    "aim-storage: unlocking the offset lock of {}: {error}",
                    self.inode.identity().name()
                );
            }
        }
        *held = false;
        offset.released.notify_one();
    }
}
impl ExclusiveLease {
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inode: self.inode.clone(),
            descriptor: self.descriptor.try_clone()?,
        })
    }
    pub fn identity(&self) -> Identity {
        self.inode.identity()
    }
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    struct Fixture {
        path: PathBuf,
        file: File,
        inode: Inode,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-inode-lease-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path.join("source"))
                .unwrap();
            let inode = Inode::open(&path.join("locks"), file.as_fd()).unwrap();
            Self { path, file, inode }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }
    pub(crate) fn isolated_fork_fixture(name: &str, marker: &str) -> bool {
        if std::env::args().any(|arg| arg == marker) {
            return false;
        }
        use std::{ffi::CString, io::Read, os::unix::ffi::OsStrExt};
        let executable =
            CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();
        let arguments = [
            executable.clone(),
            CString::new("--exact").unwrap(),
            CString::new(name).unwrap(),
            CString::new("--skip").unwrap(),
            CString::new(marker).unwrap(),
            CString::new("--nocapture").unwrap(),
        ];
        let mut argv = arguments
            .iter()
            .map(|arg| arg.as_ptr() as *mut libc::c_char)
            .collect::<Vec<_>>();
        argv.push(std::ptr::null_mut());
        let environment = [std::ptr::null_mut::<libc::c_char>()];
        let output = pipe();
        let mut pid = 0;
        struct Spawn {
            attributes: libc::posix_spawnattr_t,
            actions: libc::posix_spawn_file_actions_t,
        }
        impl Drop for Spawn {
            fn drop(&mut self) {
                unsafe {
                    if !self.actions.is_null() {
                        libc::posix_spawn_file_actions_destroy(&mut self.actions);
                    }
                    if !self.attributes.is_null() {
                        libc::posix_spawnattr_destroy(&mut self.attributes);
                    }
                }
            }
        }
        let mut spawn = Spawn {
            attributes: std::ptr::null_mut(),
            actions: std::ptr::null_mut(),
        };
        // Darwin's default excludes foreign descriptors before the child starts,
        // including non-CLOEXEC recvmsg aliases held by concurrent fixtures.
        unsafe {
            assert_eq!(libc::posix_spawnattr_init(&mut spawn.attributes), 0);
            assert_eq!(libc::posix_spawn_file_actions_init(&mut spawn.actions), 0);
            assert_eq!(
                libc::posix_spawnattr_setflags(
                    &mut spawn.attributes,
                    libc::POSIX_SPAWN_CLOEXEC_DEFAULT as i16
                ),
                0
            );
            assert_eq!(
                libc::posix_spawn_file_actions_adddup2(
                    &mut spawn.actions,
                    output[1].as_raw_fd(),
                    1
                ),
                0
            );
            assert_eq!(
                libc::posix_spawn_file_actions_adddup2(
                    &mut spawn.actions,
                    output[1].as_raw_fd(),
                    2
                ),
                0
            );
            assert_eq!(
                libc::posix_spawn(
                    &mut pid,
                    executable.as_ptr(),
                    &spawn.actions,
                    &spawn.attributes,
                    argv.as_ptr(),
                    environment.as_ptr()
                ),
                0
            );
        }
        drop(spawn);
        let mut child = OwnedChild(Some(pid));
        let [reader, writer] = output;
        drop(writer);
        let mut bytes = vec![];
        File::from(reader).read_to_end(&mut bytes).unwrap();
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        child.0 = None;
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "isolated fork fixture failed: {text}"
        );
        assert!(
            text.contains(marker),
            "isolated fork fixture did not execute: {text}"
        );
        true
    }
    struct OwnedChild(Option<i32>);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Some(pid) = self.0 {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, std::ptr::null_mut(), 0);
                }
            }
        }
    }
    fn busy(result: io::Result<ExclusiveLease>) {
        assert_eq!(result.err().unwrap().raw_os_error(), Some(libc::EBUSY));
    }
    #[test]
    fn actual_identity_round_trip_rejects_invalid_fields_and_aliases_keep_lock() {
        const MARKER: &str = "INODE_ALIAS_FINAL_CLOSE_FIXTURE_EXECUTED";
        if isolated_fork_fixture(
            "inode_lease::tests::actual_identity_round_trip_rejects_invalid_fields_and_aliases_keep_lock",
            MARKER,
        ) {
            return;
        }
        let fixture = Fixture::new();
        let directory = File::open(&fixture.path).unwrap();
        assert_eq!(
            Inode::open(fixture.inode.directory(), directory.as_fd())
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        let socket =
            unsafe { OwnedFd::from_raw_fd(libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0)) };
        assert_eq!(
            Inode::open(fixture.inode.directory(), socket.as_fd())
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        let identity = Identity::from_fd(fixture.file.as_fd()).unwrap();
        assert_eq!(identity, fixture.inode.identity());
        assert_eq!(
            Identity::from_bytes(&identity.to_bytes()).unwrap(),
            identity
        );
        let mut synthetic = identity;
        synthetic.dev = u32::MAX as u64;
        assert_eq!(
            Identity::from_bytes(&synthetic.to_bytes()).unwrap().dev,
            (-1i32 as u32) as u64
        );
        synthetic.dev = u32::MAX as u64 + 1;
        assert!(Identity::from_bytes(&synthetic.to_bytes()).is_err());
        synthetic = identity;
        synthetic.birth_nanoseconds = 1_000_000_000;
        assert!(Identity::from_bytes(&synthetic.to_bytes()).is_err());
        let writer = fixture.inode.admission().unwrap().writer().unwrap();
        let alias = writer.try_clone().unwrap();
        drop(writer);
        busy(fixture.inode.admission().unwrap().exclusive());
        let carrier = alias.into_private_fd();
        let writer = fixture.inode.adopt_private_writer(carrier).unwrap();
        busy(fixture.inode.admission().unwrap().exclusive());
        drop(writer);
        assert!(fixture.inode.admission().unwrap().exclusive().is_ok());
        assert!(
            fixture
                .inode
                .adopt_private_writer(duplicate(fixture.file.as_fd()).unwrap())
                .is_err()
        );
        println!("{MARKER}");
    }
    #[test]
    fn exclusive_enable_build_never_blocks_a_writer_holding_admission() {
        const MARKER: &str = "INODE_EXCLUSIVE_FINAL_ALIAS_FIXTURE_EXECUTED";
        if isolated_fork_fixture(
            "inode_lease::tests::exclusive_enable_build_never_blocks_a_writer_holding_admission",
            MARKER,
        ) {
            return;
        }
        let fixture = Fixture::new();
        let admission = fixture.inode.admission().unwrap();
        let exclusive = admission.exclusive().unwrap();
        let slot = admission.enable_slot().unwrap();
        drop(admission);
        let admission = fixture.inode.admission().unwrap();
        assert_eq!(
            admission.writer().err().unwrap().raw_os_error(),
            Some(libc::EBUSY)
        );
        busy(admission.enable_slot());
        drop(admission);
        let reacquired = fixture.inode.admission().unwrap();
        drop(reacquired);
        drop(slot);
        drop(exclusive);
        assert!(fixture.inode.admission().unwrap().writer().is_ok());
        println!("{MARKER}");
    }
    fn byte(fd: i32, value: u8) {
        assert_eq!(
            unsafe { libc::write(fd, (&value as *const u8).cast(), 1) },
            1
        );
    }
    fn read_byte(fd: i32) -> u8 {
        let mut event = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut event, 1, 2000) },
            1,
            "owned child did not produce a receipt"
        );
        let mut value = 0;
        assert_eq!(
            unsafe { libc::read(fd, (&mut value as *mut u8).cast(), 1) },
            1
        );
        value
    }
    fn pipe() -> [OwnedFd; 2] {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { [OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])] }
    }
    #[test]
    fn readonly_enablers_share_writer_denial_but_not_enable_slot() {
        const MARKER: &str = "INODE_READONLY_FINAL_ALIAS_FIXTURE_EXECUTED";
        if isolated_fork_fixture(
            "inode_lease::tests::readonly_enablers_share_writer_denial_but_not_enable_slot",
            MARKER,
        ) {
            return;
        }
        let fixture = Fixture::new();
        let admission = fixture.inode.admission().unwrap();
        let first = admission.deny_writers().unwrap();
        let slot = admission.enable_slot().unwrap();
        drop(admission);
        let admission = fixture.inode.admission().unwrap();
        let second = admission.deny_writers().unwrap();
        busy(admission.enable_slot());
        assert_eq!(
            admission.writer().err().unwrap().raw_os_error(),
            Some(libc::EBUSY)
        );
        drop(admission);
        let alias = second.try_clone().unwrap();
        drop(first);
        drop(second);
        drop(slot);
        assert_eq!(
            fixture
                .inode
                .admission()
                .unwrap()
                .writer()
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EBUSY)
        );
        drop(alias);
        let writer = fixture.inode.admission().unwrap().writer().unwrap();
        busy(fixture.inode.admission().unwrap().deny_writers());
        drop(writer);
        assert!(fixture.inode.admission().unwrap().deny_writers().is_ok());
        println!("{MARKER}");
    }
    #[test]
    fn actual_child_exit_and_crash_release_writer_only_on_final_close() {
        const MARKER: &str = "INODE_WRITER_FORK_FIXTURE_EXECUTED";
        if isolated_fork_fixture(
            "inode_lease::tests::actual_child_exit_and_crash_release_writer_only_on_final_close",
            MARKER,
        ) {
            return;
        }
        for crash in [false, true] {
            let fixture = Fixture::new();
            let ready = pipe();
            let exit = pipe();
            let child = unsafe { libc::fork() };
            assert!(child >= 0);
            if child == 0 {
                let writer = match fixture
                    .inode
                    .admission()
                    .and_then(|admission| admission.writer())
                {
                    Ok(writer) => writer,
                    Err(_) => unsafe { libc::_exit(31) },
                };
                let alias = match writer.try_clone() {
                    Ok(alias) => alias,
                    Err(_) => unsafe { libc::_exit(32) },
                };
                drop(writer);
                unsafe {
                    libc::write(ready[1].as_raw_fd(), b"r".as_ptr().cast(), 1);
                }
                let mut command = 0u8;
                unsafe {
                    libc::read(exit[0].as_raw_fd(), (&mut command as *mut u8).cast(), 1);
                }
                let _live = alias;
                unsafe { libc::_exit(0) }
            }
            let mut owned_child = OwnedChild(Some(child));
            assert_eq!(read_byte(ready[0].as_raw_fd()), b'r');
            busy(fixture.inode.admission().unwrap().exclusive());
            if crash {
                assert_eq!(unsafe { libc::kill(child, libc::SIGKILL) }, 0);
            } else {
                byte(exit[1].as_raw_fd(), b'e');
            }
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
            owned_child.0 = None;
            if crash {
                assert!(libc::WIFSIGNALED(status));
                assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
            } else {
                assert!(libc::WIFEXITED(status));
                assert_eq!(libc::WEXITSTATUS(status), 0);
            }
            assert!(fixture.inode.admission().unwrap().exclusive().is_ok());
        }
        println!("{MARKER}");
    }
    #[test]
    fn scm_carrier_keeps_actual_open_description_after_sender_close() {
        const MARKER: &str = "INODE_SCM_LIFETIME_FIXTURE_EXECUTED";
        if isolated_fork_fixture(
            "inode_lease::tests::scm_carrier_keeps_actual_open_description_after_sender_close",
            MARKER,
        ) {
            return;
        }
        let fixture = Fixture::new();
        let writer = fixture.inode.admission().unwrap().writer().unwrap();
        let mut sockets = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sockets.as_mut_ptr()) },
            0
        );
        let sender = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
        let receiver = unsafe { OwnedFd::from_raw_fd(sockets[1]) };
        let mut value = 1u8;
        let mut iov = libc::iovec {
            iov_base: (&mut value as *mut u8).cast(),
            iov_len: 1,
        };
        let size = unsafe { libc::CMSG_SPACE(4) } as usize;
        let mut control = vec![0u8; size];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = size as _;
        let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
        unsafe {
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(4);
            (libc::CMSG_DATA(header) as *mut i32).write_unaligned(writer.descriptor().as_raw_fd());
        }
        assert_eq!(unsafe { libc::sendmsg(sender.as_raw_fd(), &message, 0) }, 1);
        drop(writer);
        busy(fixture.inode.admission().unwrap().exclusive());
        control.fill(0);
        message.msg_controllen = size as _;
        assert_eq!(
            unsafe { libc::recvmsg(receiver.as_raw_fd(), &mut message, 0) },
            1
        );
        let fd = unsafe {
            (libc::CMSG_DATA(libc::CMSG_FIRSTHDR(&message)) as *const i32).read_unaligned()
        };
        let lease = fixture
            .inode
            .adopt_writer(unsafe { OwnedFd::from_raw_fd(fd) })
            .unwrap();
        busy(fixture.inode.admission().unwrap().exclusive());
        drop(lease);
        assert!(fixture.inode.admission().unwrap().exclusive().is_ok());
        println!("{MARKER}");
    }
    #[test]
    fn offset_lock_parallel_readers_open_same_node_and_preserve_shared_position() {
        use std::io::{Read, Seek, Write};
        for precreated in [false, true] {
            let mut fixture = Fixture::new();
            let bytes = (0u64..128).flat_map(u64::to_le_bytes).collect::<Vec<_>>();
            fixture.file.write_all(&bytes).unwrap();
            fixture.file.rewind().unwrap();
            if precreated {
                drop(fixture.inode.offset_lock().unwrap());
            }
            let barrier = Arc::new(std::sync::Barrier::new(4));
            let mut workers = Vec::new();
            for _ in 0..4 {
                let inode = fixture.inode.clone();
                let mut source = fixture.file.try_clone().unwrap();
                let barrier = barrier.clone();
                workers.push(std::thread::spawn(move || -> io::Result<Vec<u64>> {
                    barrier.wait();
                    let mut values = Vec::new();
                    loop {
                        let _lock = inode.offset_lock()?;
                        let mut bytes = [0u8; 8];
                        let count = source.read(&mut bytes)?;
                        if count == 0 {
                            break;
                        }
                        if count != 8 {
                            return Err(io::Error::from_raw_os_error(libc::EIO));
                        }
                        values.push(u64::from_le_bytes(bytes));
                    }
                    Ok(values)
                }));
            }
            let results = workers
                .into_iter()
                .map(|worker| worker.join())
                .collect::<Vec<_>>();
            let mut values = Vec::new();
            for result in results {
                values.extend(result.unwrap().unwrap());
            }
            values.sort_unstable();
            assert_eq!(
                values,
                (0u64..128).collect::<Vec<_>>(),
                "precreated={precreated}"
            );
        }
    }
    #[test]
    fn offset_lock_serializes_processes() {
        const MARKER: &str = "INODE_OFFSET_FORK_FIXTURE_EXECUTED";
        if isolated_fork_fixture(
            "inode_lease::tests::offset_lock_serializes_processes",
            MARKER,
        ) {
            return;
        }
        let fixture = Fixture::new();
        let lock = fixture.inode.offset_lock().unwrap();
        let ready = pipe();
        let acquired = pipe();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            // The child's own handle of the inode, as a fork child restores
            // its open files; the parent's handle and lock stay its own.
            std::mem::forget(lock);
            let inode = match Inode::open(fixture.inode.directory(), fixture.file.as_fd()) {
                Ok(inode) => inode,
                Err(_) => unsafe { libc::_exit(40) },
            };
            byte(ready[1].as_raw_fd(), b'r');
            let _lock = match inode.offset_lock() {
                Ok(lock) => lock,
                Err(_) => unsafe { libc::_exit(41) },
            };
            byte(acquired[1].as_raw_fd(), b'a');
            unsafe { libc::_exit(0) }
        }
        let mut owned_child = OwnedChild(Some(child));
        assert_eq!(read_byte(ready[0].as_raw_fd()), b'r');
        let mut event = libc::pollfd {
            fd: acquired[0].as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut event, 1, 100) }, 0);
        drop(lock);
        assert_eq!(unsafe { libc::poll(&mut event, 1, 2000) }, 1);
        assert_eq!(read_byte(acquired[0].as_raw_fd()), b'a');
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        owned_child.0 = None;
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 0);
        assert!(fixture.inode.offset_lock().is_ok());
        println!("{MARKER}");
    }
    /// Handles share one open lease directory while its path names it, and
    /// open the directory that replaced it once it does not.
    #[test]
    fn inodes_share_their_lease_directory_while_its_path_names_it() {
        let fixture = Fixture::new();
        let locks = fixture.path.join("locks");
        let other = Inode::open(&locks, fixture.file.as_fd()).unwrap();
        assert!(Arc::ptr_eq(&fixture.inode.0.directory, &other.0.directory));
        fs::rename(&locks, fixture.path.join("replaced")).unwrap();
        fs::create_dir(&locks).unwrap();
        let replacement = Inode::open(&locks, fixture.file.as_fd()).unwrap();
        assert!(!Arc::ptr_eq(
            &fixture.inode.0.directory,
            &replacement.0.directory
        ));
        drop(replacement.admission().unwrap());
        assert!(fs::read_dir(&locks).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".admission.")
        }));
    }
    /// Inodes of one device share its lock files but not their locks: a
    /// reused inode number is another identity, and a writer carrier of
    /// one inode is no carrier of another.
    #[test]
    fn inodes_of_a_device_lock_their_own_slots() {
        let fixture = Fixture::new();
        let other = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(fixture.path.join("other"))
            .unwrap();
        let other = Inode::open(fixture.inode.directory(), other.as_fd()).unwrap();
        let writer = fixture.inode.admission().unwrap().writer().unwrap();
        let exclusive = other.admission().unwrap().exclusive().unwrap();
        busy(fixture.inode.admission().unwrap().exclusive());
        assert_eq!(
            other
                .admission()
                .unwrap()
                .writer()
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EBUSY)
        );
        assert_eq!(
            other
                .adopt_private_writer(writer.try_clone().unwrap().into_private_fd())
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        drop((writer, exclusive));
        let mut reused = fixture.inode.identity();
        reused.birth_nanoseconds = (reused.birth_nanoseconds + 1) % 1_000_000_000;
        assert_ne!(reused.slot(), fixture.inode.identity().slot());
        let mut names = fs::read_dir(fixture.inode.directory())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        let dev = fixture.inode.identity().dev;
        let mut expected = [&fixture.inode, &other]
            .into_iter()
            .flat_map(|inode| {
                let shard = inode.identity().slot() >> 54;
                ["admission", "denials", "writers"]
                    .map(|kind| format!("{dev:x}.{kind}.{shard:02x}"))
            })
            .collect::<Vec<_>>();
        expected.sort();
        expected.dedup();
        assert_eq!(names, expected);
    }
    #[test]
    fn canonical_directory_and_lock_nodes_reject_symlink_redirection() {
        let fixture = Fixture::new();
        let alias = fixture.path.join("alias");
        std::os::unix::fs::symlink(fixture.inode.directory(), &alias).unwrap();
        assert_eq!(
            Inode::open(&alias, fixture.file.as_fd())
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::ELOOP)
        );
        let target = fixture.inode.directory().join(format!(
            "{:x}.writers.{:02x}",
            fixture.inode.identity().dev,
            fixture.inode.identity().slot() >> 54
        ));
        std::os::unix::fs::symlink(fixture.path.join("source"), &target).unwrap();
        assert!(fixture.inode.admission().unwrap().writer().is_err());
    }
}
