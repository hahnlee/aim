//! Android process credentials supplied by the profile launch authority.
//!
//! Darwin's uid describes the macOS account hosting the compatibility layer;
//! it is never an Android application identity.  Profile-launched Android
//! processes resolve their already-registered package uid before ART starts
//! and install that immutable credential set into the Bionic process snapshot.

use darwin_art_engine::ProcessCredentialsInputs;
use std::path::Path;

pub(super) fn inputs() -> Result<Option<ProcessCredentialsInputs>, String> {
    let Some(socket) = std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV) else {
        // Standalone native/bootstrap tests are not Android package processes.
        return Ok(None);
    };
    let pid = std::process::id();
    let debug = std::env::var_os("DARWIN_ART_DEBUG_PROCESS_CREDENTIALS").is_some();
    if debug {
        eprintln!("ART process credentials: resolve pid={pid}");
    }
    let identity = darwin_art_profile::resolve_process_identity_at(Path::new(&socket), pid)
        .map_err(|error| format!("resolve Android process credentials: {error}"))?;
    if identity.pid != pid {
        return Err("profile returned credentials for a different process".into());
    }
    if debug {
        eprintln!(
            "ART process credentials: resolved pid={} package={} uid={}",
            identity.pid, identity.package, identity.uid
        );
    }
    Ok(Some(ProcessCredentialsInputs {
        uid: identity.uid,
        euid: identity.uid,
        suid: identity.uid,
        gid: identity.uid,
        egid: identity.uid,
        sgid: identity.uid,
        groups: Vec::new(),
        permitted: 0,
        effective: 0,
        inheritable: 0,
    }))
}
