//! Free space on the host volumes the guest writes to, with a reserve the
//! guest never gets (docs/storage.md).
//!
//! The guest's `/data` is a sparse disk image that grows into the Mac's
//! disk. When the Mac's disk fills, the image cannot grow, its writes fail
//! with EIO and the image is detached under the running guest. So the last
//! [`RESERVE`] bytes of each volume behind a writable path map entry are
//! kept out of the guest's reach, as a Linux filesystem keeps its reserved
//! blocks: statfs reports them as used, and a write that would reach them
//! is cut short, or fails with ENOSPC when nothing is left. APFS in a
//! sparse image reports the host volume's free space as its own (it is
//! thin provisioned), so the image's free space is the host's.
//!
//! Checking the volume on every write would cost a syscall each; a
//! process instead holds a [`BUDGET`] of bytes it may write unchecked, a
//! 64th of the room left (at most [`MAX_BUDGET`]), so that many processes
//! writing at once still stop short of the reserve. `trampoline.S` charges
//! write and pwrite to it on the lean path; once it runs out, the next
//! write comes here and looks again. Writes through a shared mapping are
//! not counted; the reserve absorbs them.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::errno::ENOSPC;
use crate::vfs;

/// Space on the host volume the guest never gets.
pub const RESERVE: u64 = 1 << 30;
const MAX_BUDGET: u64 = 16 << 20;

/// Bytes this process may write before it looks at the volumes again.
/// Read and lowered by the lean path in `trampoline.S` without atomics
/// (a lost update only brings the next look closer or later by one write).
pub static BUDGET: AtomicU64 = AtomicU64::new(0);

/// A volume behind a writable path map entry.
struct Volume {
    fsid: [i32; 2],
    path: CString,
}

fn fsid(s: &libc::statfs) -> [i32; 2] {
    // SAFETY: fsid_t is two i32 values.
    unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(s.f_fsid) }
}

fn statfs(path: &CString) -> Option<libc::statfs> {
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    (unsafe { libc::statfs(path.as_ptr(), &mut s) } == 0).then_some(s)
}

/// The volumes, once the path map is known; none before (a process
/// without one, such as a test of the syscall layer, has no reserve).
fn volumes() -> &'static [Volume] {
    static VOLUMES: OnceLock<Vec<Volume>> = OnceLock::new();
    if let Some(v) = VOLUMES.get() {
        return v;
    }
    let Some(hosts) = vfs::writable_hosts() else {
        return &[];
    };
    VOLUMES.get_or_init(|| {
        let mut out: Vec<Volume> = Vec::new();
        for host in hosts {
            let Ok(path) = CString::new(host.as_os_str().as_bytes()) else {
                continue;
            };
            if let Some(s) = statfs(&path)
                && !out.iter().any(|v| v.fsid == fsid(&s))
            {
                out.push(Volume {
                    fsid: fsid(&s),
                    path,
                });
            }
        }
        out
    })
}

fn reserved_blocks(s: &libc::statfs) -> u64 {
    RESERVE.div_ceil(u64::from(s.f_bsize.max(1)))
}

/// Bytes the guest may still write to the volume of `s`.
fn room(s: &libc::statfs) -> u64 {
    s.f_bavail.saturating_sub(reserved_blocks(s)) * u64::from(s.f_bsize)
}

/// The guest's view of a host volume: the reserve is not free.
pub fn adjust(s: &mut libc::statfs) {
    if volumes().iter().any(|v| v.fsid == fsid(s)) {
        let r = reserved_blocks(s);
        s.f_bfree = s.f_bfree.saturating_sub(r);
        s.f_bavail = s.f_bavail.saturating_sub(r);
    }
}

/// How many of `len` bytes a write to `fd` may put on its volume: `len`
/// unless the volume is short of room, then what is left, or ENOSPC.
pub fn charge(fd: i32, len: u64) -> Result<u64, i64> {
    let budget = BUDGET.load(Ordering::Relaxed);
    if budget >= len {
        BUDGET.store(budget - len, Ordering::Relaxed);
        return Ok(len);
    }
    let least = volumes()
        .iter()
        .filter_map(|v| statfs(&v.path))
        .map(|s| room(&s))
        .min()
        .unwrap_or(u64::MAX);
    BUDGET.store((least / 64).min(MAX_BUDGET), Ordering::Relaxed);
    if least >= len {
        return Ok(len);
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: plain fstat and fstatfs into local buffers.
    let on_volume = unsafe { libc::fstat(fd, &mut st) == 0 && libc::fstatfs(fd, &mut s) == 0 }
        && st.st_mode & libc::S_IFMT == libc::S_IFREG
        && volumes().iter().any(|v| v.fsid == fsid(&s));
    match if on_volume { room(&s).min(len) } else { len } {
        0 => Err(-(ENOSPC as i64)),
        n => Ok(n),
    }
}
