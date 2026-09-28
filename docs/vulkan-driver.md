# Vulkan driver (ADR 0012)

The derived image's Vulkan driver is `/vendor/lib64/hw/vulkan.aim.so`, a
guest ELF library that the original `/system/lib64/libvulkan.so` loads like
any device's Vulkan HAL. Every Vulkan command in it is a thunk that makes
one host call; the host calls the same command of MoltenVK (Vulkan over
Metal). Guest pointers are host pointers, so arguments, structures and their
`pNext` chains pass through untouched, as in the GLES driver
([gles-driver.md](gles-driver.md)).

| Piece | Where |
| --- | --- |
| Guest driver (Rust cdylib) | `hal/vulkan` |
| Thunks, generated from the Khronos registry (`vk.xml`) | `tools/gen-vulkan-thunks.py` → `hal/vulkan/src/thunks.rs`, `crates/aim-host-vulkan/src/table.rs` |
| Host module `vulkan` (id 10) | `crates/aim-host-vulkan` |
| ABI (functions, argument blocks) | `aim_hostcall::vulkan` |
| MoltenVK, pinned | `upstream/moltenvk.lock`, the `moltenvk` node of `cargo aim` |
| Graphics buffers it renders into | [graphics-buffers.md](graphics-buffers.md) |

## MoltenVK

The `moltenvk` node ([build.md](build.md)) downloads the official release
archive of the pin (v1.4.2, `MoltenVK-macos.tar` from the KhronosGroup
GitHub release), checks its sha256 and installs `libMoltenVK.dylib` and its
license unchanged in `target/aim/moltenvk`, with the `vk.xml` of the
Vulkan-Headers revision the release is built with. `linux-run --vulkan DIR`
(and `guest-init --vulkan`, which `cargo aim boot` passes) names that
directory; without it the driver fails to open and libvulkan reports no
driver.

MoltenVK is Apache-2.0 and links SPIRV-Cross, SPIRV-Tools, SPIRV-Headers,
cereal and the Vulkan headers; `THIRD_PARTY_NOTICES.md` lists them and how
the dylib ships (fetched by the build, loaded by `linux-run`, not part of the
repository or the Android image).

## Loading

The original loader (`frameworks/native/vulkan/libvulkan/driver.cpp`) loads
`vulkan.<ro.hardware.vulkan>.so` from `/vendor/lib64/hw` into the `sphal`
namespace, looks up `HMI` (a `hwvulkan_module_t`) and opens device `vk0`, a
`hwvulkan_device_t` with `EnumerateInstanceExtensionProperties`,
`CreateInstance` and `GetInstanceProcAddr`.

- `ro.hardware.vulkan` comes from the device's init script:
  `/vendor/etc/init/hw/init.aim.rc` sets it to `aim`. The emulator's
  `vulkan.ranchu.so` is removed.
- Opening the device loads MoltenVK in the host (`FN_INIT`), which checks
  that the guest's generated table is the host's and returns which entry
  points MoltenVK resolved; `vkGet*ProcAddr` returns a thunk only for those.
- Zygote preloads the driver when HWUI renders with Vulkan
  (`vkEnumerateInstanceVersion`); no Metal object is made then. A guest fork
  child is a fresh process (docs/fork.md): the host loads MoltenVK on the
  child's first forwarded call.
- The driver needs only LL-NDK libraries: libc, libdl and
  `libnativewindow.so` (`AHardwareBuffer_*`, including the LL-NDK
  `AHardwareBuffer_getNativeHandle`).

## Thunks

`tools/gen-vulkan-thunks.py` forwards every command that `vk.xml` defines
for the `vulkan` API, that the pinned MoltenVK exports under its name or an
alias, and that belongs to a core version or an extension the driver offers:
272 entry points under 383 names. The output is checked in and carries the
registry's license header; rerun the script after changing the pin.

