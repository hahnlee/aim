//! One public SCM descriptor, with the existing inode writer lease in native
//! custody while the actual carrier endpoint remains queued or open (#226).
use crate::errno::{self, Errno};
use super::super::{binder, fdtab};
use aim_storage::private_fd::PrivateFd;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(super) struct Export {
    carrier: PrivateFd,
    _source: fdtab::RegularExport,
}
impl Export {
    pub(super) fn fd(&self) -> i32 { self.carrier.as_raw_fd() }
}
fn io(error: std::io::Error) -> Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}
pub(super) fn export(source: fdtab::RegularExport) -> Result<Export, Errno> {
    let port = binder::create_regular_scm(source.backing_fd, source.transport.writer_fd, &source.transport.metadata)?;
    let carrier = PrivateFd::allocate(|| {
        let fd = aim_binder_host::mach::port_to_fd(port.as_port())
            .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EIO))?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }).map_err(io)?;
    Ok(Export { carrier, _source: source })
}
/// The received slot remains unpublished until both the real backing and
/// authenticated existing writer open description have been installed.
pub(super) fn adopt(fd: i32) -> Result<(), Errno> {
    let resolved = binder::resolve_regular_scm(fd)?;
    fdtab::install_fileport(fd, resolved.backing.as_port())?;
    fdtab::install_regular(fd, &resolved.metadata, resolved.writer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;

    #[test]
    fn regular_scm_queue_receipt_reexport_and_discard_retain_same_writer() {
        if fdtab::isolated_kernel_test("sys::net::regular_scm::tests::regular_scm_queue_receipt_reexport_and_discard_retain_same_writer") { return; }
        let (_view, directory) = crate::vfs::test_view();
        let name = format!("dev.aim.test.regular-scm.{}", std::process::id());
        let _server = aim_binder_host::server::Server::start(&name).unwrap();
        binder::init(&name).unwrap();
        let path = directory.join("data/regular-scm-file");
        std::fs::write(&path, b"original").unwrap();
        let guest = c"/data/regular-scm-file";
        let fd = super::super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64, guest.as_ptr() as u64, 2, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        let fdtab::Kind::Regular(description) = fdtab::get(fd).unwrap() else { panic!("real open did not install its regular owner") };
        let identity = description.identity.to_bytes();
        let store = description.store.clone();
        drop(description);
        let read_only = std::fs::File::open(&path).unwrap();
        let busy = || {
            binder::drain_regular_scm(&identity).unwrap();
            let admission = store.lock_inode(&read_only).unwrap();
            assert!(matches!(admission.begin_enable(), Err(aim_storage::fsverity::Error::Linux(26))));
        };
        let pair = || {
            let mut pair = [-1i32; 2];
            assert_eq!(socketpair([L_AF_UNIX as u64, L_SOCK_STREAM, 0, pair.as_mut_ptr() as u64, 0, 0]), 0);
            pair
        };
        let transfer = |channel: i32, file: i32| {
            let mut control = [0u8; 24];
            control[..8].copy_from_slice(&20u64.to_ne_bytes());
            control[8..12].copy_from_slice(&L_SOL_SOCKET.to_ne_bytes());
            control[12..16].copy_from_slice(&L_SCM_RIGHTS.to_ne_bytes());
            control[16..20].copy_from_slice(&file.to_ne_bytes());
            let mut byte = [b'q'];
            let iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
            let message = LinuxMsghdr { name:0, namelen:0, _pad:0, iov:&iov as *const _ as u64, iovlen:1, control:control.as_mut_ptr() as u64, controllen:24, flags:0, _pad2:0 };
            assert_eq!(sendmsg([channel as u64, &message as *const _ as u64, 0,0,0,0]), 1);
        };
        let receive = |channel: i32, writer: bool| {
            let mut control = [0u8; 24]; let mut byte = [0u8];
            let iov = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
            let mut message = LinuxMsghdr { name:0, namelen:0, _pad:0, iov:&iov as *const _ as u64, iovlen:1, control:control.as_mut_ptr() as u64, controllen:24, flags:0, _pad2:0 };
            assert_eq!(recvmsg([channel as u64, &mut message as *mut _ as u64, L_MSG_CMSG_CLOEXEC,0,0,0]), 1);
            assert_eq!(byte, [b'q']); assert_eq!(message.controllen, 24); assert_eq!(message.flags & L_MSG_CTRUNC, 0);
            let received = i32::from_ne_bytes(control[16..20].try_into().unwrap());
            let fdtab::Kind::Regular(owner) = fdtab::get(received).unwrap() else { panic!("carrier escaped without its real backing") };
            assert_eq!(owner.identity.to_bytes(), identity); assert_eq!(owner.writer.is_some(), writer);
            assert_eq!(unsafe { libc::fcntl(received,libc::F_GETFD) } & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
            received
        };
        let close = |fd: i32| assert_eq!(super::super::super::fs::close([fd as u64,0,0,0,0,0]),0);
        let first = pair(); transfer(first[0], fd); close(fd); busy();
        let received = receive(first[1], true); busy();
        let data = b"z";
        assert_eq!(super::super::super::fs::write([received as u64, data.as_ptr() as u64, 1,0,0,0]),1);
        assert_eq!(std::fs::read(&path).unwrap(), b"zriginal");
        let second = pair(); transfer(second[0], received); close(received); busy();
        let reexported = receive(second[1], true); busy(); close(reexported);
        binder::drain_regular_scm(&identity).unwrap();
        drop(store.lock_inode(&read_only).unwrap().begin_enable().unwrap());
        let discard = pair();
        let fd = super::super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64, guest.as_ptr() as u64, 2,0,0,0]) as i32;
        assert!(fd >= 0); transfer(discard[0],fd); close(fd); busy();
        close(discard[1]);
        binder::drain_regular_scm(&identity).unwrap();
        drop(store.lock_inode(&read_only).unwrap().begin_enable().unwrap());
        let reader = pair();
        let fd = super::super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64, guest.as_ptr() as u64, 0,0,0,0]) as i32;
        assert!(fd >= 0); transfer(reader[0],fd); close(fd);
        binder::drain_regular_scm(&identity).unwrap();
        drop(store.lock_inode(&read_only).unwrap().begin_enable().unwrap());
        let received = receive(reader[1], false);
        assert_eq!(super::super::super::fs::write([received as u64, data.as_ptr() as u64, 1,0,0,0]), -(errno::EBADF as i64));
        close(received); binder::drain_regular_scm(&identity).unwrap();
        for fd in [first[0],first[1],second[0],second[1],discard[0],reader[0],reader[1]] { close(fd); }
        drop(read_only); std::fs::remove_file(path).unwrap();
    }
}
