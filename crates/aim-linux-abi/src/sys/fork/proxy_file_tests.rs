//! Proxy capabilities across the same spawn/FD-state boundary as guest fork.
use super::*;
use crate::sys::{binder, fs, fsops, net, proxy_file};
use aim_binder_host::{proxy_file as owner, server::Server};
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
};

const CHILD: &str = "sys::fork::spawn::proxy_file_tests::proxy_receipt_child";

fn scm(socket: i32, send: Option<i32>) -> i32 {
    let mut control = [0u8; 24];
    if let Some(fd) = send {
        control[..8].copy_from_slice(&20u64.to_ne_bytes());
        control[8..12].copy_from_slice(&1i32.to_ne_bytes());
        control[12..16].copy_from_slice(&1i32.to_ne_bytes());
        control[16..20].copy_from_slice(&fd.to_ne_bytes());
    }
    let mut payload = [b'x'];
    let iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: 1,
    };
    // Linux arm64 msghdr: name, namelen/pad, iov, iovlen, control,
    // controllen, flags/pad (Darwin's native msghdr has a different layout).
    let mut message = [
        0u64,
        0,
        &iov as *const _ as u64,
        1,
        control.as_mut_ptr() as u64,
        control.len() as u64,
        0,
    ];
    let args = [socket as u64, message.as_mut_ptr() as u64, 0, 0, 0, 0];
    if let Some(fd) = send {
        assert_eq!(net::sendmsg(args), 1);
        fd
    } else {
        assert_eq!(net::recvmsg(args), 1);
        assert_eq!(payload, [b'x']);
        assert_eq!(message[6], 0, "truncated ancillary data");
        assert_eq!(message[5], 24);
        assert_eq!(u64::from_ne_bytes(control[..8].try_into().unwrap()), 20);
        assert_eq!(i32::from_ne_bytes(control[8..12].try_into().unwrap()), 1);
        assert_eq!(i32::from_ne_bytes(control[12..16].try_into().unwrap()), 1);
        let fd = i32::from_ne_bytes(control[16..20].try_into().unwrap());
        assert!(proxy_file::is_proxy(fd));
        fd
    }
}

fn expect(channel: &mut UnixStream, expected: u8) {
    let mut byte = [0];
    channel.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [expected]);
}

fn close(fd: i32) {
    assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    assert!(!fdtab::visible(fd));
    assert!(!proxy_file::is_proxy(fd));
}

#[test]
#[ignore = "actual subprocess of the proxy fork receipt tests"]
fn proxy_receipt_child() {
    let argument = std::env::args()
        .find(|arg| arg.starts_with("FORK_PROXY="))
        .unwrap();
    let fields = argument
        .strip_prefix("FORK_PROXY=")
        .unwrap()
        .split('|')
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 5);
    let fd = fields[0].parse::<i32>().unwrap();
    let channel_fd = fields[1].parse::<i32>().unwrap();
    reserve_fds(fields[2]).unwrap();
    assert!(fdtab::get(fd).is_none());
    assert!(!fdtab::visible(fd));
    let blob = std::fs::read(fields[3]).unwrap();
    let mut reader = Reader::new(&blob);
    fdtab::fork_restore(&mut reader);
    assert!(reader.ok(), "{}", reader.diagnostic());
    fdtab::after_fork_child().unwrap();
    // Check before binder::init can reclassify through the owner's registry.
    assert!(proxy_file::is_proxy(fd));
    assert!(fdtab::visible(fd));
    assert!(net::hidden_socket(fd));
    let bytes = b"child";
    assert_eq!(fs::write([fd as u64, bytes.as_ptr() as u64, 5, 0, 0, 0]), 5);
    assert_eq!(fs::lseek([fd as u64, 0, 1, 0, 0, 0]), 5);
    // SAFETY: the receipt passed this endpoint exclusively to the child.
    let mut channel = unsafe { UnixStream::from_raw_fd(channel_fd) };
    channel
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    if fields[4].is_empty() {
        channel.write_all(b"i").unwrap();
    } else {
        binder::init(fields[4]).unwrap();
        scm(channel_fd, Some(fd));
    }
    expect(&mut channel, b'r');
    let mut byte = [0u8];
    assert_eq!(
        fs::read([fd as u64, byte.as_mut_ptr() as u64, 1, 0, 0, 0]),
        -1
    );
    assert_eq!(
        fs::write([fd as u64, bytes.as_ptr() as u64, 5, 0, 0, 0]),
        -1
    );
    assert_eq!(fs::lseek([fd as u64, 0, 0, 0, 0, 0]), -1);
    assert_eq!(fsops::fsync([fd as u64, 0, 0, 0, 0, 0]), -1);
    close(fd);
    channel.write_all(b"c").unwrap();
    // Remain alive while the parent checks release of each remaining reference.
    expect(&mut channel, b'q');
    close(channel.into_raw_fd());
    println!("FORK_PROXY_CHILD_EXECUTED");
}

#[test]
fn proxy_class_io_reexport_revocation_and_last_close_survive_fork() {
    if fdtab::isolated_kernel_test(
        "sys::fork::spawn::proxy_file_tests::proxy_class_io_reexport_revocation_and_last_close_survive_fork",
    ) {
        return;
    }
    check(true);
}

