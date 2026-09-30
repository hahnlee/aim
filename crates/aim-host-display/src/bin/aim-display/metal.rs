//! Buffers as Metal textures, and presenting one into the windows' layers.
//!
//! A graphics buffer is a memfd whose pixels start at offset 0
//! (docs/graphics-buffers.md). The server maps it once; the mapping starts
//! on a page, so Metal wraps it without copying
//! (`newBufferWithBytesNoCopy`) and a linear texture over that buffer is
//! the buffer's memory. A present is one render pass that samples the
//! texture into the layer's drawable: the drawable is Core Animation's, so
//! this is the one copy a frame costs. It also converts the format (the
//! layer is BGRA) and scales when the sizes differ. A layer may show part
//! of the buffer (a task's window in window mode, docs/windows.md). The
//! present fence signals when Core Animation has shown every drawable of
//! the present, with the latest time.

use std::ffi::c_void;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use aim_hostcall::display::Import;
use aim_sync_file::Writer;

use crate::objc::{GlobalBlock, Id, Pool, class, nsstring, release, retain, text};
use crate::vsync;

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

const MTL_PIXEL_FORMAT_BGRA8_UNORM: usize = 80;
const MTL_STORAGE_MODE_SHARED: usize = 0;
const MTL_TEXTURE_USAGE_SHADER_READ: usize = 1;
const MTL_LOAD_ACTION_CLEAR: usize = 2;
const MTL_LOAD_ACTION_DONT_CARE: usize = 0;
const MTL_STORE_ACTION_STORE: usize = 1;
const MTL_PRIMITIVE_TYPE_TRIANGLE: usize = 3;

const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 position [[position]]; float2 uv; };
// One triangle over the whole target; uv (0,0) is the top-left texel, as
// row 0 is the top row of an Android buffer. `crop` is the part of the
// texture shown: left, top, right, bottom, in texture coordinates.
vertex V vertex_main(uint id [[vertex_id]], constant float4& crop [[buffer(0)]]) {
    float2 t = float2((id << 1) & 2, id & 2);
    return V { float4(t * float2(2, -2) + float2(-1, 1), 0, 1), mix(crop.xy, crop.zw, t) };
}
fragment half4 fragment_main(V in [[stage_in]], texture2d<half> t [[texture(0)]]) {
    constexpr sampler s(filter::linear);
    return half4(t.sample(s, in.uv).rgb, 1.0h);
}
"#;

/// `PixelFormat` → `MTLPixelFormat`, bytes per pixel.
fn metal_format(format: i32) -> Option<(usize, u32)> {
    Some(match format {
        0x1 | 0x2 => (70, 4), // RGBA_8888, RGBX_8888: RGBA8Unorm (alpha is ignored)
        0x4 => (40, 2),       // RGB_565: B5G6R5Unorm
        0x5 => (80, 4),       // BGRA_8888: BGRA8Unorm
        0x16 => (115, 8),     // RGBA_FP16: RGBA16Float
        0x2b => (90, 4),      // RGBA_1010102: RGB10A2Unorm
        _ => return None,
    })
}

/// An imported buffer: its mapping, and the Metal objects over it.
pub struct Texture {
    pub import: Import,
    map: *mut c_void,
    buffer: Id,
    texture: Id,
    _fd: OwnedFd,
}

// SAFETY: Metal objects are thread-safe; the mapping is plain memory.
unsafe impl Send for Texture {}
unsafe impl Sync for Texture {}

impl Texture {
    /// The buffer's memfd, to hand to a window host.
    pub fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self._fd)
    }

    /// The pixels, `import.length` bytes.
    pub fn pixels(&self) -> &[u8] {
        // SAFETY: mapped for the texture's lifetime.
        unsafe { std::slice::from_raw_parts(self.map.cast(), self.import.length as usize) }
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        release(self.texture);
        release(self.buffer);
        // SAFETY: our mapping; Metal no longer references it.
        unsafe { libc::munmap(self.map, self.import.length as usize) };
    }
}

