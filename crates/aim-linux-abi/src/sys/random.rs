//! `/dev/random` and `/dev/urandom` opened for writing. On Linux both are
//! mode 0666 and any process may write them: the bytes are mixed into the
//! input pool without entropy credit (random(4)), so a write returns its
//! length and changes nothing a reader can tell. Darwin's random device
//! takes writes from root only, and its pool is the kernel's own; the
//! layer takes the guest's write as Linux does, counting the bytes, and
//! leaves the host pool to seed itself. Reads stay the host device's.

use std::sync::LazyLock;

use super::fdtab::{self, Kind};
use crate::errno::EINVAL;

/// The device numbers of the host's random devices.
static DEVICES: LazyLock<Vec<libc::dev_t>> = LazyLock::new(|| {
    [c"/dev/random", c"/dev/urandom"]
        .iter()
        .filter_map(|path| {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: NUL-terminated path, local buffer.
            (unsafe { libc::stat(path.as_ptr(), &mut st) } == 0).then_some(st.st_rdev)
        })
        .collect()
});

/// Gives `fd` Linux's write semantics if it is a random device open for
/// writing (at open, and for an fd that arrived from elsewhere).
pub fn adopt(fd: i32) {
    // SAFETY: plain fcntl and fstat on a guest fd.
    let writable = unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_ACCMODE != libc::O_RDONLY;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if writable
        && unsafe { libc::fstat(fd, &mut st) } == 0
        && st.st_mode & libc::S_IFMT == libc::S_IFCHR
        && DEVICES.contains(&st.st_rdev)
    {
        fdtab::insert(fd, Kind::Random);
    }
}

/// write/writev/pwrite: the whole write is taken.
pub fn write(iov: &[libc::iovec]) -> i64 {
    iov.iter()
        .try_fold(0u64, |t, v| t.checked_add(v.iov_len as u64))
        .filter(|&t| t <= isize::MAX as u64)
        .map_or(-(EINVAL as i64), |t| t as i64)
}
