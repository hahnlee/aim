//! Frame callback mailbox and diagnostic frame serialization.

use std::mem::size_of;
use std::ptr;

const MAX_FRAME_DIMENSION: u32 = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedFrame {
    pub width: u32,
    pub height: u32,
    pub argb_pixels: Vec<u32>,
}

pub struct FrameHost {
    pub frames_received: u64,
    pub last_frame: Option<OwnedFrame>,
}

impl FrameHost {
    pub unsafe fn receive(
        &mut self,
        pixels: *const u32,
        width: u32,
        height: u32,
        stride_bytes: usize,
    ) -> bool {
        let Some(row_bytes) = (width as usize).checked_mul(size_of::<u32>()) else {
            return false;
        };
        let Some(pixel_count) = (width as usize).checked_mul(height as usize) else {
            return false;
        };
        if pixels.is_null()
            || width == 0
            || height == 0
            || width > MAX_FRAME_DIMENSION
            || height > MAX_FRAME_DIMENSION
            || stride_bytes < row_bytes
        {
            return false;
        }
        let Some(last_row_offset) = (height as usize - 1).checked_mul(stride_bytes) else {
            return false;
        };
        if last_row_offset.checked_add(row_bytes).is_none() {
            return false;
        }

        // The C ABI borrows the source only for this callback. Keep an owned,
        // tightly packed copy and return immediately; presentation happens
        // after ART has released the managed execution boundary.
        let mut owned = vec![0_u32; pixel_count];
        let source = pixels.cast::<u8>();
        let destination = owned.as_mut_ptr().cast::<u8>();
        for row in 0..height as usize {
            // SAFETY: the producer guarantees stride_bytes of readable frame
            // storage for the callback duration; arithmetic was checked above.
            unsafe {
                ptr::copy_nonoverlapping(
                    source.add(row * stride_bytes),
                    destination.add(row * row_bytes),
                    row_bytes,
                );
            }
        }

        self.last_frame = Some(OwnedFrame {
            width,
            height,
            argb_pixels: owned,
        });
        self.frames_received += 1;
        true
    }
}

pub unsafe extern "C" fn receive_frame(
    context: *mut std::ffi::c_void,
    pixels: *const u32,
    width: u32,
    height: u32,
    stride_bytes: usize,
) -> i32 {
    if context.is_null() {
        return 0;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: run() passes a live, uniquely borrowed FrameHost for the
        // synchronous duration of aim_run_process.
        unsafe { (&mut *context.cast::<FrameHost>()).receive(pixels, width, height, stride_bytes) }
    }));
    i32::from(result.unwrap_or(false))
}
