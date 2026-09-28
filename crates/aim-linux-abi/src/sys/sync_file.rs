//! Linux `sync_file` (include/uapi/linux/sync_file.h): the fences the GPU
//! and display drivers hand out (docs/graphics-buffers.md, "Fences").
//!
//! A sync_file is a host datagram socket made by `aim_sync_file`, which the
//! GPU module, the display server and [`SYNC_IOC_MERGE`] signal once. `poll`,
//! `epoll` and `select` wait for it as for any fd; here are its ioctls and
//! the rest of its Linux behaviour: `read` and `write` fail with `EINVAL`,
//! and `/proc/self/fd` shows it as `anon_inode:sync_file`.

use aim_sync_file::State;
use std::os::fd::BorrowedFd;

use super::fdtab::{self, Kind};
use super::fork_state::{Reader, Writer};
use crate::errno::{EFAULT, EINVAL, ENOENT, ENOTTY};

/// `_IOWR('>', 3, struct sync_merge_data)`.
const SYNC_IOC_MERGE: u64 = 0xc030_3e03;
/// `_IOWR('>', 4, struct sync_file_info)`.
const SYNC_IOC_FILE_INFO: u64 = 0xc038_3e04;
/// `_IOW('>', 5, struct sync_set_deadline)`.
const SYNC_IOC_SET_DEADLINE: u64 = 0x4010_3e05;
/// The ioctl type byte of `SYNC_IOC_*`.
const SYNC_IOC_MAGIC: u64 = b'>' as u64;

/// The name every fence reports (Linux: the driver and timeline).
const NAME: &[u8] = b"aim";
const DRIVER: &[u8] = b"aim";

