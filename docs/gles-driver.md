# GLES driver (ADR 0012, phase P4)

The derived image's GPU driver is `/vendor/lib64/egl/libGLES_aim.so`, a
guest ELF library that the original `/system/lib64/libEGL.so` loads like any
device's driver. Every EGL and OpenGL ES entry point in it is a thunk that
makes one host call; the host calls the same entry point of ANGLE (OpenGL ES
over Metal). Guest pointers are host pointers, so arguments pass through
untouched, the way Wine's `opengl32` reaches the host's GL.

| Piece | Where |
| --- | --- |
| Guest driver (Rust cdylib) | `hal/gles` |
| Thunks, generated from the Khronos registry | `tools/gen-gpu-thunks.py` → `hal/gles/src/thunks.rs`, `crates/aim-host-gpu/src/table.rs` |
| Host module `gpu` (id 2) | `crates/aim-host-gpu` |
| ABI (functions, argument blocks) | `aim_hostcall::gpu` |
| Graphics buffers it renders into | [graphics-buffers.md](graphics-buffers.md) |

## Loading

The original loader (`frameworks/native/opengl/libs/EGL/Loader.cpp`) tries
`libGLES_<suffix>.so`, then `libEGL_`/`libGLESv1_CM_`/`libGLESv2_<suffix>.so`,
in `/vendor/lib64/egl`, for the first set property of `persist.graphics.egl`,
`ro.hardware.egl` and `ro.board.platform`, and loads it into the `sphal`
namespace. One library serves EGL, GLES 1 and GLES 2/3.

- `ro.hardware.egl` comes from the device's init script:
  `/vendor/etc/init/hw/init.aim.rc` sets it to `aim`.
- The driver needs only LL-NDK libraries: libc, libdl and
  `libnativewindow.so` (for `ANativeWindow_dequeueBuffer` and friends).
- `linux-run --gpu DIR` names the directory of the host's ANGLE
  (`libEGL.dylib`, `libGLESv2.dylib`; the pinned build is
  `_build/angle-source/out/AimRelease`, Metal backend only). Without
  it, `eglGetDisplay` fails and GL calls do nothing.
- The ANGLE pin is `37c976ffba9d2940add977cad9945b23de14c150`
  (`upstream/angle.lock`), a checkout of
  `chromium.googlesource.com/angle/angle` (with `gclient sync`) in
  `_build/angle-source`, built with `gn gen out/AimRelease` and these
  `args.gn` (`upstream/angle-args.gn`), then `ninja -C out/AimRelease
  libEGL libGLESv2`. The `angle` node of `cargo aim` does all of this
  ([build.md](build.md), "ANGLE"):

  ```
  is_debug = false
  is_component_build = true
  angle_enable_metal = true
  angle_enable_gl = false
  angle_enable_vulkan = false
  angle_enable_wgpu = false
  angle_enable_null = false
  angle_enable_d3d11 = false
  angle_build_all = false
  target_cpu = "arm64"
  ```

## Thunks