#[test]
fn proxy_class_io_revocation_and_last_close_survive_fork() {
    if fdtab::isolated_kernel_test(
        "sys::fork::spawn::proxy_file_tests::proxy_class_io_revocation_and_last_close_survive_fork",
    ) {
        return;
    }
    check(false);
}

fn check(reexport: bool) {
    fdtab::install_storage_registrar().unwrap();
    let name = if reexport {
        format!("dev.aim.test.fork-proxy.{}", std::process::id())
    } else {
        String::new()
    };
    let _server = reexport.then(|| {
        let server = Server::start(&name).unwrap();
        binder::init(&name).unwrap();
        server
    });
    let directory = std::env::temp_dir().join(format!("aim-fork-proxy-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("data");
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let (owner, capability, worker) = owner::open(file).unwrap();
    let fd = capability.into_raw_fd();
    fdtab::insert(fd, fdtab::Kind::ProxyFile);
    fdtab::publish_guest(fd).unwrap();
    let (mut channel, child_channel) = UnixStream::pair().unwrap();
    channel
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    fdtab::publish_guest(child_channel.as_raw_fd()).unwrap();
    let mut writer = Writer::default();
    let guard = fdtab::lifecycle();
    let (fds, guest_owners) = fdtab::fork_guest_snapshot().unwrap();
    fdtab::fork_save(&mut writer);
    let private = writer.take_private().unwrap();
    assert!(private.is_empty());
    let stdout =
        fdtab::hold_fork_private(unsafe { std::os::fd::BorrowedFd::borrow_raw(1) }).unwrap();
    let stderr =
        fdtab::hold_fork_private(unsafe { std::os::fd::BorrowedFd::borrow_raw(2) }).unwrap();
    let transfers = guest_owners
        .iter()
        .chain([&stdout, &stderr])
        .map(|owner| (owner.source().as_raw_fd(), owner.target()))
        .collect::<Vec<_>>();
    let reservation = reservation_text(&[], &fds, &transfers).unwrap();
    drop(guard);
    let blob = directory.join("state");
    std::fs::write(&blob, writer.into_bytes()).unwrap();
    let marker = format!(
        "FORK_PROXY={fd}|{}|{}|{}|{name}",
        child_channel.as_raw_fd(),
        reservation.to_str().unwrap(),
        blob.display()
    );
    let executable = CString::new(std::env::current_exe().unwrap().as_os_str().as_bytes()).unwrap();
    let arguments = [
        executable.clone(),
        CString::new("--exact").unwrap(),
        CString::new(CHILD).unwrap(),
        CString::new("--ignored").unwrap(),
        CString::new("--nocapture").unwrap(),
        CString::new("--skip").unwrap(),
        CString::new(marker).unwrap(),
    ];
    let mut argv = arguments.iter().map(|arg| arg.as_ptr()).collect::<Vec<_>>();
    argv.push(std::ptr::null());
    let port = new_port().unwrap();
    let pid = spawn_program(&executable, &argv, &transfers, port).unwrap();
    struct Child(Option<i32>);
    impl Drop for Child {
        fn drop(&mut self) {
            if let Some(pid) = self.0 {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, std::ptr::null_mut(), 0);
                }
            }
        }
    }
    let mut child = Child(Some(pid));
    drop(guest_owners);
    drop(stdout);
    drop(stderr);
    fdtab::withdraw_guest(child_channel.as_raw_fd()).unwrap();
    fdtab::on_close(child_channel.as_raw_fd());
    drop(child_channel);
    fdtab::publish_guest(channel.as_raw_fd()).unwrap();
    let exported = if reexport {
        Some(scm(channel.as_raw_fd(), None))
    } else {
        expect(&mut channel, b'i');
        None
    };
    assert_eq!(fs::lseek([fd as u64, 0, 1, 0, 0, 0]), 5);
    let bytes = b"parent";
    let remaining = exported.unwrap_or(fd);
    assert_eq!(
        fs::write([remaining as u64, bytes.as_ptr() as u64, 6, 0, 0, 0]),
        6
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"childparent");
    owner.revoke();
    channel.write_all(b"r").unwrap();
    expect(&mut channel, b'c');
    assert!(
        !owner.is_released(),
        "child's last close released the parent's capability"
    );
    assert!(!worker.is_finished());
    assert!(proxy_file::is_proxy(fd) && proxy_file::is_proxy(remaining));
    assert_eq!(fsops::fsync([remaining as u64, 0, 0, 0, 0, 0]), -1);
    close(fd);
    if let Some(exported) = exported {
        assert!(
            !owner.is_released(),
            "re-exported reference was released early"
        );
        close(exported);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !owner.is_released() || !worker.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "last capability close did not release owner"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    channel.write_all(b"q").unwrap();
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    child.0 = None;
    assert!(libc::WIFEXITED(status));
    assert_eq!(libc::WEXITSTATUS(status), 0);
    unsafe {
        mach_port_deallocate(task(), port);
        mach_port_mod_refs(task(), port, MACH_PORT_RIGHT_RECEIVE, -1);
    }
    close(channel.into_raw_fd());
    drop(worker);
    std::fs::remove_dir_all(directory).unwrap();
}
