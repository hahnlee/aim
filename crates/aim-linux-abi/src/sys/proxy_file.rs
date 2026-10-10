//! Syscall view of an explicitly typed native proxy-file Binder capability.
use super::fdtab::{self, Kind};
use crate::errno;
use aim_binder_host::proxy_file::{self, Operation};
pub fn is_proxy(fd: i32) -> bool {
    matches!(fdtab::get(fd), Some(Kind::ProxyFile))
}
fn call(
    fd: i32,
    op: Operation,
    offset: i64,
    count: u32,
    data: &[u8],
) -> Result<(i64, Vec<u8>), i64> {
    proxy_file::request(fd, op, offset, count, data)
        .map_err(|error| -(errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)) as i64))
}
pub fn rw(fd: i32, iov: &[libc::iovec], pos: Option<i64>, write: bool) -> i64 {
    let mut length = 0usize;
    for v in iov {
        length = match length.checked_add(v.iov_len) {
            Some(n) => n.min(proxy_file::MAX_DATA),
            None => return -22,
        };
    }
    let mut input = Vec::new();
    if write {
        if input.try_reserve_exact(length).is_err() {
            return -12;
        }
        for v in iov {
            let count = v.iov_len.min(length - input.len());
            // SAFETY: the syscall caller supplied this guest iovec.
            input.extend_from_slice(unsafe {
                std::slice::from_raw_parts(v.iov_base.cast::<u8>(), count)
            });
            if input.len() == length {
                break;
            }
        }
    }
    let op = match (write, pos.is_some()) {
        (false, false) => Operation::Read,
        (true, false) => Operation::Write,
        (false, true) => Operation::Pread,
        (true, true) => Operation::Pwrite,
    };
    let (n, data) = match call(fd, op, pos.unwrap_or(0), length as u32, &input) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if !write {
        if data.len() != n as usize || data.len() > length {
            return -5;
        }
        let mut consumed = 0;
        for v in iov {
            let count = v.iov_len.min(data.len() - consumed);
            // SAFETY: validated guest iovec, bounded by returned bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(consumed),
                    v.iov_base.cast::<u8>(),
                    count,
                );
            }
            consumed += count;
            if consumed == data.len() {
                break;
            }
        }
    }
    n
}

pub fn sync(fd: i32) -> i64 {
    call(fd, Operation::Sync, 0, 0, &[]).map_or_else(|e| e, |v| v.0)
}
pub fn seek(fd: i32, offset: i64, whence: u32) -> i64 {
    call(fd, Operation::Seek, offset, whence, &[]).map_or_else(|e| e, |v| v.0)
}
pub fn size(fd: i32) -> Result<i64, i64> {
    call(fd, Operation::Size, 0, 0, &[]).map(|v| v.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;
    #[test]
    fn linux_io_dup_offsets_positioned_io_stat_and_revoke_use_native_owner() {
        // fork_restore mutates a process-wide child namespace, so exercise
        // that boundary in an actual child rather than other tests' process.
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","sys::proxy_file::tests::proxy_linux_io_controlled_child","--ignored","--nocapture"])
            .stdin(std::process::Stdio::piped()).spawn().unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"AIM-PROXY-CHILD1\n").unwrap();
        assert!(child.wait().unwrap().success());
    }
    #[test]
    #[ignore="controlled child of linux_io_dup_offsets_positioned_io_stat_and_revoke_use_native_owner"]
    fn proxy_linux_io_controlled_child() {
        let mut input=String::new();std::io::stdin().read_line(&mut input).unwrap();assert_eq!(input,"AIM-PROXY-CHILD1\n");
        let path = std::env::temp_dir().join(format!("aim-proxy-linux-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let (owner, client, worker) = proxy_file::open(file).unwrap();
        let fd = client.into_raw_fd();
        fdtab::insert(fd, Kind::ProxyFile);
        fdtab::publish_guest(fd).unwrap();
        let dup = super::super::fs::dup([fd as u64, 0, 0, 0, 0, 0]) as i32;
        assert!(is_proxy(dup));
        let mut writer = super::super::fork_state::Writer::default();
        // Use the real snapshot schema, including hidden and visible roles.
        fdtab::fork_save(&mut writer);
        let saved = writer.into_bytes();
        fdtab::withdraw_guest(fd).unwrap();
        fdtab::withdraw_guest(dup).unwrap();
        fdtab::on_close(fd);
        fdtab::on_close(dup);
        let mut reader = super::super::fork_state::Reader::new(&saved);
        fdtab::fork_restore(&mut reader);
        assert!(is_proxy(fd) && is_proxy(dup));
        assert!(super::super::net::hidden_socket(fd));
        let bytes = b"native";
        assert_eq!(
            super::super::fs::write([fd as u64, bytes.as_ptr() as u64, 6, 0, 0, 0]),
            6
        );
        assert_eq!(super::super::fs::lseek([dup as u64, 0, 0, 0, 0, 0]), 0);
        let mut output = [0u8; 3];
        assert_eq!(
            super::super::fs::read([dup as u64, output.as_mut_ptr() as u64, 3, 0, 0, 0]),
            3
        );
        assert_eq!(&output, b"nat");
        assert_eq!(
            super::super::fs::read([fd as u64, output.as_mut_ptr() as u64, 3, 0, 0, 0]),
            3
        );
        assert_eq!(&output, b"ive");
        assert_eq!(
            super::super::fs::pread64([fd as u64, output.as_mut_ptr() as u64, 3, 1, 0, 0]),
            3
        );
        assert_eq!(&output, b"ati");
        assert_eq!(super::super::fs::lseek([fd as u64, 0, 1, 0, 0, 0]), 6);
        assert_eq!(super::super::fsops::fsync([fd as u64, 0, 0, 0, 0, 0]), 0);
        let mut stat = [0u64; 16];
        assert_eq!(
            super::super::fs::fstat([fd as u64, stat.as_mut_ptr() as u64, 0, 0, 0, 0]),
            0
        );
        let raw = unsafe { std::slice::from_raw_parts(stat.as_ptr().cast::<u8>(), 128) };
        assert_eq!(
            u32::from_ne_bytes(raw[16..20].try_into().unwrap()),
            0o100777
        );
        assert_eq!(i64::from_ne_bytes(raw[48..56].try_into().unwrap()), 6);
        assert_eq!(size(fd), Ok(6));
        assert_eq!(seek(fd, 0, 0), 0);
        let a = vec![b'A'; 8192];
        let b = vec![b'B'; 8192];
        let iov = [
            libc::iovec {
                iov_base: a.as_ptr() as *mut _,
                iov_len: a.len(),
            },
            libc::iovec {
                iov_base: b.as_ptr() as *mut _,
                iov_len: b.len(),
            },
        ];
        assert_eq!(
            super::super::fs::writev([fd as u64, iov.as_ptr() as u64, 2, 0, 0, 0]),
            16384
        );
        assert_eq!(seek(dup, 0, 1), 16384);
        let actual = std::fs::read(&path).unwrap();
        assert_eq!(&actual[..8192], &a);
        assert_eq!(&actual[8192..], &b);
        owner.revoke();
        assert_eq!(
            super::super::fs::read([dup as u64, output.as_mut_ptr() as u64, 3, 0, 0, 0]),
            -1
        );
        assert_eq!(super::super::fsops::fsync([fd as u64, 0, 0, 0, 0, 0]), -1);
        assert_eq!(seek(fd, 0, 0), -1);
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]),0);
        assert_eq!(super::super::fs::close([dup as u64, 0, 0, 0, 0, 0]),0);
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }
}