pub struct Renderer {
    device: Id,
    queue: Id,
    pipeline: Id,
}

// SAFETY: Metal devices, queues and pipelines are thread-safe.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

/// A layer to present into, and what it shows.
pub struct Target {
    /// A `CAMetalLayer`, retained.
    pub layer: Id,
    /// The part of the buffer it shows, in pixels: left, top, right,
    /// bottom. None: all of it.
    pub crop: Option<[i32; 4]>,
}

// SAFETY: `CAMetalLayer` hands out drawables on any thread.
unsafe impl Send for Target {}

impl Target {
    pub fn new(layer: Id, crop: Option<[i32; 4]>) -> Target {
        Target {
            layer: retain(layer),
            crop,
        }
    }
}

impl Clone for Target {
    fn clone(&self) -> Target {
        Target::new(self.layer, self.crop)
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        release(self.layer);
    }
}

/// What one present cost.
pub struct Frame {
    /// From the request to the GPU finishing the pass.
    pub cpu_ns: u64,
    /// GPU execution of the pass.
    pub gpu_ns: u64,
}

/// A present's fence: signaled once everything that shows the frame has
/// shown it (each drawable, and each window host), at the display's first
/// vsync at or after the latest of their times, as a panel's present fence
/// signals at the refresh that starts showing the frame. SurfaceFlinger
/// predicts vsync from these times, and not every one is a refresh's: a
/// drawable Core Animation dropped for a newer one counts when its handler
/// ran, and a window host when it read the frame. The present itself holds
/// one count until [`Fence::done`].
#[derive(Clone)]
pub struct Fence(Arc<Mutex<(usize, i64, Option<Writer>)>>);

impl Fence {
    pub fn new(writer: Writer) -> Fence {
        Fence(Arc::new(Mutex::new((1, 0, Some(writer)))))
    }

    /// One more to wait for.
    pub fn expect(&self) {
        self.0.lock().unwrap().0 += 1;
    }

    /// One shown at `ns`.
    pub fn shown(&self, ns: i64) {
        let mut p = self.0.lock().unwrap();
        p.1 = p.1.max(ns);
        p.0 -= 1;
        if p.0 == 0
            && let Some(w) = p.2.take()
        {
            let at = if p.1 > 0 { p.1 } else { vsync::monotonic_ns() };
            w.signal_at(vsync::vsync_at_or_after(at), 1);
        }
    }

    /// The present has handed out all it waits for.
    pub fn done(&self) {
        self.shown(0);
    }
}

/// Present fences of drawables not shown yet, by drawable.
static PRESENTED: Mutex<Vec<(usize, Fence)>> = Mutex::new(Vec::new());

/// Count each of `drawables` for `fence` until it is shown, and `watch`
/// it. Core Animation runs presented handlers under a lock of its own
/// that adding one waits for, so `watch` runs without [`PRESENTED`] held:
/// holding it deadlocked the server with the handler of an earlier
/// present, and the present fence never signaled.
fn expect_shown(fence: &Fence, drawables: &[Id], watch: impl Fn(Id)) {
    {
        let mut p = PRESENTED.lock().unwrap();
        for &d in drawables {
            fence.expect();
            p.push((d as usize, fence.clone()));
        }
    }
    for &d in drawables {
        watch(d);
    }
}

/// The fence waiting for `drawable` to be shown.
fn take_shown(drawable: Id) -> Option<Fence> {
    let mut p = PRESENTED.lock().unwrap();
    p.iter()
        .position(|(d, _)| *d == drawable as usize)
        .map(|i| p.swap_remove(i).1)
}

/// A drawable's presented handler: count it shown for its present's fence,
/// at the time it was shown (now, when it was dropped instead).
extern "C" fn presented(_block: *const GlobalBlock, drawable: Id) {
    if let Some(f) = take_shown(drawable) {
        let shown = send!(drawable, c"presentedTime" => f64);
        f.shown(if shown > 0.0 {
            vsync::uptime_to_monotonic(shown)
        } else {
            vsync::monotonic_ns()
        });
    }
}

