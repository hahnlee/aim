//! `mapper.darwin.so`: the stable-C `AIMapper` v5 of the derived image
//! (`docs/graphics-buffers.md`). libui's Gralloc5 loads it in every process
//! that touches graphics buffers.
//!
//! A buffer is one memfd. Importing maps all of it once; `lock` returns
//! that mapping, and the metadata area is read and written in place, so
//! every process sees the same mutable metadata.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::{Mutex, OnceLock};

use darwin_gralloc::Handle;
use darwin_gralloc::handle::{
    self, NATIVE_HANDLE_VERSION, NUM_FDS, NUM_INTS, NativeHandle, RESERVED_OFFSET,
};
use darwin_gralloc::metadata::{self, STANDARD_NAME, SUPPORTED, SharedMetadata};

/// `AIMapper_Error`.
type Error = i32;
const NONE: Error = 0;
const BAD_BUFFER: Error = 2;
const BAD_VALUE: Error = 3;
const NO_RESOURCES: Error = 5;
const UNSUPPORTED: Error = 7;

const AIMAPPER_VERSION_5: u32 = 5;

/// The exported version (the stable-C README and the VTS name both).
#[unsafe(no_mangle)]
pub static ANDROID_HAL_STABLEC_VERSION: u32 = AIMAPPER_VERSION_5;
#[unsafe(no_mangle)]
pub static ANDROID_HAL_MAPPER_VERSION: u32 = AIMAPPER_VERSION_5;

type BufferHandle = *const NativeHandle;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MetadataType {
    name: *const c_char,
    value: i64,
}

#[repr(C)]
pub struct MetadataTypeDescription {
    metadata_type: MetadataType,
    description: *const c_char,
    is_gettable: bool,
    is_settable: bool,
    reserved: [u8; 32],
}

/// `ARect`.
#[repr(C)]
pub struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

type DumpCallback = unsafe extern "C" fn(*mut c_void, MetadataType, *const c_void, usize);
type BeginDumpCallback = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
pub struct MapperV5 {
    import_buffer: unsafe extern "C" fn(BufferHandle, *mut BufferHandle) -> Error,
    free_buffer: unsafe extern "C" fn(BufferHandle) -> Error,
    get_transport_size: unsafe extern "C" fn(BufferHandle, *mut u32, *mut u32) -> Error,
    lock: unsafe extern "C" fn(BufferHandle, u64, Rect, c_int, *mut *mut c_void) -> Error,
    unlock: unsafe extern "C" fn(BufferHandle, *mut c_int) -> Error,
    flush_locked_buffer: unsafe extern "C" fn(BufferHandle) -> Error,
    reread_locked_buffer: unsafe extern "C" fn(BufferHandle) -> Error,
    get_metadata: unsafe extern "C" fn(BufferHandle, MetadataType, *mut c_void, usize) -> i32,
    get_standard_metadata: unsafe extern "C" fn(BufferHandle, i64, *mut c_void, usize) -> i32,
    set_metadata: unsafe extern "C" fn(BufferHandle, MetadataType, *const c_void, usize) -> Error,
    set_standard_metadata: unsafe extern "C" fn(BufferHandle, i64, *const c_void, usize) -> Error,
    list_supported_metadata_types:
        unsafe extern "C" fn(*mut *const MetadataTypeDescription, *mut usize) -> Error,
    dump_buffer: unsafe extern "C" fn(BufferHandle, DumpCallback, *mut c_void) -> Error,
    dump_all_buffers: unsafe extern "C" fn(BeginDumpCallback, DumpCallback, *mut c_void) -> Error,
    get_reserved_region: unsafe extern "C" fn(BufferHandle, *mut *mut c_void, *mut u64) -> Error,
}

/// `AIMapper`: `version` is `alignas(max_align_t)`.
#[repr(C, align(16))]
pub struct Mapper {
    version: u32,
    v5: MapperV5,
}

