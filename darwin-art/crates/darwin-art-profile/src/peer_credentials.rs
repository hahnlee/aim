//! Socket-bound Darwin process credentials, not Android permission policy.

use crate::{ProfileError, process_incarnation::ProcessIncarnation};
use std::{
    io,
    os::{fd::AsRawFd, unix::net::UnixStream},
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AuditToken {
    values: [u32; 8],
}

// XNU proc_info_private.h, PROC_PIDUNIQIDENTIFIERINFO=17. The pinned
// Darwin 25 provider validates the complete 56-byte result, never a prefix.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UniqueProcessInfo {
    executable_uuid: [u8; 16],
    unique_id: u64,
    parent_unique_id: u64,
    pid_version: i32,
    original_parent_version: i32,
    reserved: [u64; 2],
}
const _: () = assert!(size_of::<AuditToken>() == 32);
const _: () = assert!(size_of::<UniqueProcessInfo>() == 56);

fn live_unique_info(pid: u32) -> Result<UniqueProcessInfo, ProfileError> {
    let mut info = UniqueProcessInfo::default();
    let length = size_of::<UniqueProcessInfo>() as i32;
    // SAFETY: storage is exactly the validated native flavor's output size.
    let actual = unsafe {
        proc_pidinfo(
            pid as i32,
            17,
            0,
            (&mut info as *mut UniqueProcessInfo).cast(),
            length,
        )
    };
    if actual <= 0 {
        return Err(io::Error::last_os_error().into());
    }
    if actual != length || info.unique_id == 0 {
        return Err(ProfileError::Daemon(
            "invalid kernel process version".into(),
        ));
    }
    Ok(info)
}

fn validate_version(
    version: i32,
    before: UniqueProcessInfo,
    after: UniqueProcessInfo,
) -> Result<(), ProfileError> {
    if before.pid_version != version
        || after.pid_version != version
        || before.unique_id != after.unique_id
    {
        return Err(ProfileError::Daemon(
            "socket peer process lifetime changed".into(),
        ));
    }
    Ok(())
}

pub(crate) fn identity(stream: &UnixStream) -> Result<(u32, ProcessIncarnation), ProfileError> {
    let mut token = AuditToken::default();
    let mut length = size_of::<AuditToken>() as libc::socklen_t;
    // SDK sys/un.h: SOL_LOCAL=0, LOCAL_PEERTOKEN=6. This token belongs to
    // the socket connection, unlike a later lookup of its numeric PID.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            0,
            6,
            (&mut token as *mut AuditToken).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if length as usize != size_of::<AuditToken>() {
        return Err(ProfileError::Daemon(
            "invalid socket audit token size".into(),
        ));
    }
    let pid = unsafe { audit_token_to_pid(token) };
    let version = unsafe { audit_token_to_pidversion(token) };
    if pid <= 0 {
        return Err(ProfileError::Daemon("invalid socket peer PID".into()));
    }
    let pid = pid as u32;
    let before = live_unique_info(pid)?;
    let birth = ProcessIncarnation::read_live(pid)?;
    let after = live_unique_info(pid)?;
    validate_version(version, before, after)?;
    Ok((pid, birth))
}

#[link(name = "bsm")]
unsafe extern "C" {
    fn audit_token_to_pid(token: AuditToken) -> libc::pid_t;
    fn audit_token_to_pidversion(token: AuditToken) -> i32;
}
unsafe extern "C" {
    fn proc_pidinfo(
        pid: i32,
        flavor: i32,
        arg: u64,
        buffer: *mut std::ffi::c_void,
        size: i32,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_socket_token_matches_live_kernel_process() {
        let path =
            std::env::temp_dir().join(format!("darwin-peer-token-{}.sock", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        let observed = identity(&server).unwrap();
        assert_eq!(observed.0, std::process::id());
        assert_eq!(
            observed.1,
            ProcessIncarnation::read_live(observed.0).unwrap()
        );
        drop(client);
        drop(server);
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reused_pid_version_or_changed_unique_process_is_rejected() {
        let before = UniqueProcessInfo {
            unique_id: 42,
            pid_version: 7,
            ..UniqueProcessInfo::default()
        };
        assert!(validate_version(8, before, before).is_err());
        let after = UniqueProcessInfo {
            unique_id: 43,
            ..before
        };
        assert!(validate_version(7, before, after).is_err());
        assert!(validate_version(7, before, before).is_ok());
    }
}
