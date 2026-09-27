//! Walking this process's VM map, for `/proc/self/maps`, madvise and memfd
//! views.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

const PROC_PIDREGIONPATHINFO: i32 = 8;
const VM_INHERIT_SHARE: u32 = 0;

/// `struct proc_regioninfo` (sys/proc_info.h).
#[repr(C)]
struct ProcRegionInfo {
    protection: u32,
    max_protection: u32,
    inheritance: u32,
    flags: u32,
    offset: u64,
    behavior: u32,
    user_wired_count: u32,
    user_tag: u32,
    pages_resident: u32,
    pages_shared_now_private: u32,
    pages_swapped_out: u32,
    pages_dirtied: u32,
    ref_count: u32,
    shadow_depth: u32,
    share_mode: u32,
    private_pages_resident: u32,
    shared_pages_resident: u32,
    obj_id: u32,
    depth: u32,
    address: u64,
    size: u64,
}

#[repr(C)]
struct ProcRegionWithPathInfo {
    info: ProcRegionInfo,
    vip: libc::vnode_info_path,
}

pub struct Region {
    pub start: u64,
    pub end: u64,
    /// Darwin VM_PROT bits (read 1, write 2, execute 4).
    pub prot: u32,
    /// Mapped shared (inherited as shared by fork children).
    pub shared: bool,
    /// File offset of `start`, for file-backed regions.
    pub offset: u64,
    /// The backing file, if any: host path, device and inode.
    pub file: Option<(PathBuf, u64, u64)>,
}

/// A raw VM map entry: bounds and Darwin VM tag.
pub struct Entry {
    pub start: u64,
    pub end: u64,
    pub tag: u32,
}

fn entry_info(addr: u64) -> Option<ProcRegionWithPathInfo> {
    let mut r: ProcRegionWithPathInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcRegionWithPathInfo>() as i32;
    // SAFETY: proc_pidinfo writes at most `size` bytes into r.
    let n = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            PROC_PIDREGIONPATHINFO,
            addr,
            (&mut r as *mut ProcRegionWithPathInfo).cast(),
            size,
        )
    };
    (n >= size).then_some(r)
}

/// The VM map entry containing `addr`, or the next one above it, including
/// the heap reference window's reservation.
pub fn entry_at(addr: u64) -> Option<Entry> {
    let r = entry_info(addr)?;
    Some(Entry {
        start: r.info.address,
        end: r.info.address + r.info.size,
        tag: r.info.user_tag,
    })
}

/// The region containing `addr`, or the next one above it. Reserved pages
/// of the heap reference window are not mapped as far as the guest can
/// tell, so they are skipped.
pub fn region_at(addr: u64) -> Option<Region> {
    let mut at = addr;
    let r = loop {
        let r = entry_info(at)?;
        if r.info.user_tag != super::window::TAG {
            break r;
        }
        at = r.info.address + r.info.size;
    };
    // SAFETY: vip_path is 32x32 c_chars, NUL-terminated by the kernel.
    let raw: &[u8; 1024] = unsafe { &*(r.vip.vip_path.as_ptr() as *const [u8; 1024]) };
    let len = raw.iter().position(|&c| c == 0).unwrap_or(0);
    let file = (len > 0).then(|| {
        let st = &r.vip.vip_vi.vi_stat;
        (
            PathBuf::from(OsStr::from_bytes(&raw[..len])),
            st.vst_dev as u32 as u64,
            st.vst_ino,
        )
    });
    Some(Region {
        start: r.info.address,
        end: r.info.address + r.info.size,
        prot: r.info.protection,
        shared: r.info.inheritance == VM_INHERIT_SHARE,
        offset: r.info.offset,
        file,
    })
}

/// Regions overlapping `[lo, hi)`, clipped to it.
pub fn regions(lo: u64, hi: u64) -> impl Iterator<Item = Region> {
    let mut cur = lo;
    std::iter::from_fn(move || {
        if cur >= hi {
            return None;
        }
        let mut r = region_at(cur)?;
        if r.start >= hi {
            return None;
        }
        if r.start < cur {
            if r.file.is_some() {
                r.offset += cur - r.start;
            }
            r.start = cur;
        }
        r.end = r.end.min(hi);
        cur = r.end;
        Some(r)
    })
}
