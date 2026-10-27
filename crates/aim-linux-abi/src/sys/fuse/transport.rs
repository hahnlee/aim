//! Host transport lifetime follows the broker, independently of its opener.
use super::*;
use std::os::unix::{ffi::OsStrExt, fs::DirBuilderExt, fs::MetadataExt, fs::OpenOptionsExt};

pub(super) struct Directory {
    pub path: PathBuf,
    holder: Option<Lease>,
    identity: (u64, u64),
}

struct Lease(fs::File);
impl std::ops::Deref for Lease {
    type Target = fs::File;
    fn deref(&self) -> &fs::File {
        &self.0
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        super::super::fdtab::unhide(self.0.as_raw_fd());
    }
}

fn open_directory(path: &Path) -> Result<Lease, Errno> {
    use std::os::fd::{FromRawFd, IntoRawFd};
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(io_error)?;
    Ok(Lease(unsafe {
        fs::File::from_raw_fd(super::super::fdtab::hide(file.into_raw_fd()))
    }))
}

fn lock(file: &fs::File, operation: i32) -> Result<(), Errno> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = errno::last();
        if error != crate::errno::EINTR {
            return Err(error);
        }
    }
}

impl Directory {
    pub fn hold(path: &Path) -> Result<Self, Errno> {
        let holder = open_directory(path)?;
        lock(&holder, libc::LOCK_SH)?;
        Self::new(path.to_owned(), holder)
    }

    fn new(path: PathBuf, holder: Lease) -> Result<Self, Errno> {
        let metadata = holder.metadata().map_err(io_error)?;
        Ok(Self {
            path,
            holder: Some(holder),
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    pub fn release(&mut self) {
        self.holder.take();
    }

    fn remove(&self) -> Result<(), Errno> {
        match fs::symlink_metadata(&self.path) {
            Ok(named) if named.is_dir() && (named.dev(), named.ino()) == self.identity => {
                match fs::remove_dir_all(&self.path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(io_error(error)),
                }
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("owned FUSE transport cleanup: errno {error}");
        }
    }
}

pub(super) fn listen(path: &Path) -> Result<(std::os::unix::net::UnixListener, Directory), Errno> {
    let parent = path.parent().ok_or(EINVAL)?;
    let holder = open_directory(parent)?;
    lock(&holder, libc::LOCK_SH)?;
    let listener = std::os::unix::net::UnixListener::bind(path).map_err(io_error)?;
    Ok((listener, Directory::new(parent.to_owned(), holder)?))
}

fn socket_gone(path: &Path) -> bool {
    use std::os::fd::{FromRawFd, OwnedFd};
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() {
        return false;
    }
    address.sun_family = libc::AF_UNIX as _;
    address.sun_len = std::mem::size_of_val(&address) as _;
    for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
        *dst = *src as _;
    }
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return false;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // A full live listener backlog must never stall stale-session collection.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
        return false;
    }
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    };
    result < 0
        && matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOENT | libc::ECONNREFUSED)
        )
}

fn sweep(base: &Path, min_age: Duration) -> Result<(), Errno> {
    for entry in fs::read_dir(base).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_prefix("af-")) else {
            continue;
        };
        if id.is_empty() || id.len() > 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(error)),
        };
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o700
            || metadata
                .modified()
                .map_err(io_error)?
                .elapsed()
                .unwrap_or_default()
                < min_age
        {
            continue;
        }
        let holder = match open_directory(&path) {
            Ok(holder) => holder,
            Err(crate::errno::ENOENT) => continue,
            Err(error) => return Err(error),
        };
        match lock(&holder, libc::LOCK_EX | libc::LOCK_NB) {
            Ok(()) => {}
            Err(crate::errno::EAGAIN) => continue,
            Err(error) => return Err(error),
        }
        // Brokers predating directory locks may still be alive. A refused or
        // missing socket is stale; other connect failures do not prove death.
        if !socket_gone(&path.join("c.sock")) {
            continue;
        }
        Directory::new(path, holder)?.remove()?;
    }
    Ok(())
}

/// Serialize publication with sweeping so even an allocator killed before
/// starting its broker leaves a sweepable directory. Shared directory locks
/// span startup and the broker's lifetime; the kernel releases them on crash.
fn lock_parent(base: &Path) -> Result<Lease, Errno> {
    // /tmp itself may be a symlink on Darwin.
    let parent = open_directory(&fs::canonicalize(base).map_err(io_error)?)?;
    lock(&parent, libc::LOCK_EX)?;
    Ok(parent)
}

