//! Tracefs instances (Documentation/trace/ftrace.rst, "Instances").
//! The error_report tracepoint has no producer on Darwin. Its receive
//! queue stays empty: native reads block and poll/kqueue see no input.

use super::{dir, fdtab, procfs};
use crate::{errno, vfs};
use aim_storage::private_fd::PrivateFd;
use procfs::{Node, entries};
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::Arc,
};

pub(super) struct Open {
    pub path: String,
    instance: Option<PrivateFd>,
}

fn io_error(error: std::io::Error) -> errno::Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}

fn lock(path: &std::path::Path, operation: i32) -> Result<PrivateFd, errno::Errno> {
    let file = PrivateFd::allocate(|| Ok(std::fs::File::open(path)?.into())).map_err(io_error)?;
    if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } < 0 {
        return Err(match errno::last() {
            errno::EAGAIN => errno::EBUSY,
            error => error,
        });
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } < 0 {
        return Err(errno::last());
    }
    if stat.st_nlink == 0 {
        return Err(errno::ENOENT);
    }
    Ok(file)
}

impl Open {
    pub(super) fn save(&self, w: &mut super::fork_state::Writer) {
        w.str(&self.path);
        w.opt(self.instance.as_ref(), |w, file| {
            let fd = file.as_raw_fd();
            w.retain_private(fd);
            w.i32(fd);
        });
    }

    pub(super) fn load(r: &mut super::fork_state::Reader) -> Result<Arc<Self>, errno::Errno> {
        let path = r.str();
        let instance = r
            .opt(|r| {
                let fd = r.i32();
                fdtab::remember_restored_private(&[fd]);
                PrivateFd::allocate(|| {
                    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
                    if copy < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
                })
                .map_err(io_error)
            })
            .transpose()?;
        Ok(Arc::new(Self { path, instance }))
    }
}

const ROOT: &str = "/sys/kernel/tracing";

pub(super) fn tail(guest: &str) -> Option<&str> {
    [ROOT, "/sys/kernel/debug/tracing"].iter().find_map(|root| {
        let tail = guest.strip_prefix(root)?;
        (tail.is_empty() || tail.starts_with('/')).then(|| tail.trim_start_matches('/'))
    })
}

fn instance(tail: &str) -> Option<(&str, &str)> {
    let tail = tail.strip_prefix("instances/")?;
    let (name, rest) = tail.split_once('/').unwrap_or((tail, ""));
    (!name.is_empty()).then_some((name, rest))
}

pub(super) fn node(tail: &str) -> Option<Node> {
    let mut rest = tail;
    if let Some((name, child)) = instance(tail) {
        if !vfs::lookup(&format!("{ROOT}/instances/{name}")).0.is_dir() {
            return None;
        }
        rest = child;
    }
    Some(match rest {
        "" => {
            let mut list = entries(&[
                ("trace_pipe", dir::DT_REG),
                ("trace_marker", dir::DT_REG),
                ("tracing_on", dir::DT_REG),
                ("events", dir::DT_DIR),
                ("buffer_size_kb", dir::DT_REG),
                ("trace_options", dir::DT_REG),
            ]);
            if tail.is_empty() {
                list.push(dir::Entry::new(1, dir::DT_DIR, "instances"));
            }
            Node::Dir(list)
        }
        "instances" if tail == rest => Node::Dir(Vec::new()),
        "trace_pipe" | "trace_marker" => Node::File(Vec::new()),
        "tracing_on" if tail == rest => Node::File(b"0\n".to_vec()),
        "tracing_on" => Node::File(b"1\n".to_vec()),
        "buffer_size_kb" => Node::File(b"7\n".to_vec()),
        "trace_options" => Node::File(b"\n".to_vec()),
        "events" => Node::Dir(entries(&[("error_report", dir::DT_DIR)])),
        "events/error_report" => Node::Dir(entries(&[("error_report_end", dir::DT_DIR)])),
        "events/error_report/error_report_end" => Node::Dir(entries(&[("enable", dir::DT_REG)])),
        "events/error_report/error_report_end/enable" => Node::File(b"0\n".to_vec()),
        _ => return None,
    })
}

