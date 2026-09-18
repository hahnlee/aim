//! macOS transport/process checks for profile process-registration requests.
use crate::ProfileError;
use crate::process_incarnation::ProcessIncarnation;
use std::ffi::c_void;
use std::io;
use std::os::unix::net::UnixStream;

pub(crate) fn verify_registration(
    stream: &UnixStream,
    claimed: u32,
) -> Result<ProcessIncarnation, ProfileError> {
    let before = ProcessIncarnation::read_live(claimed)?;
    verify_relationship(stream, claimed)?;
    if ProcessIncarnation::read_live(claimed)? != before {
        return Err(ProfileError::Daemon(
            "process changed during authentication".into(),
        ));
    }
    Ok(before)
}

pub(crate) fn identity(stream: &UnixStream) -> Result<(u32, ProcessIncarnation), ProfileError> {
    crate::peer_credentials::identity(stream)
}

fn peer_pid(stream: &UnixStream) -> Result<u32, ProfileError> {
    identity(stream).map(|(pid, _)| pid)
}

fn verify_relationship(stream: &UnixStream, claimed: u32) -> Result<(), ProfileError> {
    let peer = peer_pid(stream)?;
    if claimed == peer {
        return Ok(());
    }
    // `supervise` legitimately holds a lease for its directly spawned child.
    // Do not extend this authority to arbitrary same-user or descendant PIDs.
    const PROC_PPID_ONLY: u32 = 6;
    let required = unsafe { proc_listpids(PROC_PPID_ONLY, peer as u32, std::ptr::null_mut(), 0) };
    if required < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if required > 4 * 1024 * 1024 {
        return Err(ProfileError::Daemon(
            "child process list exceeds limit".into(),
        ));
    }
    let mut children = vec![0_i32; required as usize / size_of::<i32>() + 64];
    let capacity = (children.len() * size_of::<i32>()) as i32;
    let actual = unsafe {
        proc_listpids(
            PROC_PPID_ONLY,
            peer as u32,
            children.as_mut_ptr().cast(),
            capacity,
        )
    };
    if actual < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if actual > capacity || actual as usize % size_of::<i32>() != 0 {
        return Err(ProfileError::Daemon("invalid child process list".into()));
    }
    if children[..actual as usize / size_of::<i32>()].contains(&(claimed as i32)) {
        return Ok(());
    }
    Err(ProfileError::Daemon(
        "PID is neither the socket peer nor its direct child".into(),
    ))
}

unsafe extern "C" {
    fn proc_listpids(kind: u32, info: u32, buffer: *mut c_void, size: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    #[test]
    fn allows_peer_or_direct_child_but_not_unrelated_pid() {
        let path = std::env::temp_dir().join(format!(
            "darwin-registration-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(verify_registration(&server, std::process::id()).is_ok());
        assert!(verify_registration(&server, 1).is_err());
        assert!(verify_registration(&server, 0).is_err());
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let result = verify_registration(&server, child.id());
        drop(child.stdin.take());
        child.wait().unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(verify_registration(&server, child.id()).is_err());
        let (inherited, _) = UnixStream::pair().unwrap();
        assert!(verify_registration(&inherited, std::process::id()).is_err());
    }
}