static MAPPER: Mapper = Mapper {
    version: AIMAPPER_VERSION_5,
    v5: MapperV5 {
        import_buffer,
        free_buffer,
        get_transport_size,
        lock,
        unlock,
        flush_locked_buffer: validate,
        reread_locked_buffer: validate,
        get_metadata,
        get_standard_metadata,
        set_metadata,
        set_standard_metadata,
        list_supported_metadata_types,
        dump_buffer,
        dump_all_buffers,
        get_reserved_region,
    },
};

/// # Safety
/// `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AIMapper_loadIMapper(out: *mut *const Mapper) -> Error {
    // SAFETY: caller contract.
    unsafe { *out = &MAPPER };
    NONE
}

/// An imported buffer: our own handle (the key) and the mapping.
struct Buffer {
    handle: Handle,
    /// The imported handle's storage: header, fd and ints.
    storage: Box<[i32; 3 + NUM_FDS + NUM_INTS]>,
    base: *mut u8,
    len: usize,
}

// SAFETY: the mapping is process-wide shared memory.
unsafe impl Send for Buffer {}

impl Buffer {
    fn fd(&self) -> c_int {
        self.storage[3]
    }

    fn metadata(&self) -> *mut SharedMetadata {
        // SAFETY: the mapping covers the metadata area (checked at import).
        unsafe { self.base.add(self.handle.metadata_offset as usize) }.cast()
    }
}

fn buffers() -> std::sync::MutexGuard<'static, HashMap<usize, Buffer>> {
    static BUFFERS: OnceLock<Mutex<HashMap<usize, Buffer>>> = OnceLock::new();
    BUFFERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Run `f` on an imported buffer, or return `BAD_BUFFER`.
fn with<R>(h: BufferHandle, bad: R, f: impl FnOnce(&Buffer) -> R) -> R {
    match buffers().get(&(h as usize)) {
        Some(b) => f(b),
        None => bad,
    }
}

unsafe extern "C" fn import_buffer(raw: BufferHandle, out: *mut BufferHandle) -> Error {
    // SAFETY: libui passes a native_handle_t it received.
    let Some((fd, handle)) = (unsafe { handle::parse(raw) }) else {
        return BAD_BUFFER;
    };
    // SAFETY: plain fd calls on our own fds.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 || (st.st_size as u64) < handle.total_size() {
            return BAD_BUFFER;
        }
        let dup = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0);
        if dup < 0 {
            return NO_RESOURCES;
        }
        let len = handle.total_size() as usize;
        let base = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            dup,
            0,
        );
        if base == libc::MAP_FAILED {
            libc::close(dup);
            return NO_RESOURCES;
        }
        let mut storage = Box::new([0i32; 3 + NUM_FDS + NUM_INTS]);
        storage[..4].copy_from_slice(&[NATIVE_HANDLE_VERSION, 1, NUM_INTS as i32, dup]);
        storage[4..].copy_from_slice(&handle.to_ints());
        let buffer = Buffer {
            handle,
            storage,
            base: base.cast(),
            len,
        };
        if !(*buffer.metadata()).is_valid() {
            libc::munmap(base, len);
            libc::close(dup);
            return BAD_BUFFER;
        }
        let key = buffer.storage.as_ptr() as BufferHandle;
        buffers().insert(key as usize, buffer);
        *out = key;
    }
    NONE
}

unsafe extern "C" fn free_buffer(h: BufferHandle) -> Error {
    let Some(b) = buffers().remove(&(h as usize)) else {
        return BAD_BUFFER;
    };
    // SAFETY: our mapping and fd.
    unsafe {
        libc::munmap(b.base.cast(), b.len);
        libc::close(b.fd());
    }
    NONE
}

unsafe extern "C" fn get_transport_size(h: BufferHandle, fds: *mut u32, ints: *mut u32) -> Error {
    with(h, BAD_BUFFER, |_| {
        // SAFETY: out pointers from the caller.
        unsafe {
            *fds = NUM_FDS as u32;
            *ints = NUM_INTS as u32;
        }
        NONE
    })
}

