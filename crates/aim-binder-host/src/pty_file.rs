//! Opaque PTY carriers are classified by their native endpoint owner.
use aim_storage::pty_owner::transport::{Client, OwnerConfig};
use std::{io, os::fd::BorrowedFd, sync::{Arc, RwLock}};

pub const CLASS: u32 = aim_storage::pty_owner::transport::CLASS;
static OWNER: RwLock<Option<Arc<Client>>> = RwLock::new(None);

pub fn install_owner(config: &OwnerConfig) -> io::Result<()> {
    let client = Client::lookup(&config.endpoint, config.process)?;
    let previous=OWNER.write().unwrap().replace(Arc::new(client));
    drop(previous);Ok(())
}
pub(crate) fn registered_class(fd: i32) -> io::Result<u32> {
    if aim_storage::pipe_identity::key(fd)?.is_none() { return Ok(0); }
    let Some(owner) = OWNER.read().unwrap().clone() else { return Ok(0); };
    // The raw number is retained by the calling transport for this query.
    owner.classify(unsafe { BorrowedFd::borrow_raw(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::{fd::{AsFd, AsRawFd}, unix::fs::DirBuilderExt};
    #[test]
    fn authenticated_pipe_carrier_crosses_actual_binder_class_query() {
        const MARKER: &str = "PTY_BINDER_PIPE_OWNER_EXECUTED";
        if crate::socket_scm::tests::isolated_exec_fixture(
            "pty_file::tests::authenticated_pipe_carrier_crosses_actual_binder_class_query", MARKER) { return; }
        let runtime = std::env::temp_dir().join(format!("aim-pty-binder-{}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
        let runtime = std::fs::canonicalize(runtime).unwrap();
        let name = format!("dev.aim.pty-binder-owner.{}", std::process::id());
        let owner = aim_storage::pty_owner::transport::RunningServer::start(&name, &runtime).unwrap();
        install_owner(&owner.config).unwrap();
        let pty = Client::lookup(&name, owner.config.process).unwrap();
        let master = pty.allocate(2).unwrap();
        let slave = pty.open_slave(&master, 2).unwrap();
        let carrier = pty.export_carrier(&slave).unwrap();
        let weak = slave.capability.observe().unwrap();
        let binder_name = format!("dev.aim.pty-binder-driver.{}", std::process::id());
        let _binder = crate::server::Server::start(&binder_name).unwrap();
        let client = crate::client::Client::connect(&binder_name).unwrap();
        assert_eq!(client.file_class(carrier.as_raw_fd()).unwrap(), CLASS);
        let ordinary = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(client.file_class(ordinary.as_raw_fd()).unwrap(), 0);
        let native_file = crate::server::file_from_fd(carrier.as_fd()).unwrap();
        assert_eq!(crate::server::file_class(&native_file), Some(CLASS));
        drop(slave);
        drop(carrier);
        assert!(!pty.readiness(&master).unwrap().hangup);
        drop(native_file);
        let mut event = libc::pollfd { fd: master.capability.notification().unwrap().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut event, 1, 2000) }, 1);
        assert!(weak.status.retired());
        drop(weak);
        drop(master);
        owner.shutdown().unwrap();
        std::fs::remove_dir(runtime).unwrap();
        println!("{MARKER}");
    }
}