- **One call.** As in the GLES driver: a thunk stores its arguments into the
  callee's register image (x registers, d registers, then stack words packed
  as Apple's arm64 ABI packs them) and makes host call `FN_TABLE_BASE +
  index`; one assembly trampoline on the host loads the registers and calls
  MoltenVK. Calls other than `vkCmd*` run in an autorelease pool, since
  guest threads have none and MoltenVK autoreleases Metal objects.
- **Dispatchable handles.** The loader requires dispatchable objects to
  begin with a `hwvulkan_dispatch_t` holding `HWVULKAN_DISPATCH_MAGIC`,
  which it overwrites with its dispatch table. MoltenVK's handles begin with
  `VK_LOADER_DATA` holding `ICD_LOADER_MAGIC`: the same word and the same
  value (0x01CDC0DE), which the Khronos loader overwrites the same way. So
  handles are MoltenVK's own, unwrapped
  (`crates/aim-host-vulkan/tests/moltenvk.rs` checks the magic). MoltenVK
  rewrites the word only where it returns a handle, where the loader sets it
  again, and in debug callbacks, which are never registered; the driver
  never asks MoltenVK for a handle the application may hold (it records the
  application's queues instead).
- **Callbacks.** Host code never calls guest code. `pAllocator` is not
  passed to the host (MoltenVK allocates for itself), and debug messenger
  and report create infos are taken out of `vkCreateInstance`'s chain.

## What the driver offers

Instance extensions are MoltenVK's less the withheld ones; the loader adds
`VK_KHR_surface`, `VK_KHR_android_surface` and the other surface extensions
itself, over the driver's `VK_ANDROID_native_buffer`. Device extensions are
MoltenVK's less the withheld ones, plus the driver's own:

| Extension | How |
| --- | --- |
| `VK_ANDROID_native_buffer` (spec 8) | Swapchain images over gralloc buffers (below); the loader offers `VK_KHR_swapchain` over it |
| `VK_ANDROID_external_memory_android_hardware_buffer` (5) | Import and export (below) |
| `VK_EXT_queue_family_foreign` (1) | Required by the one above; transfers to the foreign family are ordinary barriers |
| `VK_KHR_external_semaphore_fd` (1) | Sync-fd semaphores (below), which HWUI requires |

Withheld (`WITHHELD_*` in the generator): MoltenVK's window-system
extensions (`VK_KHR_surface`, `VK_EXT_metal_surface`, `VK_KHR_swapchain`
and its relatives, present id/wait, `VK_EXT_hdr_metadata`,
`VK_GOOGLE_display_timing`), which are for CAMetalLayer and which the
loader implements for Android; `VK_KHR_portability_enumeration`;
`VK_EXT_debug_report` and `VK_EXT_debug_utils` (callbacks);
`VK_EXT_layer_settings` and `VK_MVK_moltenvk` (MoltenVK's configuration);
and `VK_EXT_metal_objects` and `VK_EXT_external_memory_metal` (Metal
objects, which guest code cannot hold). `vkCreateDevice` removes the
driver's own extensions from the list it passes on and enables
`VK_EXT_external_memory_host`.

### Queue families

MoltenVK has one queue per family (a `VkQueue` is an `MTLCommandQueue`;
Metal has no queue families) and offers several alike general-purpose
families instead: four on an M2 Pro. HWUI's `VulkanManager`, like other
Vulkan applications, asks for two queues of its graphics family. When all
of a physical device's families are alike, the driver presents them as one
family with all their queues: queue `i` of family 0 is the host's family
`i` (`vkGetPhysicalDeviceQueueFamilyProperties{,2}`, `vkCreateDevice`,
`vkGetDeviceQueue{,2}`).

Family 0 is the host's first family, so every other family index means the
same to MoltenVK and passes through: command pools, barriers (an ownership
transfer is between two families; `VK_QUEUE_FAMILY_EXTERNAL` and
`VK_QUEUE_FAMILY_FOREIGN_EXT` pass as they are) and sharing lists
(concurrent sharing names two families or more, so there is none). A
command buffer from a pool of family 0 may run on any of its queues:
MoltenVK records command buffers apart from any queue and encodes them into
the queue they are submitted to (it would use the pool's family only to
prefill Metal command buffers, which is off).

### Swapchain images (`VK_ANDROID_native_buffer`)

The loader's `vkCreateSwapchainKHR` dequeues the window's buffers and
creates an image for each with a `VkNativeBufferANDROID` in the chain. The
driver creates the image in MoltenVK without the Android structures, maps
the buffer's memfd (once per buffer and process) and makes the mapping the
image's storage: the host wraps it without copying as an `MTLBuffer`
(`newBufferWithBytesNoCopy`), makes a linear `MTLTexture` over it in the
Metal format MoltenVK uses for the image's `VkFormat` (so an sRGB swapchain
gets an sRGB view of an RGBA_8888 buffer), and sets it as the image's
texture (`vkSetMTLTextureMVK`, `FN_ATTACH`; MoltenVK marks it deprecated,
and its replacements do not fit yet, #318). Rendering writes the buffer the
consumer reads; Vulkan's row 0 is the buffer's top row, as Android expects.

The producer usage the loader requests comes from
`vkGetPhysicalDeviceImageFormatProperties2` with an AHardwareBuffer
external-format query, as for any AHardwareBuffer image;
`vkGetSwapchainGrallocUsage2ANDROID` is the loader's required fallback.

### AHardwareBuffers

An AHardwareBuffer is one of our gralloc buffers, so an import maps its
memfd.

- **Images** (dedicated allocations, which the extension requires for
  images): the mapping becomes the image's storage as for swapchain images;
  the memory object is a device-local one MoltenVK allocates nothing for.
- **Buffers** (`BLOB`): the mapping is imported as host memory
  (`VK_EXT_external_memory_host`), host-visible and coherent.
- **Export** allocates an AHardwareBuffer that fits the dedicated image (or
  a `BLOB` of the allocation's size) and imports it.
- `vkGetAndroidHardwareBufferPropertiesANDROID` reports the data size, the
  memory types above and the Vulkan format: RGBA_8888 and RGBX_8888 (alpha
  swizzled to one) as `R8G8B8A8_UNORM`, RGB_565, BGRA_8888, RGBA_FP16,
  RGBA_1010102 and R_8. Buffers without a Metal format (YUV, RGB_888) are
  refused as invalid handles: there are no external formats (#316).

### Synchronization

The syscall layer has no sync files the host can signal yet, so fences cross
on the CPU (#315):

- `vkAcquireImageANDROID` waits for the dequeued buffer's fence, then
  signals the application's semaphore or fence with an empty submission on
  the device's first queue;
- `vkQueueSignalReleaseImageANDROID` submits a batch that waits for the
  loader's semaphores and waits on the CPU until it (and everything
  submitted before it) has run, then returns fence -1, so the buffer is
  queued finished;
- a sync-fd semaphore export waits the same way and returns -1 (an already
  signaled payload; the wait consumes the payload, as the copy transference
  of a sync fd does); an import waits for the fd and signals the semaphore.

The driver's own submissions take a per-queue lock that the application's
`vkQueueSubmit*`, `vkQueueWaitIdle` and `vkQueueBindSparse` also take, since
an acquire is not synchronized with the application's use of the queue.

## Conformance gaps

MoltenVK 1.4.2 on an M2 Pro reports Vulkan 1.4 (as high as the
application's instance asks for) and `VK_KHR_portability_subset`, which the
driver keeps: a device that is not fully conformant says so. Core features
it lacks: `geometryShader`, `logicOp`, `depthBounds`, `wideLines`,
`pipelineStatisticsQuery`, `shaderStorageImageMultisample`,
`shaderCullDistance`, `shaderFloat64`, `shaderResourceResidency`,
`variableMultisampleRate` and all sparse residency. The device therefore
declares Vulkan hardware level 0.

## Verified (2026-09-29, M2 Pro, heavily loaded host)

- `crates/aim-host-vulkan/tests/moltenvk.rs` (host only): all 272 table
  entries resolve in the pinned MoltenVK; an instance, its physical device
  (Apple M2 Pro, Vulkan 1.4.357) and their loader magic.
- `crates/aim-linux-abi/tests/vulkan.rs` builds
  `tests/fixtures/vulkan_triangle.c` with the NDK and runs it on the derived
  image with the original libvulkan, servicemanager and our allocator:
  - `vulkan_triangle info` (vulkaninfo-style): instance version 1.4.0; the
    device "Apple M2 Pro", Vulkan 1.3.357 for an application asking for 1.3,
    vendor 0x106b, integrated; 14 instance extensions (the loader's surface
    extensions and `VK_EXT_debug_report` among them) and 123 device
    extensions, `VK_KHR_swapchain`, the AHardwareBuffer, foreign queue
    family and sync-fd ones included;
  - one queue family with four queues, and a device with two of them, as
    HWUI makes it;
  - an instance and a device with those extensions; a red triangle on blue
    into a 64×64 image whose memory is an AHardwareBuffer, read back through
    the buffer's CPU mapping (corner, centre and apex pixels, so orientation
    too), in 1.5–10 ms for the submit and wait; the memory exported again is
    the same AHardwareBuffer; the same on the second queue, after a
    semaphore the first signals;
  - a sync-fd semaphore exported (fd -1) and imported into another that a
    submission then waits for.
- A full boot (`guest-init` as `cargo aim boot` runs it, first boot):
  `sys.boot_completed` after 49 s; `ro.hardware.vulkan=aim`;
  `pm list features` lists `android.hardware.vulkan.compute`, `.level`
  and `.version=4206592` (1.3); `dumpsys gpu` reports `vulkanVersion =
  4206592`.

Not verified yet:

- The swapchain mode of the program needs SurfaceFlinger (the loader asks
  it for the display's refresh period, `native_window_get_refresh_cycle_duration`),
  so it runs only in a booted guest (#317); that run was cut short by the
  host's security agent (#232).
- HWUI on Vulkan (`debug.hwui.renderer=skiavk`, for a test only) reaches
  `VulkanManager::initialize` and aborts: HWUI asks for two queues of the
  graphics family, and MoltenVK has one queue per family by design (it
  offers more families instead). Settings shows its splash screen only
  (#314).
