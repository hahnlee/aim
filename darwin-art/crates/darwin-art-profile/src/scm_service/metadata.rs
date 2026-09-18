//! Narrow Darwin regular-file carrier; authorization never comes from its bytes.
use crate::ProfileError;
use darwin_art_scm_transfer::inheritance::guard;
use std::{
    fs::File,
    io::Write,
    os::fd::{FromRawFd, OwnedFd},
};

struct TemporaryPath {
    path: Vec<u8>,
    created: bool,
}
impl Drop for TemporaryPath {
    fn drop(&mut self) {
        // mkstemp has resolved this exact private file, not a directory/glob.
        if self.created {
            unsafe { libc::unlink(self.path.as_ptr().cast()) };
        }
    }
}

pub(super) fn create(bytes: &[u8]) -> Result<OwnedFd, ProfileError> {
    let mut path = TemporaryPath {
        path: b"/tmp/darwin-art-scm-metadata.XXXXXX\0".to_vec(),
        created: false,
    };
    let mut file = {
        let _inheritance = guard()?;
        let raw = unsafe { libc::mkstemp(path.path.as_mut_ptr().cast()) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        path.created = true;
        let owned = unsafe { OwnedFd::from_raw_fd(raw) };
        loop {
            if unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        File::from(owned)
    };
    file.write_all(bytes)?;
    // Reopen read-only before unlink, rather than sending the writable creation
    // description. The service ledger remains authoritative even for bad files.
    let _inheritance = guard()?;
    let raw = unsafe {
        libc::open(
            path.path.as_ptr().cast(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let reader = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::unlink(path.path.as_ptr().cast()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    path.created = false;
    drop(file);
    Ok(reader)
}