/// Wait for and close a fence fd (a sync_file, or anything pollable).
fn wait_fence(fence: c_int) {
    if fence < 0 {
        return;
    }
    let mut p = libc::pollfd {
        fd: fence,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one pollfd; the fence is ours to close.
    unsafe {
        while libc::poll(&mut p, 1, -1) < 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
        {}
        libc::close(fence);
    }
}

unsafe extern "C" fn lock(
    h: BufferHandle,
    _cpu_usage: u64,
    _region: Rect,
    acquire_fence: c_int,
    out: *mut *mut c_void,
) -> Error {
    let Some(base) = with(h, None, |b| Some(b.base)) else {
        // The fence is ours to close even on error.
        wait_fence(acquire_fence);
        return BAD_BUFFER;
    };
    wait_fence(acquire_fence);
    // SAFETY: out pointer from the caller.
    unsafe { *out = base.cast() };
    NONE
}

unsafe extern "C" fn unlock(h: BufferHandle, release_fence: *mut c_int) -> Error {
    with(h, BAD_BUFFER, |_| {
        // SAFETY: out pointer from the caller.
        unsafe { *release_fence = -1 };
        NONE
    })
}

unsafe extern "C" fn validate(h: BufferHandle) -> Error {
    with(h, BAD_BUFFER, |_| NONE)
}

fn is_standard(t: &MetadataType) -> bool {
    // SAFETY: metadata type names are NUL-terminated strings.
    !t.name.is_null() && unsafe { CStr::from_ptr(t.name) }.to_bytes() == STANDARD_NAME.as_bytes()
}

unsafe extern "C" fn get_metadata(
    h: BufferHandle,
    t: MetadataType,
    dest: *mut c_void,
    size: usize,
) -> i32 {
    if !is_standard(&t) {
        return -UNSUPPORTED;
    }
    // SAFETY: forwarded caller contract.
    unsafe { get_standard_metadata(h, t.value, dest, size) }
}

unsafe extern "C" fn get_standard_metadata(
    h: BufferHandle,
    t: i64,
    dest: *mut c_void,
    size: usize,
) -> i32 {
    with(h, -BAD_BUFFER, |b| {
        let dest: &mut [u8] = if dest.is_null() {
            &mut []
        } else {
            // SAFETY: the caller's buffer of `size` bytes.
            unsafe { std::slice::from_raw_parts_mut(dest.cast(), size) }
        };
        // SAFETY: the shared metadata of a mapped buffer.
        metadata::get_standard(&b.handle, unsafe { &*b.metadata() }, t, dest)
    })
}

unsafe extern "C" fn set_metadata(
    h: BufferHandle,
    t: MetadataType,
    src: *const c_void,
    size: usize,
) -> Error {
    if !is_standard(&t) {
        return UNSUPPORTED;
    }
    // SAFETY: forwarded caller contract.
    unsafe { set_standard_metadata(h, t.value, src, size) }
}

unsafe extern "C" fn set_standard_metadata(
    h: BufferHandle,
    t: i64,
    src: *const c_void,
    size: usize,
) -> Error {
    if src.is_null() && size > 0 {
        return BAD_VALUE;
    }
    with(h, BAD_BUFFER, |b| {
        let src: &[u8] = if size == 0 {
            &[]
        } else {
            // SAFETY: the caller's value of `size` bytes.
            unsafe { std::slice::from_raw_parts(src.cast(), size) }
        };
        // SAFETY: the shared metadata of a mapped buffer.
        metadata::set_standard(unsafe { &mut *b.metadata() }, t, src)
    })
}

struct Descriptions(Vec<MetadataTypeDescription>);
// SAFETY: immutable after creation; the pointers are to static strings.
unsafe impl Send for Descriptions {}
unsafe impl Sync for Descriptions {}

const STANDARD_NAME_C: &CStr = c"android.hardware.graphics.common.StandardMetadataType";

fn standard_type(value: i64) -> MetadataType {
    MetadataType {
        name: STANDARD_NAME_C.as_ptr(),
        value,
    }
}

unsafe extern "C" fn list_supported_metadata_types(
    out: *mut *const MetadataTypeDescription,
    count: *mut usize,
) -> Error {
    static LIST: OnceLock<Descriptions> = OnceLock::new();
    let list = LIST.get_or_init(|| {
        Descriptions(
            SUPPORTED
                .iter()
                .map(|&(value, settable)| MetadataTypeDescription {
                    metadata_type: standard_type(value),
                    description: std::ptr::null(),
                    is_gettable: true,
                    is_settable: settable,
                    reserved: [0; 32],
                })
                .collect(),
        )
    });
    // SAFETY: out pointers from the caller.
    unsafe {
        *out = list.0.as_ptr();
        *count = list.0.len();
    }
    NONE
}

/// Every gettable standard type of `b`, encoded.
fn dump(b: &Buffer) -> Vec<(i64, Vec<u8>)> {
    // SAFETY: the shared metadata of a mapped buffer.
    let m = unsafe { &*b.metadata() };
    SUPPORTED
        .iter()
        .filter_map(|&(t, _)| {
            let n = metadata::get_standard(&b.handle, m, t, &mut []);
            (n > 0).then(|| {
                let mut v = vec![0; n as usize];
                metadata::get_standard(&b.handle, m, t, &mut v);
                (t, v)
            })
        })
        .collect()
}

unsafe fn report(values: Vec<(i64, Vec<u8>)>, cb: DumpCallback, ctx: *mut c_void) {
    for (t, v) in values {
        // SAFETY: the caller's callback with its context.
        unsafe { cb(ctx, standard_type(t), v.as_ptr().cast(), v.len()) };
    }
}

unsafe extern "C" fn dump_buffer(h: BufferHandle, cb: DumpCallback, ctx: *mut c_void) -> Error {
    // Callbacks run without the registry lock.
    let Some(values) = with(h, None, |b| Some(dump(b))) else {
        return BAD_BUFFER;
    };
    // SAFETY: forwarded caller contract.
    unsafe { report(values, cb, ctx) };
    NONE
}

unsafe extern "C" fn dump_all_buffers(
    begin: BeginDumpCallback,
    cb: DumpCallback,
    ctx: *mut c_void,
) -> Error {
    let all: Vec<_> = buffers().values().map(dump).collect();
    for values in all {
        // SAFETY: the caller's callbacks with their context.
        unsafe {
            begin(ctx);
            report(values, cb, ctx);
        }
    }
    NONE
}

unsafe extern "C" fn get_reserved_region(
    h: BufferHandle,
    out: *mut *mut c_void,
    size: *mut u64,
) -> Error {
    with(h, BAD_BUFFER, |b| {
        let reserved = b.handle.reserved_size;
        let region = if reserved == 0 {
            std::ptr::null_mut()
        } else {
            // SAFETY: inside the mapped metadata area.
            unsafe { b.metadata().cast::<u8>().add(RESERVED_OFFSET as usize) }
        };
        // SAFETY: out pointers from the caller.
        unsafe {
            *out = region.cast();
            *size = reserved;
        }
        NONE
    })
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    use darwin_gralloc::metadata::standard;
    use darwin_gralloc::{Descriptor, PAGE_SIZE, format};

    use super::*;

    /// A buffer as the allocator makes it, in an unlinked temporary file.
    fn allocate(desc: &Descriptor) -> (std::fs::File, Handle) {
        let h = Handle::new(desc, &desc.layout().unwrap(), 42);
        let path = std::env::temp_dir().join(format!("mapper-test-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        file.set_len(h.total_size()).unwrap();
        // SAFETY: a fresh file of the buffer's size; the mapping is ours.
        unsafe {
            let len = h.metadata_size() as usize;
            let m = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                h.metadata_offset as i64,
            );
            assert_ne!(m, libc::MAP_FAILED);
            (*m.cast::<SharedMetadata>()).init(b"test buffer");
            libc::munmap(m, len);
        }
        (file, h)
    }

    /// Get a standard metadata value the way libui does: size, then value.
    unsafe fn get(m: &MapperV5, buffer: BufferHandle, t: i64) -> Vec<u8> {
        // SAFETY: caller contract.
        unsafe {
            let n = (m.get_standard_metadata)(buffer, t, std::ptr::null_mut(), 0);
            assert!(n >= 0, "type {t}: {n}");
            let mut v = vec![0u8; n as usize];
            assert_eq!(
                (m.get_standard_metadata)(buffer, t, v.as_mut_ptr().cast(), v.len()),
                n
            );
            v
        }
    }

    #[test]
    fn import_lock_metadata_free() {
        let desc = Descriptor {
            width: 32,
            height: 8,
            layer_count: 1,
            format: format::RGBA_8888,
            usage: 0x333,
            reserved_size: 16,
        };
        let (file, h) = allocate(&desc);
        let mut raw = vec![NATIVE_HANDLE_VERSION, 1, NUM_INTS as i32, file.as_raw_fd()];
        raw.extend_from_slice(&h.to_ints());
        let mut mapper: *const Mapper = std::ptr::null();
        let region = || Rect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 8,
        };
        // SAFETY: the mapper's C entry points with valid arguments.
        unsafe {
            assert_eq!(AIMapper_loadIMapper(&mut mapper), NONE);
            let m = &(*mapper).v5;
            let mut buffer: BufferHandle = std::ptr::null();
            assert_eq!((m.import_buffer)(raw.as_ptr().cast(), &mut buffer), NONE);
            let (mut fds, mut ints) = (0, 0);
            assert_eq!((m.get_transport_size)(buffer, &mut fds, &mut ints), NONE);
            assert_eq!((fds, ints), (1, NUM_INTS as u32));

            // CPU writes through one lock are seen by the next.
            let mut data: *mut c_void = std::ptr::null_mut();
            assert_eq!((m.lock)(buffer, 0x30, region(), -1, &mut data), NONE);
            *data.cast::<u32>() = 0xdead_beef;
            let mut fence = 0;
            assert_eq!((m.unlock)(buffer, &mut fence), NONE);
            assert_eq!(fence, -1);
            assert_eq!((m.lock)(buffer, 0x3, region(), -1, &mut data), NONE);
            assert_eq!(*data.cast::<u32>(), 0xdead_beef);
            (m.unlock)(buffer, &mut fence);

            let stride = get(m, buffer, standard::STRIDE);
            assert_eq!(
                &stride[stride.len() - 4..],
                &(h.stride as i32).to_le_bytes()
            );
            let mut dataspace = get(m, buffer, standard::DATASPACE);
            let len = dataspace.len();
            dataspace[len - 4..].copy_from_slice(&0x0891_0000i32.to_le_bytes());
            assert_eq!(
                (m.set_standard_metadata)(
                    buffer,
                    standard::DATASPACE,
                    dataspace.as_ptr().cast(),
                    len
                ),
                NONE
            );
            assert_eq!(get(m, buffer, standard::DATASPACE), dataspace);
            assert_eq!(
                (m.get_standard_metadata)(buffer, 999, std::ptr::null_mut(), 0),
                -UNSUPPORTED
            );

            let mut list: *const MetadataTypeDescription = std::ptr::null();
            let mut count = 0;
            assert_eq!(
                (m.list_supported_metadata_types)(&mut list, &mut count),
                NONE
            );
            assert_eq!(count, SUPPORTED.len());

            let mut reserved: *mut c_void = std::ptr::null_mut();
            let mut size = 0;
            assert_eq!(
                (m.get_reserved_region)(buffer, &mut reserved, &mut size),
                NONE
            );
            assert_eq!(size, 16);
            assert_eq!(
                reserved as usize % PAGE_SIZE as usize,
                RESERVED_OFFSET as usize
            );

            assert_eq!((m.free_buffer)(buffer), NONE);
            assert_eq!((m.free_buffer)(buffer), BAD_BUFFER);
            let junk = [NATIVE_HANDLE_VERSION, 0, 0, 0];
            assert_eq!(
                (m.import_buffer)(junk.as_ptr().cast(), &mut buffer),
                BAD_BUFFER
            );
        }
    }
}