/// Keep the longest descriptor address within Darwin's sockaddr_un, even
/// when the guest runtime path is too long for a Unix socket.
pub(super) fn allocate() -> Result<Directory, Errno> {
    let capacity = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
        .sun_path
        .len();
    let mut bases = vec![std::env::temp_dir()];
    if bases[0] != Path::new("/tmp") {
        bases.push(PathBuf::from("/tmp"));
    }
    for base in &bases {
        let _parent = lock_parent(base)?;
        // Leave recent legacy sessions alone during their bind-to-listen gap.
        sweep(base, Duration::from_secs(10))?;
    }
    for base in bases {
        let _parent = lock_parent(&base)?;
        for _ in 0..16 {
            let path = base.join(format!("af-{:016x}", id()?));
            if path
                .join("c.dffffffffffffffff")
                .as_os_str()
                .as_bytes()
                .len()
                >= capacity
            {
                break;
            }
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Directory::hold(&path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(error)),
            }
        }
    }
    Err(36)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{fs::symlink, net::UnixListener};

    fn base() -> PathBuf {
        let path = std::env::temp_dir().join(format!("afs-{:x}", id().unwrap()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn directory(base: &Path, name: &str) -> PathBuf {
        let path = base.join(name);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }

    #[test]
    fn sweeping_retains_locked_startup_and_live_legacy_brokers() {
        let base = base();
        let startup = directory(&base, "af-1");
        let held = Directory::hold(&startup).unwrap();
        let legacy = directory(&base, "af-2");
        let listener = UnixListener::bind(legacy.join("c.sock")).unwrap();
        let stale = directory(&base, "af-3");
        drop(UnixListener::bind(stale.join("c.sock")).unwrap());
        fs::write(stale.join("c.sock.ready"), b"").unwrap();
        fs::write(stale.join("c.d1"), b"").unwrap();
        let empty = directory(&base, "af-4");
        let other = directory(&base, "af-not-a-session");
        symlink(&other, base.join("af-5")).unwrap();
        sweep(&base, Duration::from_secs(60)).unwrap();
        assert!(stale.exists());
        sweep(&base, Duration::ZERO).unwrap();
        assert!(startup.exists());
        assert!(legacy.exists());
        assert!(!stale.exists());
        assert!(!empty.exists());
        assert!(other.exists());
        assert!(
            fs::symlink_metadata(base.join("af-5"))
                .unwrap()
                .is_symlink()
        );
        drop(held);
        drop(listener);
        sweep(&base, Duration::ZERO).unwrap();
        assert!(!startup.exists());
        assert!(!legacy.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cleanup_preserves_a_replacement_at_the_same_path() {
        let base = base();
        let path = directory(&base, "af-1");
        let held = Directory::hold(&path).unwrap();
        fs::rename(&path, base.join("original")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"replacement").unwrap();
        drop(held);
        assert_eq!(fs::read(path.join("keep")).unwrap(), b"replacement");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn failed_second_bind_leaves_the_live_transport_intact() {
        let base = base();
        let path = directory(&base, "af-1").join("c.sock");
        let (listener, owner) = listen(&path).unwrap();
        assert!(listen(&path).is_err());
        assert!(path.exists());
        drop(listener);
        drop(owner);
        assert!(!path.parent().unwrap().exists());
        fs::remove_dir(base).unwrap();
    }

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn sweeping_collects_a_crashed_broker_but_never_its_live_transport() {
        let Some(helper) = aim_paths::input(aim_paths::host_bin("linux-run"), "host/linux-run")
        else {
            return;
        };
        let base = base();
        let path = directory(&base, "af-1");
        let key = SessionKey(path.join("c.sock"));
        let mut child = Child(
            std::process::Command::new(helper)
                .arg("--fuse-broker")
                .arg(key.transport())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !key.listening() {
            assert!(child.0.try_wait().unwrap().is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let fd = open_on_key(&key, 0, OPEN_DEVICE, &[]).unwrap();
        sweep(&base, Duration::ZERO).unwrap();
        assert_eq!(poll_device(fd).unwrap(), 4);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        unsafe {
            libc::close(fd);
        }
        assert!(path.exists());
        sweep(&base, Duration::ZERO).unwrap();
        assert!(!path.exists());
        fs::remove_dir(base).unwrap();
    }
}
