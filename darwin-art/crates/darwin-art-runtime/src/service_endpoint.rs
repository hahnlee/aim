//! Owned Darwin listening endpoints for Binder/compositor transports.
//! Binding a socket is not Android service readiness.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

// flock is shared by dup/fork file descriptions. The service owner's lifetime,
// not the last transient pre-exec copy of its FD, determines explicit release.
struct EndpointLock {
    file: File,
    owner_pid: u32,
}

impl EndpointLock {
    fn acquire(file: File) -> io::Result<Self> {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file,
            owner_pid: std::process::id(),
        })
    }

    fn is_current_owner(&self) -> bool {
        self.owner_pid == std::process::id()
    }
}

impl Drop for EndpointLock {
    fn drop(&mut self) {
        if !self.is_current_owner() {
            return;
        }
        loop {
            if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) } == 0 {
                break;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                // File's Drop still closes this owner's descriptor on failure.
                break;
            }
        }
    }
}

pub(crate) struct ServiceEndpoint {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    _lock: EndpointLock,
}

impl ServiceEndpoint {
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        if !path.is_absolute()
            || path.file_name().is_none()
            || path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err(io::Error::other(
                "service endpoint requires an absolute path",
            ));
        }
        let parent = path.parent().unwrap();
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)?;
        let metadata = directory.metadata()?;
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::other(
                "service endpoint directory is not private to its owner",
            ));
        }
        let name = path.file_name().unwrap();
        let mut lock_name = name.to_os_string();
        lock_name.push(".owner-lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent.join(lock_name))?;
        let metadata = lock.metadata()?;
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::other("invalid endpoint owner lock"));
        }
        let lock = EndpointLock::acquire(lock)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() || metadata.uid() != uid {
                    return Err(io::Error::other("endpoint path is not an owned socket"));
                }
                if !recorded_owner_is_dead(&lock.file, &metadata)? {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "endpoint has no matching, proven-dead owner",
                    ));
                }
                let current = fs::symlink_metadata(path)?;
                if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                    return Err(io::Error::other(
                        "service endpoint changed during stale check",
                    ));
                }
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        let metadata = fs::symlink_metadata(path)?;
        let endpoint = Self {
            listener,
            path: path.into(),
            device: metadata.dev(),
            inode: metadata.ino(),
            _lock: lock,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let mut record = &endpoint._lock.file;
        record.rewind()?;
        record.set_len(0)?;
        writeln!(
            record,
            "v1 {} {} {}",
            std::process::id(),
            endpoint.device,
            endpoint.inode
        )?;
        record.sync_data()?;
        Ok(endpoint)
    }

    /// Borrowed descriptor: caller may accept, but must not close this listener.
    pub(crate) fn descriptor(&self) -> RawFd {
        self.listener.as_raw_fd()
    }
}

// A refused connection does not prove death: Darwin may refuse a full backlog.
// Only our retained lock's matching creation record plus ESRCH permits cleanup.
// PID reuse deliberately fails closed. Unknown legacy sockets stay untouched.
fn recorded_owner_is_dead(mut lock: &File, socket: &fs::Metadata) -> io::Result<bool> {
    lock.rewind()?;
    let mut record = String::new();
    lock.take(129).read_to_string(&mut record)?;
    if record.len() > 128 {
        return Ok(false);
    }
    let fields: Vec<_> = record.split_whitespace().collect();
    if fields.len() != 4 || fields[0] != "v1" {
        return Ok(false);
    }
    let Ok(pid) = fields[1].parse::<i32>() else {
        return Ok(false);
    };
    if pid <= 0
        || fields[2].parse::<u64>().ok() != Some(socket.dev())
        || fields[3].parse::<u64>().ok() != Some(socket.ino())
    {
        return Ok(false);
    }
    Ok(unsafe { libc::kill(pid, 0) } < 0
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
}

impl Drop for ServiceEndpoint {
    fn drop(&mut self) {
        // A forked process may close its inherited descriptors, but it did not
        // publish this endpoint and must not unpublish the original owner's path.
        if !self._lock.is_current_owner() {
            return;
        }
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
#[path = "service_endpoint_tests.rs"]
mod tests;
