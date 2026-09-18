//! APK process ownership of its Binder endpoint and authority dispatcher.
//!
//! Guest descriptor lifetime remains in the FD broker. This owner is only for
//! OS-lifetime application processes, never reusable/in-process runtime sessions.

use crate::{config::HostError, process_exit::exit_android_process};
use darwin_art_binder_process::{
    AuthorityLifetime, BinderFdEndpoint, BrokerApi, Error, connect_process,
};
use darwin_art_profile::BinderAuthorityConnection;
use std::{path::Path, thread::JoinHandle};

/// Arm the OS exit obligation *before* acquiring Binder resources. On an error
/// return or unwind, Drop exits before Rust drops the endpoint's fields: endpoint
/// unregister waits for active guest ioctls and must not precede APK process exit.
pub(crate) struct ApplicationBinderProcess {
    endpoint: Option<Box<BinderFdEndpoint<BinderAuthorityConnection>>>,
    dispatcher: Option<JoinHandle<Result<(), Error>>>,
}

impl ApplicationBinderProcess {
    pub(crate) fn new() -> Self {
        Self {
            endpoint: None,
            dispatcher: None,
        }
    }

    pub(crate) fn connect(&mut self, socket: &Path, broker: BrokerApi) -> Result<(), HostError> {
        assert!(self.endpoint.is_none() && self.dispatcher.is_none());
        let (client, dispatcher) =
            connect_process(socket, std::process::id() as i32).map_err(|error| {
                HostError::HostService(format!("connect Binder authority: {error:?}"))
            })?;
        self.dispatcher = Some(
            std::thread::Builder::new()
                .name("darwin-art-binder-dispatch".into())
                .spawn(move || {
                    let result = dispatcher.run();
                    if let Err(error) = &result {
                        eprintln!("ART Binder authority dispatcher stopped: {error:?}");
                    }
                    result
                })
                .map_err(|error| {
                    HostError::HostService(format!("spawn Binder dispatcher: {error}"))
                })?,
        );
        client.get_context_manager().map_err(|error| {
            HostError::HostService(format!("resolve Binder context manager: {error:?}"))
        })?;
        self.endpoint = Some(BinderFdEndpoint::install(client, broker).map_err(|error| {
            HostError::HostService(format!("install Binder FD endpoint: {error:?}"))
        })?);
        Ok(())
    }

    /// Return the exact authenticated authority metadata from the installed
    /// client. This clones only its atomic terminal flag; it does not expose
    /// the Binder transport or perform endpoint I/O.
    pub(crate) fn authority_lifetime(&self) -> Option<AuthorityLifetime> {
        self.endpoint
            .as_ref()
            .map(|endpoint| endpoint.client().authority_lifetime())
    }
}

impl Drop for ApplicationBinderProcess {
    fn drop(&mut self) {
        eprintln!("ART APK Binder scope ended unexpectedly; exiting process before Binder drain");
        exit_android_process(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct MustNotDrop;
    impl Drop for MustNotDrop {
        fn drop(&mut self) {
            eprintln!("unexpected-later-resource-drop");
            std::process::abort();
        }
    }

    #[test]
    fn apk_error_scope_exits_before_later_resource_drop() {
        const CHILD: &str = "DARWIN_ART_BINDER_SCOPE_TEST_CHILD";
        if let Some(mode) = std::env::var_os(CHILD) {
            let _later_resource = MustNotDrop;
            let _binder = ApplicationBinderProcess::new();
            if mode == "panic" {
                panic!("synthetic APK unwind");
            }
            return; // Same scope-drop path as an error propagated by `?`.
        }
        for mode in ["error", "panic"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "process_binder::tests::apk_error_scope_exits_before_later_resource_drop",
                    "--nocapture",
                ])
                .env(CHILD, mode)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1), "{mode}: {output:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("exiting process before Binder drain"));
            assert!(!stderr.contains("unexpected-later-resource-drop"));
        }
    }
}
