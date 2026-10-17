//! DMA-BUF heaps (`drivers/dma-buf/dma-heap.c`, the system heap) and the
//! buffers they export, as libdmabufheap's BufferAllocator and Codec2's
//! linear blocks use them: `/dev/dma_heap/system`'s `DMA_HEAP_IOCTL_ALLOC`
//! returns a dma-buf fd of the page-rounded size.
//!
//! A buffer is an unlinked host file, as an ashmem region is: `MAP_SHARED`
//! mappings, `SEEK_END` for its size, fork, dup and descriptor passing work
//! natively and across processes. The file carries marker flags
//! ([`BUFFER`]) so that a dma-buf fd is recognized wherever it arrives; its
//! name (`DMA_BUF_SET_NAME`) is in an extended attribute. The heap's node
//! is such a file too, of no size, where a buffer is at least a page. Host memory is coherent, so the CPU
//! access brackets of `DMA_BUF_IOCTL_SYNC` have nothing to flush.

use crate::errno::{self, EINVAL};

/// `st_flags` of a dma-buf and of the heap node: neither ashmem's
/// (`UF_NODUMP | UF_OPAQUE`) nor a memfd's (`UF_NODUMP | UF_HIDDEN`).
const MARK: u32 = libc::UF_NODUMP | libc::UF_TRACKED;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Heap,
    Buffer,
}
const NAME_XATTR: &std::ffi::CStr = c"dev.aim.dmabuf.name";
const PAGE: u64 = 16384;
/// The heaps there are: the system heap's node.
const SYSTEM: &str = "/dev/dma_heap/system";
/// `DMA_HEAP_IOCTL_ALLOC`: `_IOWR('H', 0, struct dma_heap_allocation_data)`.
const HEAP_ALLOC: u64 = 0xc018_4800;
/// `DMA_BUF_IOCTL_SYNC`, `DMA_BUF_SET_NAME_A`, `DMA_BUF_SET_NAME_B`.
const BUF_SYNC: u64 = 0x4008_6200;
const BUF_SET_NAME_A: u64 = 0x4004_6201;
const BUF_SET_NAME_B: u64 = 0x4008_6201;
/// `DMA_BUF_SYNC_RW`, `DMA_BUF_SYNC_END`.
const SYNC_RW: u64 = 3;
const SYNC_END: u64 = 4;
/// `DMA_BUF_NAME_LEN`.
const NAME_LEN: usize = 32;
/// `DMA_HEAP_VALID_FD_FLAGS`: `O_CLOEXEC | O_ACCMODE`.
const O_CLOEXEC: u64 = 0o2000000;
const VALID_FD_FLAGS: u64 = O_CLOEXEC | 3;
/// The heap node's device number: a dynamic character major, minor 0.
const RDEV: i32 = 248 << 24;

/// An unlinked marked host file of `size`, published as a guest fd.
fn marked_file(size: u64, cloexec: bool) -> i64 {
    let mut tmpl = std::env::temp_dir().join("linux-abi-dmabuf.XXXXXX").into_os_string().into_encoded_bytes();
    tmpl.push(0);
    // SAFETY: mkstemp fills in the template; the name is removed at once.
    unsafe {
        let fd = libc::mkstemp(tmpl.as_mut_ptr().cast());
        if fd < 0 {
            return -(errno::last() as i64);
        }
        libc::unlink(tmpl.as_ptr().cast());
        if libc::fchflags(fd, MARK) < 0 || libc::ftruncate(fd, size as i64) < 0 {
            let e = errno::last();
            libc::close(fd);
            return -(e as i64);
        }
        super::fdtab::set_flags(fd, false, cloexec);
        if let Err(error) = super::fdtab::publish_typed_guest(fd) {
            libc::close(fd);
            return -(error as i64);
        }
        fd as i64
    }
}

/// `openat` of a heap's node: None for other paths.
pub fn open(guest: &str, flags: u64) -> Option<i64> {
    (guest == SYSTEM).then(|| marked_file(0, flags & O_CLOEXEC != 0))
}

fn kind_of(st: &libc::stat) -> Option<Kind> {
    if st.st_mode & libc::S_IFMT != libc::S_IFREG || st.st_nlink != 0 || st.st_flags & MARK != MARK {
        return None;
    }
    Some(if st.st_size == 0 { Kind::Heap } else { Kind::Buffer })
}

