//! `android.hardware.graphics.allocator-service.aim`: the
//! `IAllocator/default` V2 vendor HAL of the derived image
//! (`docs/graphics-buffers.md`).
//!
//! Each buffer is one memfd: the pixel planes from offset 0, then the
//! shared metadata area. The handle's layout is `aim_gralloc::Handle`;
//! `mapper.aim.so` maps it in every process that imports the buffer.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};

use aim_gralloc::metadata::SharedMetadata;
use aim_gralloc::{Descriptor, Handle, Unsupported};
use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_graphics_allocator::aidl::android::hardware::graphics::allocator::{
    AllocationError::AllocationError, AllocationResult::AllocationResult,
    BufferDescriptorInfo::BufferDescriptorInfo, IAllocator::BnAllocator, IAllocator::IAllocator,
};
use binder::{BinderFeatures, ParcelFileDescriptor, Status};

const INSTANCE: &str = "android.hardware.graphics.allocator.IAllocator/default";
/// `mapper.<suffix>.so` in `/vendor/lib64/hw`.
const MAPPER_SUFFIX: &str = "aim";

fn error(e: AllocationError) -> Status {
    Status::new_service_specific_error(e.0, None)
}

fn descriptor(info: &BufferDescriptorInfo) -> Result<Descriptor, AllocationError> {
    if !info.additionalOptions.is_empty() {
        return Err(AllocationError::UNSUPPORTED);
    }
    Ok(Descriptor {
        width: info.width,
        height: info.height,
        layer_count: info.layerCount,
        format: info.format.0,
        usage: info.usage.0 as u64,
        reserved_size: info.reservedSize,
    })
}

fn map_error(e: Unsupported) -> AllocationError {
    match e {
        Unsupported::BadDescriptor => AllocationError::BAD_DESCRIPTOR,
        Unsupported::Unsupported => AllocationError::UNSUPPORTED,
    }
}

struct Allocator {
    next_id: AtomicU32,
}

impl Allocator {
    /// A new buffer id, unique across processes and allocator restarts.
    fn id(&self) -> u64 {
        // SAFETY: getpid has no preconditions.
        let pid = unsafe { libc::getpid() } as u64;
        pid << 32 | self.next_id.fetch_add(1, Ordering::Relaxed) as u64
    }

    fn allocate_one(
        &self,
        info: &BufferDescriptorInfo,
    ) -> Result<(i32, NativeHandle), AllocationError> {
        let desc = descriptor(info)?;
        let layout = desc.layout().map_err(map_error)?;
        if desc.reserved_size as u64 > aim_gralloc::metadata::MAX_RESERVED_SIZE {
            return Err(AllocationError::UNSUPPORTED);
        }
        let h = Handle::new(&desc, &layout, self.id());
        let fd = memfd(&info.name, &h).ok_or(AllocationError::NO_RESOURCES)?;
        let native = NativeHandle {
            fds: vec![ParcelFileDescriptor::new(fd)],
            ints: h.to_ints().to_vec(),
        };
        Ok((h.stride as i32, native))
    }
}

/// A zero-filled memfd of the buffer's size with its metadata initialized.
fn memfd(name: &[u8; 128], h: &Handle) -> Option<OwnedFd> {
    let label = CString::new(format!("gralloc-{:x}", h.id)).unwrap();
    // SAFETY: plain syscalls on a fd we own; the mapping is ours.
    unsafe {
        let fd = libc::memfd_create(label.as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return None;
        }
        let fd = OwnedFd::from_raw_fd(fd);
        if libc::ftruncate(fd.as_raw_fd(), h.total_size() as i64) != 0 {
            return None;
        }
        let len = h.metadata_size() as usize;
        let m = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            h.metadata_offset as i64,
        );
        if m == libc::MAP_FAILED {
            return None;
        }
        (*m.cast::<SharedMetadata>()).init(name);
        libc::munmap(m, len);
        Some(fd)
    }
}

impl binder::Interface for Allocator {}

impl IAllocator for Allocator {
    fn allocate(&self, _descriptor: &[u8], _count: i32) -> binder::Result<AllocationResult> {
        // The IMapper 4 descriptor path; mapper 5 clients use allocate2.
        Err(error(AllocationError::UNSUPPORTED))
    }

    fn allocate2(
        &self,
        info: &BufferDescriptorInfo,
        count: i32,
    ) -> binder::Result<AllocationResult> {
        if count <= 0 {
            return Err(error(AllocationError::BAD_DESCRIPTOR));
        }
        let mut result = AllocationResult {
            stride: 0,
            buffers: Vec::with_capacity(count as usize),
        };
        for _ in 0..count {
            let (stride, buffer) = self.allocate_one(info).map_err(error)?;
            result.stride = stride;
            result.buffers.push(buffer);
        }
        Ok(result)
    }

    fn isSupported(&self, info: &BufferDescriptorInfo) -> binder::Result<bool> {
        Ok(descriptor(info).is_ok_and(|d| d.layout().is_ok()))
    }

    fn getIMapperLibrarySuffix(&self) -> binder::Result<String> {
        Ok(MAPPER_SUFFIX.into())
    }
}

fn main() {
    binder::ProcessState::set_thread_pool_max_thread_count(2);
    binder::ProcessState::start_thread_pool();
    let allocator = Allocator {
        next_id: AtomicU32::new(1),
    };
    let binder = BnAllocator::new_binder(allocator, BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, binder.as_binder()) {
        eprintln!("allocator: cannot register {INSTANCE}: {e:?}");
        std::process::exit(1);
    }
    binder::ProcessState::join_thread_pool();
}
