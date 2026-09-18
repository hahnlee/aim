#pragma once

#include <cstdint>

struct AHardwareBuffer;

// Borrowed aliases/storage stay valid while the caller retains the owning
// AHardwareBuffer. ABI layout, alias registry and IOSurface release are private
// to hardware_buffer_owner.mm, never SurfaceControl or the Vulkan caller.
extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(
    AHardwareBuffer* buffer);
extern "C" AHardwareBuffer*
darwin_art_android_hardware_buffer_from_client_buffer(void* client_buffer);
extern "C" void* darwin_art_android_hardware_buffer_iosurface(
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