fn stat_of(fd: i32) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st)
}

/// fstat of a heap fd shows its character device, as ueventd makes it
/// (`0444 system system`). False: not one.
pub fn as_device(st: &mut libc::stat) -> bool {
    if kind_of(st) != Some(Kind::Heap) {
        return false;
    }
    st.st_mode = libc::S_IFCHR | 0o444;
    st.st_rdev = RDEV;
    st.st_nlink = 1;
    st.st_size = 0;
    st.st_blocks = 0;
    st.st_uid = 1000;
    st.st_gid = 1000;
    true
}

/// stat of a heap's node, which has no host file. None for other paths.
pub fn stat(guest: &str) -> Option<libc::stat> {
    if guest != SYSTEM {
        return None;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    st.st_mode = libc::S_IFREG;
    st.st_flags = MARK;
    as_device(&mut st);
    Some(st)
}

/// The name of a buffer, as `DMA_BUF_SET_NAME` set it.
fn name(fd: i32) -> String {
    let mut buf = [0u8; NAME_LEN];
    // SAFETY: reading our attribute into a local buffer.
    let n = unsafe { libc::fgetxattr(fd, NAME_XATTR.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
    if n <= 0 { String::new() } else { String::from_utf8_lossy(&buf[..n as usize]).into_owned() }
}

/// Link text for `/proc/self/fd/N`: a buffer's dentry, `dmabuf:<name>`;
/// the heap's node.
pub fn link_name(fd: i32, st: &libc::stat) -> Option<String> {
    match kind_of(st)? {
        Kind::Heap => Some(SYSTEM.to_string()),
        Kind::Buffer => Some(format!("/dmabuf:{}", name(fd))),
    }
}

/// mmap of a buffer: past its end is `EINVAL` (`dma_buf_mmap_internal`).
/// None: not a buffer, or fine.
pub fn before_mmap(fd: i32, len: u64, off: u64) -> Option<i64> {
    let st = stat_of(fd)?;
    match kind_of(&st)? {
        Kind::Buffer if off.div_ceil(PAGE) + len.div_ceil(PAGE) > st.st_size as u64 / PAGE => Some(-(EINVAL as i64)),
        Kind::Buffer => None,
        // The heap's node has no mmap (`dma_heap_fops`).
        Kind::Heap => Some(-(libc::ENODEV as i64)),
    }
}

/// ioctl on a heap or a buffer; None when `fd` is neither.
pub fn ioctl(fd: i32, req: u64, arg: u64) -> Option<i64> {
    let kind = (req >> 8) & 0xff;
    if kind != b'H' as u64 && kind != b'b' as u64 {
        return None;
    }
    let st = stat_of(fd)?;
    Some(match (kind_of(&st)?, req) {
        (Kind::Heap, HEAP_ALLOC) => {
            // SAFETY: guest struct dma_heap_allocation_data { u64 len; u32
            // fd; u32 fd_flags; u64 heap_flags; }.
            let data = arg as *mut u8;
            let (len, fd_flags, heap_flags) = unsafe {
                (
                    (data as *const u64).read_unaligned(),
                    (data.add(12) as *const u32).read_unaligned() as u64,
                    (data.add(16) as *const u64).read_unaligned(),
                )
            };
            if fd_flags & !VALID_FD_FLAGS != 0 || heap_flags != 0 {
                return Some(-(EINVAL as i64));
            }
            let size = len.div_ceil(PAGE) * PAGE;
            if size == 0 {
                return Some(-(EINVAL as i64));
            }
            let buffer = marked_file(size, fd_flags & O_CLOEXEC != 0);
            if buffer >= 0 {
                // SAFETY: the guest struct's fd field.
                unsafe { (data.add(8) as *mut u32).write_unaligned(buffer as u32) };
            }
            if buffer < 0 { buffer } else { 0 }
        }
        (Kind::Buffer, BUF_SYNC) => {
            // SAFETY: guest struct dma_buf_sync { u64 flags; }.
            let flags = unsafe { (arg as *const u64).read_unaligned() };
            if flags & !(SYNC_RW | SYNC_END) != 0 || flags & SYNC_RW == 0 {
                -(EINVAL as i64)
            } else {
                0
            }
        }
        (Kind::Buffer, BUF_SET_NAME_A | BUF_SET_NAME_B) => {
            // `strndup_user(buf, DMA_BUF_NAME_LEN)`: a string that does not
            // end within the length is EINVAL.
            // SAFETY: guest string of up to DMA_BUF_NAME_LEN bytes.
            let bytes = unsafe { std::slice::from_raw_parts(arg as *const u8, NAME_LEN) };
            let Some(end) = bytes.iter().position(|&c| c == 0) else {
                return Some(-(EINVAL as i64));
            };
            // SAFETY: writing our attribute from the guest's bytes.
            let r = unsafe { libc::fsetxattr(fd, NAME_XATTR.as_ptr(), bytes.as_ptr().cast(), end, 0, 0) };
            if r < 0 { -(errno::last() as i64) } else { 0 }
        }
        _ => -(libc::ENOTTY as i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alloc(heap: i32, len: u64, fd_flags: u32) -> (i64, i32) {
        let mut data = [0u8; 24];
        data[..8].copy_from_slice(&len.to_le_bytes());
        data[12..16].copy_from_slice(&fd_flags.to_le_bytes());
        let r = ioctl(heap, HEAP_ALLOC, data.as_mut_ptr() as u64).unwrap();
        (r, u32::from_le_bytes(data[8..12].try_into().unwrap()) as i32)
    }

    #[test]
    fn the_system_heap_exports_page_rounded_buffers() {
        assert!(open("/dev/dma_heap/other", 0).is_none());
        let heap = open(SYSTEM, O_CLOEXEC).unwrap() as i32;
        let mut st = stat_of(heap).unwrap();
        assert!(as_device(&mut st));
        assert_eq!((st.st_mode, st.st_rdev), (libc::S_IFCHR | 0o444, RDEV));
        assert_eq!(stat(SYSTEM).map(|s| s.st_rdev), Some(RDEV));
        // Bad flags and an empty buffer are refused.
        assert_eq!(alloc(heap, 4096, 0o100).0, -(EINVAL as i64));
        assert_eq!(alloc(heap, 0, 2).0, -(EINVAL as i64));
        let (r, buf) = alloc(heap, 5000, (O_CLOEXEC | 2) as u32);
        assert_eq!(r, 0);
        let st = stat_of(buf).unwrap();
        assert_eq!((kind_of(&st), st.st_size as u64), (Some(Kind::Buffer), PAGE));
        // The heap has no mmap; a buffer maps up to its end.
        assert_eq!(before_mmap(heap, PAGE, 0), Some(-(libc::ENODEV as i64)));
        assert_eq!(before_mmap(buf, PAGE, 0), None);
        assert_eq!(before_mmap(buf, 2 * PAGE, 0), Some(-(EINVAL as i64)));
        // CPU access brackets: read, write or both, start or end.
        assert_eq!(ioctl(buf, BUF_SYNC, [SYNC_RW].as_ptr() as u64), Some(0));
        assert_eq!(ioctl(buf, BUF_SYNC, [SYNC_END | 1].as_ptr() as u64), Some(0));
        assert_eq!(ioctl(buf, BUF_SYNC, [0u64].as_ptr() as u64), Some(-(EINVAL as i64)));
        assert_eq!(ioctl(buf, BUF_SYNC, [8u64].as_ptr() as u64), Some(-(EINVAL as i64)));
        // A name of the buffer's, in its link.
        let mut name = [0u8; NAME_LEN];
        name[..5].copy_from_slice(b"c2buf");
        assert_eq!(ioctl(buf, BUF_SET_NAME_B, name.as_ptr() as u64), Some(0));
        assert_eq!(link_name(buf, &st).as_deref(), Some("/dmabuf:c2buf"));
        assert_eq!(ioctl(buf, BUF_SET_NAME_B, [b'x'; NAME_LEN].as_ptr() as u64), Some(-(EINVAL as i64)));
        // Another fd's ioctl of these kinds is not ours.
        assert_eq!(ioctl(0, BUF_SYNC, 0), None);
        // SAFETY: our fds.
        unsafe {
            libc::close(buf);
            libc::close(heap);
        }
    }
}