pub(super) fn prepare_mkdir(guest: &str) -> Result<(), errno::Errno> {
    let Some(tail) = tail(guest) else {
        return Ok(());
    };
    if !matches!(instance(tail), Some((_, ""))) {
        return Err(errno::EPERM);
    }
    let (host, area) = vfs::lookup(guest);
    if area != vfs::Area::Kernfs {
        return Err(errno::EACCES);
    }
    std::fs::create_dir_all(host.parent().unwrap())
        .map_err(|e| errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))
}

pub(super) fn remove(guest: &str, host: &std::path::Path, directory: bool) -> Option<i64> {
    let tail = tail(guest)?;
    if !directory || !matches!(instance(tail), Some((_, ""))) {
        return Some(-(errno::EPERM as i64));
    }
    // The shared lease follows the open description across dup and fork.
    let _exclusive = match lock(host, libc::LOCK_EX) {
        Ok(file) => file,
        Err(error) => return Some(-(error as i64)),
    };
    Some(
        std::fs::remove_dir_all(host)
            .map(|_| 0)
            .unwrap_or_else(|e| {
                -(errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)) as i64)
            }),
    )
}

pub(super) fn open(guest: &str, flags: u64) -> Option<i64> {
    let tail = tail(guest)?;
    if tail != "trace_pipe" && !matches!(instance(tail), Some((_, "trace_pipe"))) {
        return None;
    }
    if node(tail).is_none() {
        return Some(-(errno::ENOENT as i64));
    }
    if flags & 3 != 0 {
        return Some(-(errno::EACCES as i64));
    }
    let instance = if let Some((name, _)) = instance(tail) {
        match lock(
            &vfs::lookup(&format!("{ROOT}/instances/{name}")).0,
            libc::LOCK_SH,
        ) {
            Ok(file) => Some(file),
            Err(error) => return Some(-(error as i64)),
        }
    } else {
        None
    };
    // An unconnected datagram queue has neither data nor an EOF-producing
    // peer, and survives dup/fork without a hidden writer descriptor.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Some(-(errno::last() as i64));
    }
    fdtab::set_flags(fd, flags & 0o4000 != 0, flags & 0o2000000 != 0);
    fdtab::insert(
        fd,
        fdtab::Kind::TracePipe(Arc::new(Open {
            path: guest.to_string(),
            instance,
        })),
    );
    if let Err(error) = fdtab::publish_guest(fd) {
        fdtab::on_close(fd);
        unsafe { libc::close(fd) };
        return Some(-(error as i64));
    }
    Some(fd as i64)
}

#[cfg(test)]
mod tests {
    use super::super::{fs, fsops};
    use super::*;
    use std::ffi::CString;

    fn read_control(path: &str, expected: &[u8]) {
        let path = CString::new(path).unwrap();
        let fd = fs::openat([
            vfs::LINUX_AT_FDCWD as u64,
            path.as_ptr() as u64,
            0o2000000,
            0,
            0,
            0,
        ]);
        assert!(fd >= 0, "open {path:?}: {fd}");
        let mut bytes = [0u8; 64];
        let count = fs::read([fd as u64, bytes.as_mut_ptr() as u64, 64, 0, 0, 0]);
        let mut byte = 0u8;
        let eof = fs::read([fd as u64, &mut byte as *mut _ as u64, 1, 0, 0, 0]);
        assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(count, expected.len() as i64, "read {path:?}");
        assert_eq!(&bytes[..expected.len()], expected, "content {path:?}");
        assert_eq!(eof, 0, "EOF {path:?}");
    }

    #[test]
    fn top_level_tracing_on() {
        if fdtab::isolated_kernel_test("sys::tracefs::tests::top_level_tracing_on") {
            return;
        }
        let (_view, root) = vfs::test_view();
        let host = root.join("tracefs-sys");
        std::fs::create_dir_all(&host).unwrap();
        vfs::add_mount("/sys", host.clone(), vfs::Area::Kernfs, "sysfs", "sysfs").unwrap();
        for root in [ROOT, "/sys/kernel/debug/tracing"] {
            let marker = CString::new(format!("{root}/trace_marker")).unwrap();
            let fd = fs::openat([
                vfs::LINUX_AT_FDCWD as u64,
                marker.as_ptr() as u64,
                0o2000001,
                0,
                0,
                0,
            ]);
            assert!(fd >= 0, "open {marker:?}: {fd}");
            assert_eq!(
                fs::write([fd as u64, b"E|1".as_ptr() as u64, 3, 0, 0, 0]),
                3
            );
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
            read_control(&format!("{root}/tracing_on"), b"0\n");
        }
        std::fs::remove_dir_all(host).unwrap();
    }

