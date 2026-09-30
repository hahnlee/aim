//! Walking this process's VM map, for `/proc/self/maps`, madvise and memfd
//! views.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

const PROC_PIDREGIONINFO: i32 = 7;
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

/// A VM map entry as a fork snapshot needs it.
pub struct Info {
    pub start: u64,
    pub end: u64,
    /// Darwin VM_PROT bits, current and maximum.
    pub prot: u32,
    pub max_prot: u32,
    /// Inherited as shared by fork children (MAP_SHARED and the like).
    pub shared: bool,
    /// No memory behind it yet (`SM_EMPTY`): never touched.
    pub empty: bool,
    pub tag: u32,
}

const SM_EMPTY: u32 = 3;

/// The entry containing `addr`, or the next one above it (no path lookup).
pub fn info_at(addr: u64) -> Option<Info> {
    let mut r: ProcRegionInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcRegionInfo>() as i32;
    // SAFETY: proc_pidinfo writes at most `size` bytes into r.
    let n = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            PROC_PIDREGIONINFO,
            addr,
            (&mut r as *mut ProcRegionInfo).cast(),
            size,
        )
    };
    (n >= size).then(|| Info {
        start: r.address,
        end: r.address + r.size,
        prot: r.protection,
        max_prot: r.max_protection,
        shared: r.inheritance == VM_INHERIT_SHARE,
        empty: r.share_mode == SM_EMPTY && r.pages_resident == 0 && r.pages_swapped_out == 0,
        tag: r.user_tag,
    })
}

/// Whether all of `[addr, addr + len)` is mapped with every bit of `prot`
/// (Darwin VM_PROT), as `copy_to_user` and `copy_from_user` would find it.
/// A call that would otherwise fault in the layer answers EFAULT instead.
pub fn accessible(addr: u64, len: u64, prot: u32) -> bool {
    let Some(end) = addr.checked_add(len) else {
        return false;
    };
    let mut at = addr;
    while at < end {
        match info_at(at) {
            Some(r) if r.start <= at && r.prot & prot == prot => at = r.end,
            _ => return false,
        }
    }
    true
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

/// The shared regions overlapping `[lo, hi)`, clipped to it: the entries
/// are walked without the path lookup (several microseconds each), which
/// only the shared ones get.
pub fn shared_regions(lo: u64, hi: u64) -> Vec<Region> {
    let mut out = Vec::new();
    let mut cur = lo;
    while cur < hi {
        let Some(i) = info_at(cur) else {
            break;
        };
        if i.start >= hi {
            break;
        }
        if i.shared && i.tag != super::window::TAG {
            out.extend(regions(cur.max(i.start), i.end.min(hi)));
        }
        cur = i.end;
    }
    out
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
