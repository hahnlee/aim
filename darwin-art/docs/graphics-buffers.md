# Graphics buffers (ADR 0012, phase P4)

Graphics buffers are what the original libui, libgui, SurfaceFlinger, HWUI
and the media stack pass between processes as `buffer_handle_t`. The derived
image's vendor partition provides the two pieces a device vendor would:

| Piece | Guest path | Source |
| --- | --- | --- |
| Allocator, `android.hardware.graphics.allocator.IAllocator/default` V2 | `/vendor/bin/hw/android.hardware.graphics.allocator-service.darwin` | `hal/graphics/allocator` |
| Mapper, stable-C `AIMapper` v5, instance `darwin` | `/vendor/lib64/hw/mapper.darwin.so` | `hal/graphics/mapper` |
| Layout, handle and metadata code both share | (linked in) | `hal/graphics/gralloc` |

Both are guest ELF (Rust, `aarch64-linux-android`). Neither needs the host:
a buffer is ordinary shared memory. The host only sees a buffer when a GPU
user (the EGL/GLES driver, later the composer and Vulkan) imports it; see
[GPU access](#gpu-access).

## Design: memfd memory, imported by the host GPU

Every buffer is one `memfd` (in the syscall layer, an unlinked temporary
file that every process maps `MAP_SHARED`; its size is set once with
`ftruncate`). The pixel planes start at offset 0, so every mapping of the buffer starts on a
page boundary. That is the one property the host needs: on Apple silicon the
GPU uses the same memory as the CPU, and Metal wraps page-aligned memory
without copying (`newBufferWithBytesNoCopy`). A linear `MTLTexture` over that
`MTLBuffer` is then sampled and rendered to directly, and ANGLE imports that
texture as an `EGLImage` (`EGL_ANGLE_metal_texture_client_buffer`).

Why not IOSurface:

- An IOSurface cannot wrap existing memory, so it would have to be the
  allocation itself. Sharing it across processes then needs a Mach port
  (`IOSurfaceCreateMachPort`), which a `native_handle` cannot carry (it
  holds fds and ints), or a global IOSurface id (`IOSurfaceLookup`), which
  any process can guess. The binder driver already moves fds between guests
  (as fileports).
- A memfd is what Android's own software and emulator allocators use, so
  every CPU path of the original userspace (lock/unlock, `AHardwareBuffer`
  CPU access, media codecs, screenshots) works on it unchanged.
- The cost is a linear rather than a twiddled GPU layout. On Apple GPUs,
  rendering to a linear texture resolves tile memory to it at the end of a
  pass, and sampling from it is fast for the access patterns of composition
  and UI. Where a surface needs a tiled layout (MSAA, depth), the renderer
  keeps a private attachment and resolves into the buffer.

Measured with the pinned ANGLE on an M2 Pro: a shared mapping of the
buffer's memory, wrapped as `MTLBuffer` and `MTLTexture`
(`RGBA8Unorm`, `bytesPerRow` = the buffer stride) and imported as an
`EGLImage` renders and reads back correctly. Metal's minimum linear texture
alignment for 8-bit RGBA is 16 bytes; we align rows to 64.

## The handle

`native_handle_t` with `numFds = 1` and `numInts = 19`. The ints are 32-bit
slots; 64-bit values are two slots, low half first.

| Slot | Field | Notes |
| --- | --- | --- |
| fd 0 | memory | the memfd, `O_RDWR` |
| 0 | magic | `0x44474231` |
| 1 | width | pixels |
| 2 | height | pixels |
| 3 | format | resolved `PixelFormat` (never `IMPLEMENTATION_DEFINED`) |
| 4 | requested format | `PixelFormat` as asked for |
| 5 | layer count | |
| 6, 7 | usage | `BufferUsage` bits |
| 8 | stride | pixels (luma samples for YUV); `AllocationResult.stride` |
| 9, 10 | buffer id | unique across processes: allocator pid << 32 \| counter |
| 11, 12 | data size | bytes of all planes of all layers, from offset 0 |
| 13, 14 | metadata offset | page aligned, after the data |
| 15, 16 | reserved size | `BufferDescriptorInfo.reservedSize` |
| 17, 18 | layer size | bytes per layer; layer n starts at n × layer size |

The mapper rejects any handle whose fd count, int count or magic differ, or
whose memfd is shorter than the metadata region's end. The layout of every
plane is a pure function of (format, width, height, stride) in
`hal/graphics/gralloc`, so any process computes it from the handle alone.

### Memory layout

```text
0                    data size          metadata offset (page aligned)
| layer 0 | layer 1 | ... |  (padding)  | SharedMetadata | reserved region |
```

- **SharedMetadata** is a `#[repr(C)]` block the allocator initializes and
  every importer maps: the mutable metadata (dataspace, blend mode, HDR
  static and dynamic metadata) lives there, so a value set in one process is
  seen in all of them, as gralloc requires. Writers do not lock; concurrent
  setters of the same buffer are not synchronized (as in the AOSP reference
  implementations).
- **Reserved region**: `getReservedRegion` returns a pointer 8192 bytes into
  the metadata page(s), `reservedSize` bytes long.

## Formats

| `PixelFormat` | Planes and layout | GPU (Metal) format |
| --- | --- | --- |
| `RGBA_8888` (1) | 4 bytes, R G B A | `RGBA8Unorm` |
| `RGBX_8888` (2) | 4 bytes, R G B X | `RGBA8Unorm` (alpha ignored by the sampler) |
| `RGB_888` (3) | 3 bytes | none (CPU only) |
| `RGB_565` (4) | 16 bits, R in the high bits | `B5G6R5Unorm` |
| `BGRA_8888` (5) | 4 bytes, B G R A | `BGRA8Unorm` |
| `RGBA_FP16` (0x16) | 4 × half float | `RGBA16Float` |
| `RGBA_1010102` (0x2b) | 32 bits, R in the low 10 | `RGB10A2Unorm` |
| `R_8` (0x38) | 1 byte | `R8Unorm` |
| `BLOB` (0x21) | width bytes, height 1 | none (data buffer) |
| `YCBCR_420_888` (0x23) | NV12: Y plane, then interleaved Cb Cr at half resolution | none yet |
| `YCRCB_420_SP` (0x11) | NV21: Y plane, then interleaved Cr Cb | none yet |
| `YV12` (0x32315659) | Y, then Cr, then Cb; chroma stride `align(stride / 2, 16)` | none yet |
| `YCBCR_P010` (0x36) | 16-bit Y plane, then 16-bit Cb Cr pairs; 10 bits in the high bits | none yet |

- `IMPLEMENTATION_DEFINED` resolves to `YCBCR_420_888` when the usage has a
  video-encoder, camera or video-decoder bit, and to `RGBX_8888` otherwise.
- Strides: the row of a single-plane format is aligned to 64 bytes (the
  pixel stride is rounded so that this holds). YUV luma rows are aligned to
  64 bytes, and chroma planes follow at the offsets listed in their plane
  layouts.
- Depth and stencil formats, `RAW*`, `Y8`/`Y16` and the 4:2:2 formats are
  answered `isSupported = false`.

## Usage

All `BufferUsage` bits are accepted except:

- `PROTECTED`: there is no protected memory path;
- `GPU_MIPMAP_COMPLETE` and `GPU_CUBE_MAP`: a texture over a linear buffer
  has one level and one face;
- GPU usage (`GPU_TEXTURE`, `GPU_RENDER_TARGET`, `COMPOSER_*`) with a layer
  count above 1, or on a format with no Metal format above.

CPU usage never changes the layout: every buffer is always CPU mappable,
because software rendering, HWUI's fallback and screenshots lock buffers
that were allocated for the GPU.

## Mapper behaviour

- `importBuffer` duplicates the fd, maps the whole memfd once (`MAP_SHARED`,
  read-write) and returns a new handle that the mapper owns. The mapping
  lives until `freeBuffer`.
- `lock` waits on the acquire fence (polls it, then closes it) and returns
  the base address of the buffer (`accessRegion` does not offset it, as in
  gralloc). `unlock` returns release fence -1: CPU writes are visible to
  every mapping and to the GPU without a flush.
- `flushLockedBuffer` and `rereadLockedBuffer` are no-ops for the same
  reason.
- `getTransportSize` is (1, 19).

## Metadata

Encoded exactly as `IMapperMetadataTypes.h` does (header: name string and
type value, little-endian, strings and blobs prefixed by an `int64`
length).

| `StandardMetadataType` | Get | Set | Value |
| --- | --- | --- | --- |
| `BUFFER_ID` (1) | yes | | handle |
| `NAME` (2) | yes | | allocation name |
| `WIDTH`, `HEIGHT`, `LAYER_COUNT` (3–5) | yes | | handle |
| `PIXEL_FORMAT_REQUESTED` (6) | yes | | handle |
| `PIXEL_FORMAT_FOURCC` (7) | yes | | DRM fourcc of the resolved format |
| `PIXEL_FORMAT_MODIFIER` (8) | yes | | 0 (`DRM_FORMAT_MOD_LINEAR`) |
| `USAGE` (9) | yes | | handle |
| `ALLOCATION_SIZE` (10) | yes | | data size |
| `PROTECTED_CONTENT` (11) | yes | | 0 |
| `COMPRESSION` (12) | yes | | `Compression.NONE` |
| `INTERLACED` (13) | yes | | `Interlaced.NONE` |
| `CHROMA_SITING` (14) | yes | | `NONE` for RGB, `SITED_INTERSTITIAL` for YUV |
| `PLANE_LAYOUTS` (15) | yes | | from the layout |
| `CROP` (16) | yes | | one rect per plane, the whole plane |
| `DATASPACE` (17) | yes | yes | shared; `UNKNOWN` at allocation |
| `BLEND_MODE` (18) | yes | yes | shared; `INVALID` at allocation |
| `SMPTE2086` (19) | yes | yes | shared; absent at allocation |
| `CTA861_3` (20) | yes | yes | shared; absent at allocation |
| `SMPTE2094_40` (21) | yes | yes | shared, up to 2048 bytes |
| `SMPTE2094_10` (22) | yes | yes | shared, up to 2048 bytes |
| `STRIDE` (23) | yes | | handle |

Vendor (non-standard) metadata types are unsupported. `dumpBuffer` reports
every gettable standard type.

## Registration

- The vintf fragment declares `IAllocator/default` V2 (AIDL) and the native
  HAL `mapper` 5.0, instance `darwin`. libui's Gralloc5 asks the allocator
  for its suffix (`getIMapperLibrarySuffix` = `darwin`) and loads
  `mapper.darwin.so` through `AServiceManager_openDeclaredPassthroughHal`,
  which checks the native declaration with servicemanager.
- These replace the emulator's `android.hardware.graphics.allocator-service.ranchu`
  and `mapper.ranchu.so`.

## GPU access

Guest code never sees Metal. A GPU user maps the buffer itself (it knows this
handle layout, as a vendor's driver knows its gralloc's) and passes the
mapping to its host module:

- **EGL/GLES** (`/vendor/lib64/egl/libGLES_darwin.so`, host module `gpu`):
  `eglCreateImageKHR(EGL_NATIVE_BUFFER_ANDROID)` maps the buffer, and the
  host wraps the mapping as `MTLBuffer` + linear `MTLTexture` and imports
  it into ANGLE as an `EGLImage`. The buffer's memory is the texture: no
  copy either way. Window surfaces render to an ANGLE pbuffer and blit into
  the dequeued buffer's texture on `eglSwapBuffers` (flipped, so row 0 is the
  top row, as Android expects of window buffers). See `docs/gles-driver.md`.
- **Composer (next)**: the same import gives the composer a Metal texture of
  each layer's buffer in its own process. The memory is shared, so no
  cross-process GPU object is needed.

Synchronization: until sync fences exist in the syscall layer, a GPU writer
finishes its GPU work before it returns a buffer (release fence -1), and
readers need no acquire wait.