`tools/gen-gpu-thunks.py` forwards every command that the registry defines
(`gl.xml`, `egl.xml`, and ANGLE's own `gl_angle_ext.xml`/`egl_angle_ext.xml`)
and that the pinned ANGLE build exports: 935 entry points, 112 of them EGL.
The output is checked in, so a build needs neither the registry nor ANGLE.
Rerun the script after changing the ANGLE pin.

- **One call.** A thunk stores its arguments into the callee's register
  image on its stack (integer values, then float values, then the stack
  words laid out as Apple's arm64 ABI packs them, 8 bytes each) and makes
  host call `FN_TABLE_BASE + index`. The host copies the image into x0–x7
  and d0–d7, copies the stack words, and calls ANGLE's entry point; x0 comes
  back unchanged. The host side is one assembly trampoline, not per-function
  code. The call is an out-of-line `svc` stub, so a thunk saves only what
  AAPCS64 makes a caller save.
- **Agreement.** Guest and host use the same generated table. `FN_INIT`
  compares its hash and length; a mismatch fails `eglGetDisplay`.
- **Availability.** `FN_INIT` also returns a bitmap of the entry points ANGLE
  resolved; `eglGetProcAddress` returns a thunk only for those.
- **Callbacks.** Host code never calls guest code (host-call rule). Commands
  taking a function pointer are not forwarded:
  `glDebugMessageCallback(KHR)`, `eglDebugMessageControlKHR` and
  `eglSetBlobCacheFuncsANDROID` are accepted and never call back, and
  `EGL_ANDROID_blob_cache` and `EGL_KHR_debug` are withheld from the
  extension string.
- **Autorelease pools.** EGL calls create Metal objects; guest threads have
  no pool, so the host wraps each EGL call (not GL calls) in one.

## EGL on the Android platform

`hal/gles/src/egl.rs` implements what the Android platform adds to EGL; every
other call is a plain thunk.

- **Display.** `eglGetDisplay(EGL_DEFAULT_DISPLAY)` and
  `eglGetPlatformDisplay(EGL_PLATFORM_ANDROID_KHR)` are ANGLE's Metal
  display, created on first use (see "Displays and fork"). ANGLE's Metal
  backend offers OpenGL ES 3.0.
- **Configs.** `EGL_RECORDABLE_ANDROID` and `EGL_FRAMEBUFFER_TARGET_ANDROID`
  are true for every config and dropped from `eglChooseConfig` lists;
  `EGL_NATIVE_VISUAL_ID` is the `PixelFormat` of the config's color buffer
  (8888 → `RGBA_8888`/`RGBX_8888`, 565, 1010102, FP16).
- **Window surfaces.** An `ANativeWindow` surface is a host pbuffer of the
  window's size, which is the surface's default framebuffer. On
  `eglSwapBuffers` the driver dequeues a buffer, imports it (once per
  buffer), and the host (`FN_PRESENT`) blits the pbuffer into it through a
  framebuffer with `GL_MESA_framebuffer_flip_y`, so the buffer's row 0 is the
  top of the image as Android expects. The buffer is then queued. A new
  window size takes effect at the next swap (a new pbuffer, rebound if
  current). `eglSwapBuffersWithDamage` presents the whole surface;
  `eglPresentationTimeANDROID` and `eglSwapInterval` go to the window.
- **Native buffers.** `eglCreateImageKHR(EGL_NATIVE_BUFFER_ANDROID)` maps
  the buffer (graphics-buffers.md, "GPU access") and the host imports the
  mapping as a linear Metal texture, then as an ANGLE `EGLImage`: sampling
  and rendering use the buffer's memory directly. `RGBX_8888` imports with
  a `GL_RGB` internal format, so alpha reads as 1.
- **Extensions added**: `EGL_ANDROID_image_native_buffer`,
  `EGL_ANDROID_recordable`, `EGL_ANDROID_framebuffer_target`,
  `EGL_ANDROID_presentation_time`, `EGL_KHR_swap_buffers_with_damage`.

## Displays and fork

Zygote preloads the driver: `ZygoteInit.preload` →
`nativePreloadGraphicsDriver` → HWUI's `zygote_preload_graphics`, which
calls `eglGetDisplay(EGL_DEFAULT_DISPLAY)` when HWUI renders with GL. It
then forks every app without exec. ANGLE's `eglGetPlatformDisplay` already
creates the Metal device (`angle::IsMetalRendererAvailable` calls
`MTLCreateSystemDefaultDevice`, which connects to the window server through
SkyLight), and no host object of a parent is any use to a child. A child of
Darwin `fork()` could not even compile shaders ("Unable to reach
MTLCompilerService"), so app windows stayed black (#233) while
SurfaceFlinger, started by exec, drew. A guest fork is therefore a freshly
spawned process that takes over the parent's memory and state
(`docs/fork.md`), and the host module (`crates/aim-host-gpu`) keeps
Metal out of zygote:

- `eglGetDisplay`, `eglGetPlatformDisplay` and `eglGetPlatformDisplayEXT`
  record the request and return a handle, a small number
  (`src/display.rs`); nothing reaches ANGLE or Metal.
- The first call that passes the handle (`eglInitialize`) makes the request
  of ANGLE in the calling process; every EGL call's display argument (always
  the first) and `FN_IMPORT_BUFFER`'s display are translated, and
  `eglGetCurrentDisplay` returns the handle. A failed request is retried at
  the next use; the call gets `EGL_NO_DISPLAY` and fails with
  `EGL_BAD_DISPLAY`.
- A fork child takes over the handles (`fork_state`), but none of ANGLE's
  displays: each is made again on its first use there, and initialized if
  the guest had initialized it in the parent, since the guest's EGL will
  not ask again. The child also loads ANGLE on the first forwarded call if
  the parent had loaded it (`FN_INIT`, which libEGL's loader triggers
  through `eglGetProcAddress`, runs only in zygote).

`tests/fork.rs` runs zygote's pattern on the host (load ANGLE, get the
display, fork the way the layer does, then compile a shader unique to the
run and draw in the child), and `tests/graphics.rs` runs it in the guest
(`gles_triangle fork`).

## External textures

Skia (SurfaceFlinger's RenderEngine, HWUI) samples every buffer it only
reads through `GL_TEXTURE_EXTERNAL_OES`, which ANGLE's Metal backend does
not offer. RenderEngine aborts without it. `hal/gles/src/external.rs`
emulates `GL_OES_EGL_image_external` and `_essl3` over 2D textures, as
ANGLE's Vulkan backend does:

- A unit's external binding point is kept apart from its 2D one, since
  callers cache both. It lives in the 2D binding of a hidden unit
  `u + K`, where `K` is half the host's combined units, and the
  guest is told it has `K` units. Binding, `glTexParameter*`,
  `glGetTexParameter*`, `glEGLImageTargetTexture2DOES` and
  `GL_TEXTURE_BINDING_EXTERNAL_OES` on the external target act on the
  hidden unit.
- `glShaderSource` turns `samplerExternalOES` into `sampler2D` and drops
  the extension's `#extension` lines (keeping line numbers). It records
  the samplers' names per shader. After `glLinkProgram`, their locations
  point at the hidden units: `K` at link time, and `glUniform1i`/`1iv`
  add `K`.
- The extension names are added to `GL_EXTENSIONS` and to
  `glGetStringi`.

## Synchronization

There are no sync-file fences yet. A present waits for the GPU (`glFinish`)
before the buffer is queued with fence -1, and `EGL_ANDROID_native_fence_sync`
is not offered, so consumers (SurfaceFlinger's RenderEngine, HWUI) wait on the
CPU. Both are the first costs to remove.

`eglClientWaitSync(KHR)` on a thread without a current context polls the
sync's `EGL_SYNC_STATUS` every 100 µs up to the timeout. EGL allows such a
wait, but ANGLE's Metal backend fails it with `EGL_BAD_CONTEXT`, and HWUI's
`HardwareBitmapUploader` waits that way for the fence of its upload thread
("Failed to wait for the fence 0x3006" in the launcher).

## Measured (M2 Pro, `tests/graphics.rs`, release)

| What | Time |
| --- | --- |
| `glClearColor`, ANGLE called directly on the host | 3.8 ns |
| the same through the host dispatch (no guest) | 9.2 ns |
| `glClearColor` from the guest through the original libEGL | 15.6 ns |
| `glGetError` from the guest | 25 ns |
| 64×64 clear + triangle, pipelined | 6.3 µs/frame |
| 64×64 clear + triangle + `glFinish` | 317 µs/frame |
| window surface: triangle + `eglSwapBuffers` + consumer acquire | 326 µs/frame |
| first `eglSwapBuffers` of a buffer (import) | 1.4 ms |

The per-call cost is about 12 ns above ANGLE's own: the host-call entry
(4.6 ns), the module dispatch and the register copy. Frame times are
dominated by Metal's submit-and-wait, which the missing fences force.
