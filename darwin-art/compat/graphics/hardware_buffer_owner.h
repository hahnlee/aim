#pragma once

#include <cstdint>

struct AHardwareBuffer;

// Keep the transport shape independent of NDK headers: this provider header
// is also read by framework input/build closure TUs without NDK include paths.
struct DarwinArtHardwareBufferDescription {
  uint32_t width = 0;
  uint32_t height = 0;
  uint32_t layers = 0;
  uint32_t format = 0;
  uint64_t usage = 0;
  uint32_t stride = 0;
};

// A process-local IOSurface ID is a locator, not a transferable ownership
// right. The exporter must retain the source AHardwareBuffer until the remote
// importer acknowledges its own +1 IOSurfaceLookup reference.
struct DarwinArtHardwareBufferIdentity {
  uint32_t surface_id = 0;
  DarwinArtHardwareBufferDescription description{};
};
extern "C" int darwin_art_android_hardware_buffer_export_identity(
    const AHardwareBuffer* buffer, DarwinArtHardwareBufferIdentity* out);
extern "C" int darwin_art_android_hardware_buffer_import_identity(
    const DarwinArtHardwareBufferIdentity* identity, AHardwareBuffer** out);

// Borrowed aliases/storage stay valid while the caller retains the owning
// AHardwareBuffer. ABI layout, alias registry and IOSurface release are private
// to hardware_buffer_owner.mm, never SurfaceControl or the Vulkan caller.
extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(
    AHardwareBuffer* buffer);
extern "C" AHardwareBuffer*
darwin_art_android_hardware_buffer_from_client_buffer(void* client_buffer);
extern "C" void* darwin_art_android_hardware_buffer_iosurface(
    AHardwareBuffer* buffer);
// Marks software Canvas writes with the truthful RGBA/top-left storage
// metadata consumed by the Metal importer.
extern "C" void darwin_art_android_hardware_buffer_mark_cpu_rgba(
    AHardwareBuffer* buffer);

// Creates a +1 Metal texture over the exact retained buffer/IOSurface storage.
// The supplied device belongs to the actual EGL or Vulkan backend. No pixel
// copy or substitute allocation occurs at this provider boundary.
extern "C" void* darwin_art_android_hardware_buffer_metal_texture(
    AHardwareBuffer* buffer, void* metal_device);
extern "C" void* darwin_art_android_hardware_buffer_vulkan_metal_texture(
    AHardwareBuffer* buffer, void* metal_device);
extern "C" void* darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format(
    AHardwareBuffer* buffer, void* metal_device, int32_t vk_format);
extern "C" void* darwin_art_android_iosurface_metal_texture(
    void* iosurface, uint32_t width, uint32_t height, void* metal_device);
extern "C" void darwin_art_android_metal_texture_release(void* texture);