    #[test]
    fn tracefs_instances() {
        if fdtab::isolated_kernel_test("sys::tracefs::tests::tracefs_instances") {
            return;
        }
        let (_view, root) = vfs::test_view();
        let host = root.join("tracefs-sys");
        std::fs::create_dir_all(&host).unwrap();
        vfs::add_mount("/sys", host.clone(), vfs::Area::Kernfs, "sysfs", "sysfs").unwrap();
        let cwd = vfs::LINUX_AT_FDCWD as u64;
        let path = |tail: &str| CString::new(format!("{ROOT}/instances/test{tail}")).unwrap();
        let open =
            |tail: &str, flags| fs::openat([cwd, path(tail).as_ptr() as u64, flags, 0, 0, 0]);
        assert_eq!(open("/trace_pipe", 0), -(errno::ENOENT as i64));
        let make = || fsops::mkdirat([cwd, path("").as_ptr() as u64, 0o700, 0, 0, 0]);
        assert_eq!(make(), 0);
        assert_eq!(make(), -(errno::EEXIST as i64));
        read_control(path("/tracing_on").to_str().unwrap(), b"1\n");
        let stat = procfs::stat(path("").to_str().unwrap(), true)
            .unwrap()
            .unwrap();
        assert_eq!(stat.st_mode & 0o777, 0o700);
        assert_eq!(
            stat.st_uid,
            super::super::attrs::ids(super::super::attrs::FS).0
        );
        assert!(matches!(
            node("instances/test/events/error_report"),
            Some(Node::Dir(_))
        ));
        let enable = open("/events/error_report/error_report_end/enable", 1);
        assert!(enable >= 0, "{enable}");
        assert_eq!(
            fs::write([enable as u64, b"1".as_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(fs::close([enable as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(
            open("/events/error_report/missing/enable", 1),
            -(errno::ENOENT as i64)
        );
        let pipe = open("/trace_pipe", 0o4000);
        assert!(pipe >= 0, "{pipe}");
        let mut byte = 0u8;
        assert_eq!(
            fs::read([pipe as u64, &mut byte as *mut _ as u64, 1, 0, 0, 0]),
            -(errno::EAGAIN as i64)
        );
        let mut poll = libc::pollfd {
            fd: pipe as i32,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 0);
        let ep = super::super::epoll::epoll_create1([0; 6]);
        let mut event = [1u64, pipe as u64];
        assert!(ep >= 0);
        assert_eq!(
            super::super::epoll::epoll_ctl([
                ep as u64,
                1,
                pipe as u64,
                event.as_ptr() as u64,
                0,
                0
            ]),
            0
        );
        assert_eq!(
            super::super::epoll::epoll_pwait([ep as u64, event.as_mut_ptr() as u64, 1, 0, 0, 8]),
            0
        );
        // A blocking descriptor still has no readable data or EOF.
        assert_eq!(fs::fcntl([pipe as u64, 4, 0, 0, 0, 0]), 0);
        assert_eq!(fs::fcntl([pipe as u64, 3, 0, 0, 0, 0]) & 0o4000, 0);
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 0);
        assert_eq!(
            super::super::epoll::epoll_pwait([ep as u64, event.as_mut_ptr() as u64, 1, 0, 0, 8]),
            0
        );
        fs::close([ep as u64, 0, 0, 0, 0, 0]);
        let remove = || fsops::unlinkat([cwd, path("").as_ptr() as u64, 0x200, 0, 0, 0]);
        assert_eq!(remove(), -(errno::EBUSY as i64));
        let alias = fs::dup([pipe as u64, 0, 0, 0, 0, 0]);
        assert!(alias >= 0);
        assert_eq!(fs::close([pipe as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(remove(), -(errno::EBUSY as i64));
        assert_eq!(fs::close([alias as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(remove(), 0);
        assert_eq!(open("/trace_pipe", 0), -(errno::ENOENT as i64));
        assert_eq!(make(), 0);
        let enable = open("/events/error_report/error_report_end/enable", 0);
        assert_eq!(
            fs::read([enable as u64, &mut byte as *mut _ as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(byte, b'0');
        fs::close([enable as u64, 0, 0, 0, 0, 0]);
        std::fs::remove_dir_all(host).unwrap();
    }
}
