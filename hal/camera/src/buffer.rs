//! Stream buffers: gralloc buffers of the derived image
//! (`docs/graphics-buffers.md`), mapped once per buffer id, and described
//! to the host module as its outputs.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd};

use aim_gralloc::{Handle, format};
use aim_hostcall::camera::{self, Output};
use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;

/// `sizeof(CameraBlob)`: the JPEG trailer at the end of a BLOB buffer.
pub const CAMERA_BLOB_SIZE: usize = 8;
/// `CameraBlobId::JPEG`.
const BLOB_ID_JPEG: i32 = 0xff;

/// A mapped buffer.
pub struct Buffer {
    pub handle: Handle,
    base: *mut u8,
    len: usize,
}

// SAFETY: the mapping is shared memory owned by this struct; only the
// capture thread writes it, one request at a time.
unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

impl Buffer {
    /// Map the buffer of a gralloc handle.
    pub fn import(native: &NativeHandle) -> Option<Buffer> {
        let (fd, handle) = (native.fds.first()?, Handle::from_ints(&native.ints)?);
        Buffer::map(fd.as_ref().as_fd(), handle)
    }

    fn map(fd: BorrowedFd, handle: Handle) -> Option<Buffer> {
        let len = handle.total_size() as usize;
        // SAFETY: a shared mapping of the whole memfd, owned by the result.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        (base != libc::MAP_FAILED).then(|| Buffer {
            handle,
            base: base.cast(),
            len,
        })
    }

    /// The host output for this buffer, a stream of `width` × `height`
    /// (the JPEG size for a BLOB buffer, whose handle width is its byte
    /// size).
    pub fn output(
        &self,
        width: u32,
        height: u32,
        jpeg_quality: u8,
        jpeg_rotation: i32,
    ) -> Option<Output> {
        output(
            &self.handle,
            self.base as u64,
            width,
            height,
            jpeg_quality,
            jpeg_rotation,
        )
    }

    /// Write the `CameraBlob` trailer of a JPEG of `size` bytes.
    pub fn finish_jpeg(&self, size: usize) {
        let end = self.handle.width as usize;
        if end < CAMERA_BLOB_SIZE || end > self.len {
            return;
        }
        let mut blob = [0u8; CAMERA_BLOB_SIZE];
        blob[..4].copy_from_slice(&BLOB_ID_JPEG.to_le_bytes());
        blob[4..].copy_from_slice(&(size as i32).to_le_bytes());
        // SAFETY: inside our mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(
                blob.as_ptr(),
                self.base.add(end - CAMERA_BLOB_SIZE),
                CAMERA_BLOB_SIZE,
            )
        };
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: our mapping.
        unsafe { libc::munmap(self.base.cast(), self.len) };
    }
}

/// The host output of a buffer mapped at `address`, or `None` for a format
/// the camera does not write or a buffer too small for the stream.
pub fn output(
    h: &Handle,
    address: u64,
    width: u32,
    height: u32,
    jpeg_quality: u8,
    jpeg_rotation: i32,
) -> Option<Output> {
    let layout = h.layout()?;
    let mut o = Output {
        format: h.format,
        width,
        height,
        jpeg_quality: jpeg_quality as u32,
        jpeg_orientation: jpeg_rotation.rem_euclid(360) as u32,
        address,
        length: h.data_size,
        ..Output::default()
    };
    match h.format {
        format::RGBA_8888 | format::RGBX_8888 | format::YCBCR_420_888 | format::YCRCB_420_SP
            if (h.width, h.height) != (width, height) =>
        {
            return None;
        }
        format::RGBA_8888 | format::RGBX_8888 => o.stride = layout.stride_bytes() as u32,
        format::YCBCR_420_888 | format::YCRCB_420_SP => {
            o.stride = layout.planes[0].stride_bytes as u32;
            o.chroma_offset = layout.planes[1].offset;
            o.chroma_stride = layout.planes[1].stride_bytes as u32;
        }
        format::BLOB => {
            o.format = camera::format::BLOB;
            o.length = (h.width as u64).checked_sub(CAMERA_BLOB_SIZE as u64)?;
        }
        _ => return None,
    }
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_gralloc::Descriptor;

    fn handle(format: i32, width: i32, height: i32) -> Handle {
        let d = Descriptor {
            width,
            height,
            layer_count: 1,
            format,
            usage: 1 << 17,
            reserved_size: 0,
        };
        Handle::new(&d, &d.layout().unwrap(), 1)
    }

    #[test]
    fn outputs_follow_the_gralloc_layout() {
        let o = output(
            &handle(format::YCBCR_420_888, 640, 480),
            0x1000,
            640,
            480,
            90,
            0,
        )
        .unwrap();
        assert_eq!(
            (o.stride, o.chroma_offset, o.chroma_stride),
            (640, 640 * 480, 640)
        );
        assert_eq!(o.length, 640 * 480 * 3 / 2);
        let o = output(
            &handle(format::YCBCR_420_888, 100, 50),
            0x1000,
            100,
            50,
            90,
            0,
        )
        .unwrap();
        assert_eq!(
            (o.stride, o.chroma_offset, o.chroma_stride),
            (112, 112 * 50, 112)
        );
        let o = output(&handle(format::RGBX_8888, 100, 50), 0x1000, 100, 50, 90, 0).unwrap();
        assert_eq!((o.format, o.stride), (format::RGBX_8888, 448));
        assert!(output(&handle(format::RGBX_8888, 100, 50), 0x1000, 64, 50, 90, 0).is_none());
        assert!(output(&handle(format::RGB_565, 100, 50), 0x1000, 100, 50, 90, 0).is_none());
    }

    #[test]
    fn jpeg_outputs_leave_room_for_the_blob() {
        let h = handle(format::BLOB, 300_000, 1);
        let o = output(&h, 0x1000, 1920, 1080, 95, -90).unwrap();
        assert_eq!(
            (o.width, o.height, o.jpeg_quality, o.jpeg_orientation),
            (1920, 1080, 95, 270)
        );
        assert_eq!(o.length, 300_000 - 8);
    }

    #[test]
    fn a_mapped_buffer_gets_its_trailer() {
        let h = handle(format::BLOB, 4096, 1);
        // SAFETY: a new memfd owned by this test.
        let fd = unsafe { libc::memfd_create(c"camera-test".as_ptr(), 0) };
        assert!(fd >= 0);
        // SAFETY: our memfd.
        assert_eq!(unsafe { libc::ftruncate(fd, h.total_size() as i64) }, 0);
        // SAFETY: our fd, alive for the test.
        let b = Buffer::map(unsafe { BorrowedFd::borrow_raw(fd) }, h).unwrap();
        b.finish_jpeg(1234);
        // SAFETY: inside the mapping.
        let tail = unsafe { std::slice::from_raw_parts(b.base.add(4096 - 8), 8) };
        assert_eq!(tail, [0xff, 0, 0, 0, 0xd2, 0x04, 0, 0]);
        drop(b);
        // SAFETY: our fd.
        unsafe { libc::close(fd) };
    }
}