#[repr(C)]
#[derive(Clone, Copy)]
struct MergeData {
    name: [u8; 32],
    fd2: i32,
    fence: i32,
    flags: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FenceInfo {
    obj_name: [u8; 32],
    driver_name: [u8; 32],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}

const _: () = assert!(size_of::<MergeData>() == 48);
const _: () = assert!(size_of::<FileInfo>() == 56);
const _: () = assert!(size_of::<FenceInfo>() == 80);

fn name32(s: &[u8]) -> [u8; 32] {
    let mut n = [0; 32];
    n[..s.len()].copy_from_slice(s);
    n
}

/// Install the hooks through which `aim_sync_file` keeps its own fds out
/// of the guest's view and gives fences to the guest.
pub fn init() {
    aim_sync_file::set_hooks(aim_sync_file::Hooks {
        hide: fdtab::hide,
        unhide: fdtab::unhide,
        adopt,
    });
}

/// `fd` (new to the guest, or arrived from elsewhere) is a sync_file.
pub fn adopt(fd: i32) {
    fdtab::insert(fd, Kind::SyncFile);
}

/// Whether `fd` is a sync_file; one that arrived unrecognized is adopted.
fn is_sync_file(fd: i32) -> bool {
    match fdtab::get(fd) {
        Some(Kind::SyncFile) => true,
        Some(_) => false,
        // SAFETY: borrowed only for the check; a closed fd is not a socket.
        None if aim_sync_file::is_sync_file(unsafe { BorrowedFd::borrow_raw(fd) }) => {
            adopt(fd);
            true
        }
        None => false,
    }
}

fn state(fd: i32) -> State {
    // SAFETY: a guest fd checked to be a sync_file, borrowed for the call.
    aim_sync_file::state(unsafe { BorrowedFd::borrow_raw(fd) })
}

/// The `SYNC_IOC_*` ioctls. None: not a sync_file request.
pub fn ioctl(fd: i32, req: u64, arg: u64) -> Option<i64> {
    if (req >> 8) & 0xff != SYNC_IOC_MAGIC || !is_sync_file(fd) {
        return None;
    }
    if arg == 0 {
        return Some(-(EFAULT as i64));
    }
    Some(match req {
        SYNC_IOC_MERGE => merge(fd, arg),
        SYNC_IOC_FILE_INFO => file_info(fd, arg),
        SYNC_IOC_SET_DEADLINE => {
            // SAFETY: the guest's struct sync_set_deadline.
            let d = unsafe { (arg as *const [u64; 2]).read_unaligned() };
            // The deadline is a hint; the host GPU has no clock to boost.
            if d[1] != 0 { -(EINVAL as i64) } else { 0 }
        }
        _ => -(ENOTTY as i64),
    })
}

fn merge(fd: i32, arg: u64) -> i64 {
    // SAFETY: the guest's struct sync_merge_data.
    let mut d = unsafe { (arg as *const MergeData).read_unaligned() };
    if d.flags != 0 || d.pad != 0 {
        return -(EINVAL as i64);
    }
    if !is_sync_file(d.fd2) {
        return -(ENOENT as i64);
    }
    // SAFETY: both are open guest sync_files, borrowed for the call.
    let merged =
        unsafe { aim_sync_file::merge(BorrowedFd::borrow_raw(fd), BorrowedFd::borrow_raw(d.fd2)) };
    match merged {
        Ok(file) => {
            d.fence = aim_sync_file::give_to_guest(file);
            // SAFETY: the guest's struct, written back with the new fd.
            unsafe { (arg as *mut MergeData).write_unaligned(d) };
            0
        }
        Err(e) => -(e.raw_os_error().unwrap_or(libc::ENOMEM) as i64),
    }
}

fn file_info(fd: i32, arg: u64) -> i64 {
    // SAFETY: the guest's struct sync_file_info.
    let mut info = unsafe { (arg as *const FileInfo).read_unaligned() };
    if info.flags != 0 || info.pad != 0 {
        return -(EINVAL as i64);
    }
    // One fence per file: a merge signals its own fence once all of its
    // inputs have, with the last time and the first error.
    let (status, timestamp_ns) = match state(fd) {
        State::Active => (0, 0),
        State::Signaled {
            timestamp_ns,
            status,
        } => (status, timestamp_ns as u64),
    };
    if info.num_fences != 0 {
        if info.sync_fence_info == 0 {
            return -(EFAULT as i64);
        }
        let fence = FenceInfo {
            obj_name: name32(NAME),
            driver_name: name32(DRIVER),
            status,
            flags: 0,
            timestamp_ns,
        };
        // SAFETY: the guest's array of at least one struct sync_fence_info.
        unsafe { (info.sync_fence_info as *mut FenceInfo).write_unaligned(fence) };
    }
    info.status = status;
    info.name = name32(NAME);
    info.num_fences = 1;
    // SAFETY: the guest's struct, written back.
    unsafe { (arg as *mut FileInfo).write_unaligned(info) };
    0
}

/// The fds this process keeps for its pending fences, which a fork child
/// inherits.
pub(super) fn fork_save(w: &mut Writer) {
    w.seq(aim_sync_file::inherited().into_iter(), |w, fd| w.i32(fd));
}

/// A fork child closes the parent's pending-fence fds: the parent signals
/// those fences.
pub(super) fn fork_restore(r: &mut Reader) {
    init();
    aim_sync_file::close_inherited(&r.seq(|r| r.i32()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};

    /// A new guest sync_file and its writer.
    fn guest_fence() -> (i32, aim_sync_file::Writer) {
        let (file, writer) = aim_sync_file::pair().unwrap();
        (aim_sync_file::give_to_guest(file), writer)
    }

    /// Close a guest fd the way the guest does.
    fn close(fd: i32) {
        fdtab::on_close(fd);
        // SAFETY: a test fd, closed once.
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
    }

    fn info(fd: i32, with_fence: bool) -> (i64, FileInfo, FenceInfo) {
        // SAFETY: plain old data.
        let mut fence: FenceInfo = unsafe { std::mem::zeroed() };
        // SAFETY: plain old data.
        let mut info: FileInfo = unsafe { std::mem::zeroed() };
        if with_fence {
            info.num_fences = 1;
            info.sync_fence_info = &mut fence as *mut FenceInfo as u64;
        }
        let r = ioctl(fd, SYNC_IOC_FILE_INFO, &mut info as *mut FileInfo as u64).unwrap();
        (r, info, fence)
    }

    #[test]
    fn file_info_reports_the_state_and_signal_time() {
        init();
        let (fd, writer) = guest_fence();
        let (r, i, _) = info(fd, false);
        assert_eq!((r, i.status, i.num_fences), (0, 0, 1));
        assert_eq!(&i.name[..4], b"aim\0");
        writer.signal_at(1234, 1);
        let (r, i, f) = info(fd, true);
        assert_eq!((r, i.status, i.num_fences), (0, 1, 1));
        assert_eq!((f.status, f.timestamp_ns), (1, 1234));
        assert_eq!(&f.driver_name[..4], b"aim\0");
        // Flags must be zero.
        // SAFETY: plain old data.
        let mut bad: FileInfo = unsafe { std::mem::zeroed() };
        bad.flags = 1;
        let r = ioctl(fd, SYNC_IOC_FILE_INFO, &mut bad as *mut FileInfo as u64);
        assert_eq!(r, Some(-(EINVAL as i64)));
        close(fd);
    }

    #[test]
    fn merge_makes_a_new_fence_and_checks_its_arguments() {
        init();
        let (a, wa) = guest_fence();
        let (b, wb) = guest_fence();
        // SAFETY: plain old data.
        let mut d: MergeData = unsafe { std::mem::zeroed() };
        d.fd2 = b;
        let r = ioctl(a, SYNC_IOC_MERGE, &mut d as *mut MergeData as u64);
        assert_eq!(r, Some(0));
        let m = d.fence;
        assert!(m > 2 && m != a && m != b);
        assert!(matches!(fdtab::get(m), Some(Kind::SyncFile)));
        assert_eq!(info(m, false).1.status, 0);
        wa.signal_at(5, 1);
        wb.signal_at(7, 1);
        // SAFETY: a guest fd borrowed for the wait.
        assert!(aim_sync_file::wait(
            unsafe { BorrowedFd::borrow_raw(m) },
            5000
        ));
        let (_, i, f) = info(m, true);
        assert_eq!((i.status, f.timestamp_ns), (1, 7));

        // fd2 that is not a sync_file, and nonzero flags.
        let (s, t) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let other = std::os::fd::AsRawFd::as_raw_fd(&s);
        d.fd2 = other;
        let r = ioctl(a, SYNC_IOC_MERGE, &mut d as *mut MergeData as u64);
        assert_eq!(r, Some(-(ENOENT as i64)));
        d.fd2 = b;
        d.flags = 1;
        let r = ioctl(a, SYNC_IOC_MERGE, &mut d as *mut MergeData as u64);
        assert_eq!(r, Some(-(EINVAL as i64)));
        // Other sockets are not sync_files, and their ioctls go elsewhere.
        let r = ioctl(other, SYNC_IOC_FILE_INFO, &mut d as *mut MergeData as u64);
        assert_eq!(r, None);
        drop((s, t));
        for fd in [a, b, m] {
            close(fd);
        }
    }

    #[test]
    fn a_fence_from_elsewhere_is_recognized() {
        init();
        let (file, writer) = aim_sync_file::pair().unwrap();
        // Arrived by SCM_RIGHTS or binder: not in the table yet.
        let fd = std::os::fd::IntoRawFd::into_raw_fd(file);
        assert!(fdtab::get(fd).is_none());
        writer.signal(1);
        assert_eq!(info(fd, false).1.status, 1);
        assert!(matches!(fdtab::get(fd), Some(Kind::SyncFile)));
        let r = ioctl(fd, SYNC_IOC_SET_DEADLINE, [0u64; 2].as_ptr() as u64);
        assert_eq!(r, Some(0));
        let r = ioctl(fd, 0xc0083e7f, [0u64; 2].as_ptr() as u64);
        assert_eq!(r, Some(-(ENOTTY as i64)));
        close(fd);
    }
}