fn presented_block() -> &'static GlobalBlock {
    static BLOCK: OnceLock<GlobalBlock> = OnceLock::new();
    BLOCK.get_or_init(|| GlobalBlock::new(presented as *const c_void))
}

pub fn device() -> Id {
    // SAFETY: no preconditions.
    unsafe { MTLCreateSystemDefaultDevice() }
}

impl Renderer {
    pub fn new(device: Id) -> Result<Renderer, String> {
        let _pool = Pool::new();
        let mut error: Id = std::ptr::null_mut();
        let library = send!(device, c"newLibraryWithSource:options:error:" => Id,
            Id = nsstring(SHADER), Id = std::ptr::null_mut(), *mut Id = &mut error);
        if library.is_null() {
            return Err(text(send!(error, c"localizedDescription" => Id)));
        }
        let desc = send!(class(c"MTLRenderPipelineDescriptor"), c"new" => Id);
        let vertex = send!(library, c"newFunctionWithName:" => Id, Id = nsstring("vertex_main"));
        let fragment =
            send!(library, c"newFunctionWithName:" => Id, Id = nsstring("fragment_main"));
        send!(desc, c"setVertexFunction:" => (), Id = vertex);
        send!(desc, c"setFragmentFunction:" => (), Id = fragment);
        let attachments = send!(desc, c"colorAttachments" => Id);
        let color = send!(attachments, c"objectAtIndexedSubscript:" => Id, usize = 0);
        send!(color, c"setPixelFormat:" => (), usize = MTL_PIXEL_FORMAT_BGRA8_UNORM);
        let pipeline = send!(device, c"newRenderPipelineStateWithDescriptor:error:" => Id,
            Id = desc, *mut Id = &mut error);
        for o in [vertex, fragment, desc, library] {
            release(o);
        }
        if pipeline.is_null() {
            return Err(text(send!(error, c"localizedDescription" => Id)));
        }
        Ok(Renderer {
            device,
            queue: send!(device, c"newCommandQueue" => Id),
            pipeline,
        })
    }

    /// Map a buffer and wrap it as a texture.
    pub fn import(&self, fd: OwnedFd, i: &Import) -> Result<Texture, String> {
        let (format, bpp) =
            metal_format(i.format).ok_or_else(|| format!("format {:#x}", i.format))?;
        let length = i.length as usize;
        if (i.stride_bytes as u64) < i.width as u64 * bpp as u64
            || i.stride_bytes as u64 * i.height as u64 > i.length
        {
            return Err(format!("layout {i:?}"));
        }
        // SAFETY: a shared read-only mapping of the buffer's memfd.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let _pool = Pool::new();
        let buffer = send!(self.device, c"newBufferWithBytesNoCopy:length:options:deallocator:" => Id,
            *mut c_void = map, usize = length, usize = MTL_STORAGE_MODE_SHARED,
            *const c_void = std::ptr::null());
        let texture = if buffer.is_null() {
            std::ptr::null_mut()
        } else {
            let desc = send!(class(c"MTLTextureDescriptor"),
                c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:" => Id,
                usize = format, usize = i.width as usize, usize = i.height as usize, bool = false);
            send!(desc, c"setStorageMode:" => (), usize = MTL_STORAGE_MODE_SHARED);
            send!(desc, c"setUsage:" => (), usize = MTL_TEXTURE_USAGE_SHADER_READ);
            send!(buffer, c"newTextureWithDescriptor:offset:bytesPerRow:" => Id,
                Id = desc, usize = 0, usize = i.stride_bytes as usize)
        };
        let t = Texture {
            import: *i,
            map,
            buffer,
            texture,
            _fd: fd,
        };
        if t.texture.is_null() {
            return Err(format!("Metal refused the buffer {i:?}"));
        }
        Ok(t)
    }

