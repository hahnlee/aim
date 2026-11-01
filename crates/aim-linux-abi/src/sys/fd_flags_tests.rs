//! LTP-style descriptor-flag checks against epoll_create1(2), fcntl(2)
//! and each creator's CLOEXEC contract. No guest image is required.
use super::*;
use crate::sys::{epoll, event, inotify, net};

fn get(fd: u64) -> i64 {
    fcntl([fd, F_GETFD, 0, 0, 0, 0])
}

fn set(fd: u64, flags: u64) {
    assert_eq!(fcntl([fd, F_SETFD, flags, 0, 0, 0]), 0);
}

fn check(fd: i64, expected: i64) {
    assert!(fd >= 0, "creator failed: {fd}");
    let fd = fd as u64;
    assert_eq!(get(fd), expected, "initial flags of fd {fd}");
    for flags in [0, 1, 2, 3, u64::MAX, 0] {
        set(fd, flags);
        assert_eq!(get(fd), (flags & 1) as i64, "F_SETFD({flags:#x})");
        assert_eq!(
            unsafe { libc::fcntl(fd as i32, libc::F_GETFD) } & libc::FD_CLOEXEC,
            (flags & 1) as i32,
        );
    }
    assert_eq!(close([fd, 0, 0, 0, 0, 0]), 0);
    assert_eq!(get(fd), -(EBADF as i64));
    assert_eq!(fcntl([fd, F_SETFD, 1, 0, 0, 0]), -(EBADF as i64));
}

macro_rules! creator {
    ($name:ident, $flags:ident, $body:block) => {
        #[test]
        fn $name() {
            if fdtab::isolated_kernel_test(concat!("sys::fs::fd_flags_tests::", stringify!($name))) {
                return;
            }
            let _view = vfs::test_view();
            for $flags in [0, O_CLOEXEC] $body
        }
    };
}

creator!(epoll_flags, flags, {
    check(
        epoll::epoll_create1([flags, 0, 0, 0, 0, 0]),
        (flags != 0) as i64,
    );
});
creator!(eventfd_flags, flags, {
    check(event::eventfd2([0, flags, 0, 0, 0, 0]), (flags != 0) as i64);
});
creator!(timerfd_flags, flags, {
    check(
        event::timerfd_create([1, flags, 0, 0, 0, 0]),
        (flags != 0) as i64,
    );
});

#[test]
#[ignore = "#2648: signalfd4 is not implemented; flag coverage is not run"]
fn signalfd_flags() {
    let _view = vfs::test_view();
    // SAFETY: all-zero is a valid register context; no guest code runs.
    let mut context: crate::context::GuestContext = unsafe { std::mem::zeroed() };
    context.pc = 4;
    let mask = 0u64;
    for flags in [0, O_CLOEXEC] {
        let fd = crate::sys::handle(
            &mut context,
            74,
            [u64::MAX, &mask as *const u64 as u64, 8, flags, 0, 0],
        );
        check(fd, (flags != 0) as i64);
    }
}

creator!(inotify_flags, flags, {
    check(
        inotify::inotify_init1([flags, 0, 0, 0, 0, 0]),
        (flags != 0) as i64,
    );
});
creator!(pipe_flags, flags, {
    let mut pair = [-1i32; 2];
    assert_eq!(pipe2([pair.as_mut_ptr() as u64, flags, 0, 0, 0, 0]), 0);
    for fd in pair {
        check(fd as i64, (flags != 0) as i64);
    }
});
creator!(socket_flags, flags, {
    check(net::socket([1, 1 | flags, 0, 0, 0, 0]), (flags != 0) as i64);
});
creator!(open_flags, flags, {
    let fd = openat([
        (-100i64) as u64,
        c"/data/fd-flags".as_ptr() as u64,
        O_CREAT | 2 | flags,
        0o600,
        0,
        0,
    ]);
    check(fd, (flags != 0) as i64);
});
creator!(dup3_flags, flags, {
    let source = epoll::epoll_create1([O_CLOEXEC, 0, 0, 0, 0, 0]);
    assert!(source >= 0);
    let target = dup([source as u64, 0, 0, 0, 0, 0]);
    assert!(target >= 0);
    assert_eq!(dup3([source as u64, target as u64, flags, 0, 0, 0]), target);
    check(target, (flags != 0) as i64);
    assert_eq!(get(source as u64), 1, "flags belong to each descriptor");
    assert_eq!(close([source as u64, 0, 0, 0, 0, 0]), 0);
});
creator!(fcntl_dup_flags, flags, {
    let source = epoll::epoll_create1([O_CLOEXEC, 0, 0, 0, 0, 0]);
    assert!(source >= 0);
    let cmd = if flags == 0 { F_DUPFD } else { F_DUPFD_CLOEXEC };
    check(fcntl([source as u64, cmd, 0, 0, 0, 0]), (flags != 0) as i64);
    assert_eq!(get(source as u64), 1);
    assert_eq!(close([source as u64, 0, 0, 0, 0, 0]), 0);
});

#[test]
fn host_bits_are_not_linux_descriptor_flags() {
    if fdtab::isolated_kernel_test(
        "sys::fs::fd_flags_tests::host_bits_are_not_linux_descriptor_flags",
    ) {
        return;
    }
    let fd = epoll::epoll_create1([0; 6]);
    assert!(fd >= 0);
    // Darwin's FD_CLOFORK (2) is independent of Linux's only flag.
    for flags in [2, 3] {
        assert_eq!(unsafe { libc::fcntl(fd as i32, libc::F_SETFD, flags) }, 0);
        assert_eq!(unsafe { libc::fcntl(fd as i32, libc::F_GETFD) }, flags);
        assert_eq!(get(fd as u64), (flags & 1) as i64);
    }
    check(fd, 1);
}

#[test]
fn exec_close_selection_uses_current_descriptor_flags() {
    if fdtab::isolated_kernel_test(
        "sys::fs::fd_flags_tests::exec_close_selection_uses_current_descriptor_flags",
    ) {
        return;
    }
    let mut fds = Vec::new();
    for flags in [0, O_CLOEXEC] {
        for toggle in [false, true] {
            let fd = epoll::epoll_create1([flags, 0, 0, 0, 0, 0]);
            assert!(fd >= 0);
            let mut expected = (flags != 0) as i64;
            assert_eq!(get(fd as u64), expected);
            if toggle {
                expected ^= 1;
                set(fd as u64, expected as u64);
            }
            fds.push((fd as u64, expected));
        }
    }
    close_on_exec().unwrap();
    for (fd, flags) in fds {
        if flags == 1 {
            assert_eq!(get(fd), -(EBADF as i64));
        } else {
            assert_eq!(get(fd), 0);
            assert_eq!(close([fd, 0, 0, 0, 0, 0]), 0);
        }
    }
}
