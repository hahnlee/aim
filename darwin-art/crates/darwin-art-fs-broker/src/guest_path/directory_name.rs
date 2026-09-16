//! Recover a guest cwd name from the live directory, never expose a host name.
use super::*;
use std::ffi::CStr;
use std::os::unix::fs::MetadataExt;

unsafe extern "C" {
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
}

fn host_name(file: &File) -> io::Result<Vec<u8>> {
    // Darwin F_GETPATH uses MAXPATHLEN (1024), not Android PATH_MAX.
    let mut buffer = [0i8; 1024];
    if unsafe { fcntl(file.as_raw_fd(), 50, buffer.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_bytes()
        .to_vec())
}

impl GuestRoot {
    /// Naming query only. I/O must continue to use the original directory FD.
    /// A candidate is accepted only if guest resolution selects the same inode.
    pub fn directory_path(&self, directory: &File) -> io::Result<Vec<u8>> {
        let metadata = directory.metadata()?;
        if !metadata.is_dir() {
            return Err(io::Error::from_raw_os_error(20));
        }
        let target = host_name(directory)?;
        let mut roots = vec![(b"/".as_slice(), &self.root.root)];
        roots.extend(
            self.mounts
                .iter()
                .map(|(path, mount)| (path.as_slice(), &mount.root)),
        );
        for (guest, root) in roots {
            let host = host_name(root)?;
            let relative = if target == host {
                b"".as_slice()
            } else if host == b"/" {
                &target[1..]
            } else if target.starts_with(&host) && target.get(host.len()) == Some(&b'/') {
                &target[host.len() + 1..]
            } else {
                continue;
            };
            let mut candidate = guest.to_vec();
            if !relative.is_empty() {
                if !candidate.ends_with(b"/") {
                    candidate.push(b'/');
                }
                candidate.extend_from_slice(relative);
            }
            let Ok(opened) = self.open(&candidate) else {
                continue;
            };
            let found = opened.node.metadata();
            if found.dev() == metadata.dev() && found.ino() == metadata.ino() {
                return Ok(opened.canonical_path);
            }
        }
        Err(io::Error::from_raw_os_error(2))
    }
}