    /// Draw `t` (or black) into the next drawable of each target, show
    /// them at the next vsync and wait until the GPU has read `t`, so its
    /// buffer may be reused once this returns. `fence` waits for each
    /// drawable to be shown.
    pub fn present(
        &self,
        t: Option<&Texture>,
        targets: &[Target],
        fence: Option<&Fence>,
    ) -> Option<Frame> {
        let start = Instant::now();
        let _pool = Pool::new();
        let commands = send!(self.queue, c"commandBuffer" => Id);
        let mut drawables = Vec::new();
        for target in targets {
            let drawable = send!(target.layer, c"nextDrawable" => Id);
            if drawable.is_null() {
                continue;
            }
            self.encode(commands, drawable, t, target.crop);
            drawables.push(drawable);
        }
        if let Some(f) = fence {
            expect_shown(f, &drawables, |d| {
                send!(d, c"addPresentedHandler:" => (), *const GlobalBlock = presented_block());
            });
        }
        for &d in &drawables {
            send!(commands, c"presentDrawable:" => (), Id = d);
        }
        send!(commands, c"commit" => ());
        send!(commands, c"waitUntilCompleted" => ());
        if drawables.is_empty() {
            return None;
        }
        let gpu = send!(commands, c"GPUEndTime" => f64) - send!(commands, c"GPUStartTime" => f64);
        Some(Frame {
            cpu_ns: start.elapsed().as_nanos() as u64,
            gpu_ns: (gpu.max(0.0) * 1e9) as u64,
        })
    }

    /// One render pass: `t`'s `crop` (all of it when None), or black, over
    /// `drawable`.
    fn encode(&self, commands: Id, drawable: Id, t: Option<&Texture>, crop: Option<[i32; 4]>) {
        let pass = send!(class(c"MTLRenderPassDescriptor"), c"renderPassDescriptor" => Id);
        let attachments = send!(pass, c"colorAttachments" => Id);
        let color = send!(attachments, c"objectAtIndexedSubscript:" => Id, usize = 0);
        let target = send!(drawable, c"texture" => Id);
        send!(color, c"setTexture:" => (), Id = target);
        let load = if t.is_some() {
            MTL_LOAD_ACTION_DONT_CARE
        } else {
            MTL_LOAD_ACTION_CLEAR
        };
        send!(color, c"setLoadAction:" => (), usize = load);
        send!(color, c"setStoreAction:" => (), usize = MTL_STORE_ACTION_STORE);
        let encoder = send!(commands, c"renderCommandEncoderWithDescriptor:" => Id, Id = pass);
        if let Some(t) = t {
            let (w, h) = (t.import.width as f32, t.import.height as f32);
            let uv = match crop {
                Some([l, top, r, b]) => [l as f32 / w, top as f32 / h, r as f32 / w, b as f32 / h],
                None => [0.0, 0.0, 1.0, 1.0],
            };
            send!(encoder, c"setRenderPipelineState:" => (), Id = self.pipeline);
            send!(encoder, c"setVertexBytes:length:atIndex:" => (),
                *const c_void = uv.as_ptr().cast(), usize = size_of_val(&uv), usize = 0);
            send!(encoder, c"setFragmentTexture:atIndex:" => (), Id = t.texture, usize = 0);
            send!(encoder, c"drawPrimitives:vertexStart:vertexCount:" => (),
                usize = MTL_PRIMITIVE_TYPE_TRIANGLE, usize = 0, usize = 3);
        }
        send!(encoder, c"endEncoding" => ());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn a_presented_handler_may_run_while_one_is_added() {
        let (file, writer) = aim_sync_file::pair().unwrap();
        let fence = Fence::new(writer);
        let drawables = [0x10 as Id, 0x20 as Id];
        // As Core Animation: the handler runs on another thread while the
        // present waits for it to add the next.
        expect_shown(&fence, &drawables, |d| {
            let d = d as usize;
            std::thread::spawn(move || take_shown(d as Id).unwrap().shown(5))
                .join()
                .unwrap();
        });
        assert!(!aim_sync_file::wait(file.as_fd(), 0));
        fence.done();
        assert!(aim_sync_file::wait(file.as_fd(), 1000));
    }
}
