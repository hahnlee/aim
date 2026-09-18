#include "compat/graphics/hardware_buffer_owner.h"
#include "darwin_art_bionic_socket_broker.h"
#include <android/hardware_buffer.h>
#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#include <cassert>
#include <cstddef>
#include <cstdio>
#include <cstring>
#include <vector>

// Only the socket transport is mocked. Allocation, alias lifetime, lock/stride,
// IOSurface import and Metal textures execute the actual production owner.
namespace {
std::vector<unsigned char> wire;
struct NativeBase {
  int32_t magic;
  int32_t version;
  void* reserved[4];
  void (*inc_ref)(NativeBase*);
  void (*dec_ref)(NativeBase*);
};
}
extern "C" intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, size_t count) {
  assert(fd == 123);
  const auto* data = static_cast<const unsigned char*>(bytes);
  wire.assign(data, data + count);
  return static_cast<intptr_t>(count);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_read(
    int fd, void* bytes, size_t count) {
  assert(fd == 123 && count == wire.size());
  std::memcpy(bytes, wire.data(), count);
  wire.clear();
  return static_cast<intptr_t>(count);
}

int main() {
  @autoreleasepool {
    AHardwareBuffer_Desc desc{};
    desc.width = 7;
    desc.height = 3;
    desc.layers = 1;
    desc.format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
    desc.usage = AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN |
                 AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN |
                 AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
                 AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT;
    assert(AHardwareBuffer_isSupported(&desc) == 1);
    auto unsupported = desc;
    unsupported.format = AHARDWAREBUFFER_FORMAT_Y8Cb8Cr8_420;
    assert(AHardwareBuffer_isSupported(&unsupported) == 0);
    AHardwareBuffer* buffer = nullptr;
    assert(AHardwareBuffer_allocate(&unsupported, &buffer) < 0);
    assert(buffer == nullptr);
    assert(AHardwareBuffer_allocate(&desc, &buffer) == 0 && buffer != nullptr);
    AHardwareBuffer_Desc actual{};
    AHardwareBuffer_describe(buffer, &actual);
    assert(actual.width == 7 && actual.height == 3 && actual.stride == 8);
    assert(actual.format == desc.format && actual.usage == desc.usage);
    auto surface = static_cast<IOSurfaceRef>(
        darwin_art_android_hardware_buffer_iosurface(buffer));
    assert(surface != nullptr && IOSurfaceGetWidth(surface) == 7);
    assert(IOSurfaceGetHeight(surface) == 3 && IOSurfaceGetBytesPerRow(surface) == 32);
    void* alias = darwin_art_android_hardware_buffer_native_window_buffer(buffer);
    assert(alias == reinterpret_cast<unsigned char*>(buffer) + 0x10);
    assert(darwin_art_android_hardware_buffer_from_client_buffer(alias) == buffer);
    assert(darwin_art_android_hardware_buffer_from_client_buffer(buffer) == buffer);
    assert(darwin_art_android_hardware_buffer_from_client_buffer(nullptr) == nullptr);
    auto* base = static_cast<NativeBase*>(alias);
    assert(base->inc_ref != nullptr && base->dec_ref != nullptr);
    base->inc_ref(base);
    base->dec_ref(base);

    AHardwareBuffer_Planes planes{};
    assert(AHardwareBuffer_lockPlanes(buffer, desc.usage, -1, nullptr, &planes) == 0);
    assert(planes.planeCount == 1 && planes.planes[0].pixelStride == 4);
    assert(planes.planes[0].rowStride == 32 && planes.planes[0].data != nullptr);
    std::memset(planes.planes[0].data, 0x5a, 96);
    void* nested = nullptr;
    assert(AHardwareBuffer_lock(buffer, desc.usage, -1, nullptr, &nested) == 0);
    assert(nested == planes.planes[0].data);
    int fence = -2;
    assert(AHardwareBuffer_unlock(buffer, &fence) == 0 && fence == -1);
    assert(AHardwareBuffer_unlock(buffer, &fence) == 0 && fence == -1);
    assert(AHardwareBuffer_unlock(buffer, &fence) < 0);

    assert(AHardwareBuffer_sendHandleToUnixSocket(buffer, 123) == 0);
    AHardwareBuffer* imported = nullptr;
    assert(AHardwareBuffer_recvHandleFromUnixSocket(123, &imported) == 0);
    assert(imported != nullptr && imported != buffer);
    AHardwareBuffer_describe(imported, &actual);
    assert(actual.width == 7 && actual.height == 3 && actual.stride == 8);
    auto imported_surface = static_cast<IOSurfaceRef>(
        darwin_art_android_hardware_buffer_iosurface(imported));
    assert(IOSurfaceGetID(imported_surface) == IOSurfaceGetID(surface));
    void* shared = nullptr;
    assert(AHardwareBuffer_lock(imported, desc.usage, -1, nullptr, &shared) == 0);
    assert(static_cast<unsigned char*>(shared)[0] == 0x5a);
    assert(AHardwareBuffer_unlock(imported, nullptr) == 0);
    AHardwareBuffer_release(imported);

    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    if (device != nil) {
      void* query = darwin_art_android_hardware_buffer_vulkan_metal_texture(
          buffer, (__bridge void*)device);
      assert(query != nullptr);
      CFTypeRef tag = IOSurfaceCopyValue(surface, CFSTR("DarwinArtStorageRGBA"));
      assert(tag == nullptr);  // A capability query must not retag BGRA storage.
      darwin_art_android_metal_texture_release(query);
      void* texture = darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format(
          buffer, (__bridge void*)device, 43);
      assert(texture != nullptr);
      assert(((__bridge id<MTLTexture>)texture).pixelFormat == MTLPixelFormatRGBA8Unorm_sRGB);
      tag = IOSurfaceCopyValue(surface, CFSTR("DarwinArtStorageRGBA"));
      assert(tag != nullptr && CFEqual(tag, kCFBooleanTrue));
      CFRelease(tag);
      darwin_art_android_metal_texture_release(texture);
      assert(darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format(
                 buffer, (__bridge void*)device, 124) == nullptr);
      [device release];
      std::puts("hardware-buffer-owner: real Metal texture views PASS");
    } else {
      std::puts("hardware-buffer-owner: Metal device unavailable; texture checks not run");
    }
    AHardwareBuffer_acquire(buffer);
    AHardwareBuffer_release(buffer);
    assert(darwin_art_android_hardware_buffer_from_client_buffer(alias) == buffer);
    AHardwareBuffer_release(buffer);
    assert(darwin_art_android_hardware_buffer_from_client_buffer(alias) == nullptr);
    assert(darwin_art_android_hardware_buffer_from_client_buffer(buffer) == nullptr);
    std::puts("hardware-buffer-owner: storage, ABI aliases, nested locks, import lifetime PASS");
  }
}
