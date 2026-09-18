// Android gralloc storage owner and its narrow IOSurface/Metal provider.
// SurfaceControl sees only opaque AHardwareBuffer handles and accessors.
#include "hardware_buffer_owner.h"
#include "darwin_art_bionic_socket_broker.h"
#include <android/hardware_buffer.h>
#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#include <algorithm>
#include <atomic>
#include <cerrno>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <new>
#include <unordered_map>
#include <unistd.h>

struct DarwinAndroidNativeBaseAbi {
  int32_t magic = ('_' << 24) | ('b' << 16) | ('f' << 8) | 'r';
  int32_t version = 0;
  void* reserved[4]{};
  void (*inc_ref)(DarwinAndroidNativeBaseAbi*) = nullptr;
  void (*dec_ref)(DarwinAndroidNativeBaseAbi*) = nullptr;
};

struct DarwinAndroidNativeWindowBufferAbi {
  DarwinAndroidNativeBaseAbi common{};
  int32_t width = 0;
  int32_t height = 0;
  int32_t stride = 0;
  int32_t format = 0;
  int32_t usage_deprecated = 0;
  uintptr_t layer_count = 0;
  void* reserved[1]{};
  const void* handle = nullptr;
  uint64_t usage = 0;
  void* reserved_proc[7]{};
};

struct AHardwareBuffer {
  // Android implements AHardwareBuffer as a GraphicBuffer.  Its public EGL
  // client-buffer view is the embedded ANativeWindowBuffer at +0x10 on
  // arm64.  Native consumers such as Chromium's bundled ANGLE read this ABI
  // directly to determine dimensions, format and usage.
  void* graphic_buffer_prefix[2]{};
  DarwinAndroidNativeWindowBufferAbi native_buffer{};
  std::atomic<uint32_t> references{1};
  AHardwareBuffer_Desc description{};
  IOSurfaceRef surface = nullptr;
  std::mutex mutex;
  uint32_t locks = 0;
};

static_assert(offsetof(AHardwareBuffer, native_buffer) == 0x10);

static AHardwareBuffer* HardwareBufferFromNativeBase(
    DarwinAndroidNativeBaseAbi* base) {
  return base == nullptr
             ? nullptr
             : reinterpret_cast<AHardwareBuffer*>(
                   reinterpret_cast<char*>(base) -
                   offsetof(AHardwareBuffer, native_buffer));
}

static void HardwareBufferNativeIncRef(DarwinAndroidNativeBaseAbi* base) {
  AHardwareBuffer_acquire(HardwareBufferFromNativeBase(base));
}

static void HardwareBufferNativeDecRef(DarwinAndroidNativeBaseAbi* base) {
  AHardwareBuffer_release(HardwareBufferFromNativeBase(base));
}

namespace {
std::mutex g_hardware_buffer_mutex;
std::unordered_map<const void*, AHardwareBuffer*> g_hardware_buffer_aliases;

constexpr uint32_t kDarwinBgraPixelFormat =
    (static_cast<uint32_t>('B') << 24) |
    (static_cast<uint32_t>('G') << 16) |
    (static_cast<uint32_t>('R') << 8) | static_cast<uint32_t>('A');

uint32_t BytesPerPixel(uint32_t format) {
  switch (format) {
    case AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM:
    case AHARDWAREBUFFER_FORMAT_R8G8B8X8_UNORM:
      return 4;
    default:
      return 0;
  }
}

bool IsHardwareBufferDescriptionSupported(const AHardwareBuffer_Desc* desc) {
  if (desc == nullptr || desc->width == 0 || desc->height == 0 ||
      desc->layers != 1) {
    return false;
  }
  // The Darwin gralloc backend currently exposes one packed BGRA8 IOSurface
  // plane to Metal and EGL. Do not accept R8, YUV, depth, blob, or packed
  // 16-bit requests and silently allocate BGRA storage for them: Android
  // clients use allocation support to decide whether cross-thread SharedImage
  // and media paths are legal.
  return BytesPerPixel(desc->format) != 0;
}

AHardwareBuffer* WrapSurface(IOSurfaceRef surface,
                             const AHardwareBuffer_Desc& description) {
  if (surface == nullptr) return nullptr;
  auto* buffer = new (std::nothrow) AHardwareBuffer();
  if (buffer == nullptr) return nullptr;
  buffer->description = description;
  buffer->description.stride = static_cast<uint32_t>(
      IOSurfaceGetBytesPerRow(surface) / BytesPerPixel(description.format));
  buffer->native_buffer.common.version =
      sizeof(DarwinAndroidNativeWindowBufferAbi);
  buffer->native_buffer.common.inc_ref = &HardwareBufferNativeIncRef;
  buffer->native_buffer.common.dec_ref = &HardwareBufferNativeDecRef;
  buffer->native_buffer.width = static_cast<int>(description.width);
  buffer->native_buffer.height = static_cast<int>(description.height);
  buffer->native_buffer.stride = static_cast<int>(buffer->description.stride);
  buffer->native_buffer.format = static_cast<int>(description.format);
  buffer->native_buffer.usage_deprecated =
      static_cast<int>(description.usage & UINT32_MAX);
  buffer->native_buffer.layer_count = description.layers;
  buffer->native_buffer.handle = nullptr;
  buffer->native_buffer.usage = description.usage;
  buffer->surface = surface;
  {
    std::lock_guard<std::mutex> lock(g_hardware_buffer_mutex);
    g_hardware_buffer_aliases.emplace(buffer, buffer);
    // Android's EGL implementation exposes the embedded
    // ANativeWindowBuffer/GraphicBuffer client view rather than the owning
    // AHardwareBuffer address. ANGLE's host helper preserves that ABI offset.
    g_hardware_buffer_aliases.emplace(&buffer->native_buffer, buffer);
  }
  return buffer;
}

}  // namespace

extern "C" int AHardwareBuffer_allocate(const AHardwareBuffer_Desc* desc,
                                         AHardwareBuffer** out) {
  const bool trace = std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr;
  if (trace) {
    std::fprintf(stderr,
                 "ART Android AHardwareBuffer: allocate-request pid=%d desc=%p "
                 "out=%p size=%ux%u layers=%u format=%u usage=%llu\n",
                 static_cast<int>(getpid()), static_cast<const void*>(desc),
                 static_cast<void*>(out), desc ? desc->width : 0,
                 desc ? desc->height : 0, desc ? desc->layers : 0,
                 desc ? desc->format : 0,
                 static_cast<unsigned long long>(desc ? desc->usage : 0));
  }
  const auto finish = [trace](int result, AHardwareBuffer* buffer = nullptr,
                             IOSurfaceRef surface = nullptr) {
    if (trace) {
      std::fprintf(stderr,
                   "ART Android AHardwareBuffer: allocate-result pid=%d "
                   "result=%d buffer=%p surface=%p\n",
                   static_cast<int>(getpid()), result,
                   static_cast<void*>(buffer), static_cast<void*>(surface));
    }
    return result;
  };
  if (out == nullptr || !IsHardwareBufferDescriptionSupported(desc)) {
    return finish(-EINVAL);
  }
  *out = nullptr;
  const uint32_t bytes_per_pixel = BytesPerPixel(desc->format);
  const size_t packed_row_bytes =
      static_cast<size_t>(desc->width) * bytes_per_pixel;
  if (packed_row_bytes / bytes_per_pixel != desc->width ||
      packed_row_bytes > SIZE_MAX - 15) {
    return finish(-EOVERFLOW);
  }
  // IOSurface-backed Metal textures require a 16-byte-aligned row stride.
  // Android gralloc is likewise allowed to return a stride wider than the
  // requested pixel width, and clients discover it through
  // AHardwareBuffer_describe/ANativeWindowBuffer.  Keep the logical width
  // unchanged while allocating and reporting the aligned storage width.
  const size_t row_bytes = (packed_row_bytes + 15) & ~size_t{15};
  if (row_bytes / bytes_per_pixel > UINT32_MAX ||
      row_bytes > SIZE_MAX / desc->height) {
    return finish(-EOVERFLOW);
  }
  NSDictionary* properties = @{
    (__bridge NSString*)kIOSurfaceWidth : @(desc->width),
    (__bridge NSString*)kIOSurfaceHeight : @(desc->height),
    (__bridge NSString*)kIOSurfaceBytesPerElement : @(bytes_per_pixel),
    (__bridge NSString*)kIOSurfaceBytesPerRow : @(row_bytes),
    (__bridge NSString*)kIOSurfaceAllocSize : @(row_bytes * desc->height),
    (__bridge NSString*)kIOSurfacePixelFormat : @(kDarwinBgraPixelFormat),
    (__bridge NSString*)kIOSurfaceIsGlobal : @YES,
  };
  IOSurfaceRef surface = IOSurfaceCreate((__bridge CFDictionaryRef)properties);
  if (surface == nullptr) return finish(-ENOMEM);
  AHardwareBuffer* buffer = WrapSurface(surface, *desc);
  if (buffer == nullptr) {
    CFRelease(surface);
    return finish(-ENOMEM);
  }
  *out = buffer;
  return finish(0, buffer, surface);
}

extern "C" int AHardwareBuffer_isSupported(
    const AHardwareBuffer_Desc* desc) {
  return IsHardwareBufferDescriptionSupported(desc) ? 1 : 0;
}

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  if (buffer != nullptr) buffer->references.fetch_add(1, std::memory_order_relaxed);
}

extern "C" void* darwin_art_android_hardware_buffer_metal_texture(
    AHardwareBuffer* buffer, void* metal_device) {
  if (buffer == nullptr || buffer->surface == nullptr || metal_device == nullptr ||
      buffer->description.width == 0 || buffer->description.height == 0) {
    return nullptr;
  }
  return darwin_art_android_iosurface_metal_texture(
      buffer->surface, buffer->description.width, buffer->description.height,
      metal_device);
}

static void* CreateVulkanHardwareBufferTexture(
    AHardwareBuffer* buffer, void* metal_device, int32_t vk_format,
    bool importing_storage) {
  if (buffer == nullptr || buffer->surface == nullptr || metal_device == nullptr ||
      buffer->description.width == 0 || buffer->description.height == 0) {
    return nullptr;
  }
  id<MTLDevice> device = (__bridge id<MTLDevice>)metal_device;
  const uint32_t surface_width = IOSurfaceGetWidth(buffer->surface);
  const uint32_t surface_height = IOSurfaceGetHeight(buffer->surface);
  if (surface_width == 0 || surface_height == 0) return nullptr;
  if (vk_format != 37 && vk_format != 43) return nullptr;
  const MTLPixelFormat format = vk_format == 43 ? MTLPixelFormatRGBA8Unorm_sRGB
                                               : MTLPixelFormatRGBA8Unorm;
  MTLTextureDescriptor* descriptor =
      [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format
                                                         width:surface_width
                                                        height:surface_height
                                                     mipmapped:NO];
  descriptor.storageMode = MTLStorageModeShared;
  descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite |
                     MTLTextureUsageRenderTarget | MTLTextureUsagePixelFormatView;
  // This producer writes actual RGBA bytes. Preserve that storage contract
  // for consumers which otherwise assume the legacy HWUI BGRA storage.
  if (importing_storage) {
    IOSurfaceSetValue(buffer->surface, CFSTR("DarwinArtStorageRGBA"), kCFBooleanTrue);
    // Vulkan framebuffer coordinates and the imported Metal texture are
    // top-left based; unlike HWUI's Ganesh target they need no storage Y flip.
    IOSurfaceSetValue(buffer->surface, CFSTR("DarwinArtProducerTopLeft"), kCFBooleanTrue);
  }
  return reinterpret_cast<void*>(
      [device newTextureWithDescriptor:descriptor
                             iosurface:buffer->surface
                                 plane:0]);
}

extern "C" void* darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format(
    AHardwareBuffer* buffer, void* metal_device, int32_t vk_format) {
  return CreateVulkanHardwareBufferTexture(buffer, metal_device, vk_format, true);
}

extern "C" void* darwin_art_android_hardware_buffer_vulkan_metal_texture(
    AHardwareBuffer* buffer, void* metal_device) {
  // Capability queries must not change an existing producer's storage format.
  return CreateVulkanHardwareBufferTexture(buffer, metal_device, 37, false);
}

extern "C" void* darwin_art_android_iosurface_metal_texture(
    void* iosurface, uint32_t width, uint32_t height, void* metal_device) {
  if (iosurface == nullptr || metal_device == nullptr || width == 0 ||
      height == 0) {
    return nullptr;
  }
  id<MTLDevice> device = (__bridge id<MTLDevice>)metal_device;
  const uint32_t surface_width = IOSurfaceGetWidth((IOSurfaceRef)iosurface);
  const uint32_t surface_height = IOSurfaceGetHeight((IOSurfaceRef)iosurface);
  if (surface_width == 0 || surface_height == 0) return nullptr;
  CFTypeRef storage_rgba = IOSurfaceCopyValue((IOSurfaceRef)iosurface,
                                             CFSTR("DarwinArtStorageRGBA"));
  const bool rgba = storage_rgba != nullptr && CFEqual(storage_rgba, kCFBooleanTrue);
  if (storage_rgba != nullptr) CFRelease(storage_rgba);
  MTLTextureDescriptor* descriptor =
      [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                               (rgba ? MTLPixelFormatRGBA8Unorm : MTLPixelFormatBGRA8Unorm)
                                                         width:surface_width
                                                        height:surface_height
                                                     mipmapped:NO];
  descriptor.storageMode = MTLStorageModeShared;
  descriptor.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite |
                     MTLTextureUsageRenderTarget;
  id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor
                                                  iosurface:(IOSurfaceRef)iosurface
                                                      plane:0];
  // newTextureWithDescriptor returns a +1 object in this non-ARC translation
  // unit; transfer that ownership to the opaque C handle.
  return reinterpret_cast<void*>(texture);
}

extern "C" void darwin_art_android_metal_texture_release(void* texture) {
  if (texture != nullptr) CFRelease(texture);
}

extern "C" void* darwin_art_android_hardware_buffer_native_window_buffer(
    AHardwareBuffer* buffer) {
  return buffer == nullptr ? nullptr : &buffer->native_buffer;
}

extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  if (buffer != nullptr &&
      buffer->references.fetch_sub(1, std::memory_order_acq_rel) == 1) {
    {
      std::lock_guard<std::mutex> lock(g_hardware_buffer_mutex);
      std::erase_if(g_hardware_buffer_aliases,
                    [buffer](const auto& entry) {
                      return entry.second == buffer;
                    });
    }
    if (buffer->surface != nullptr) CFRelease(buffer->surface);
    delete buffer;
  }
}

extern "C" AHardwareBuffer*
darwin_art_android_hardware_buffer_from_client_buffer(void* client_buffer) {
  std::lock_guard<std::mutex> lock(g_hardware_buffer_mutex);
  auto found = g_hardware_buffer_aliases.find(client_buffer);
  return found == g_hardware_buffer_aliases.end() ? nullptr : found->second;
}

extern "C" void* darwin_art_android_hardware_buffer_iosurface(
    AHardwareBuffer* buffer) {
  if (std::getenv("DARWIN_ART_DEBUG_GRAPHICS_DSO") != nullptr) {
    std::fprintf(stderr,
                 "ART Android AHardwareBuffer: iosurface buffer=%p surface=%p\n",
                 static_cast<void*>(buffer),
                 buffer == nullptr ? nullptr : static_cast<void*>(buffer->surface));
  }
  return buffer == nullptr ? nullptr : buffer->surface;
}

extern "C" void AHardwareBuffer_describe(const AHardwareBuffer* buffer,
                                           AHardwareBuffer_Desc* out) {
  if (buffer != nullptr && out != nullptr) *out = buffer->description;
}

extern "C" int AHardwareBuffer_lock(AHardwareBuffer* buffer, uint64_t,
                                     int32_t, const ARect*, void** out) {
  if (buffer == nullptr || out == nullptr || buffer->surface == nullptr)
    return -EINVAL;
  std::lock_guard<std::mutex> lock(buffer->mutex);
  if (buffer->locks++ == 0 &&
      IOSurfaceLock(buffer->surface, 0, nullptr) != kIOReturnSuccess) {
    --buffer->locks;
    return -EIO;
  }
  *out = IOSurfaceGetBaseAddress(buffer->surface);
  return *out == nullptr ? -EIO : 0;
}

extern "C" int AHardwareBuffer_lockPlanes(AHardwareBuffer* buffer,
                                            uint64_t usage, int32_t fence,
                                            const ARect* rect,
                                            AHardwareBuffer_Planes* out) {
  if (out == nullptr) return -EINVAL;
  std::memset(out, 0, sizeof(*out));
  void* data = nullptr;
  const int result = AHardwareBuffer_lock(buffer, usage, fence, rect, &data);
  if (result != 0) return result;
  out->planeCount = 1;
  out->planes[0].data = data;
  out->planes[0].pixelStride = BytesPerPixel(buffer->description.format);
  out->planes[0].rowStride =
      static_cast<uint32_t>(IOSurfaceGetBytesPerRow(buffer->surface));
  return 0;
}

extern "C" int AHardwareBuffer_unlock(AHardwareBuffer* buffer,
                                       int32_t* fence) {
  if (buffer == nullptr || buffer->surface == nullptr) return -EINVAL;
  std::lock_guard<std::mutex> lock(buffer->mutex);
  if (buffer->locks == 0) return -EINVAL;
  if (--buffer->locks == 0 &&
      IOSurfaceUnlock(buffer->surface, 0, nullptr) != kIOReturnSuccess)
    return -EIO;
  if (fence != nullptr) *fence = -1;
  return 0;
}

struct HardwareBufferWire {
  uint32_t magic;
  uint32_t surface_id;
  AHardwareBuffer_Desc description;
};

extern "C" int AHardwareBuffer_sendHandleToUnixSocket(
    const AHardwareBuffer* buffer, int socket_fd) {
  if (buffer == nullptr || buffer->surface == nullptr) return -EINVAL;
  const HardwareBufferWire wire{0x44414842u, IOSurfaceGetID(buffer->surface),
                                buffer->description};
  const intptr_t written =
      darwin_art_bionic_socket_broker_write(socket_fd, &wire, sizeof(wire));
  return written == sizeof(wire) ? 0 : -EIO;
}

extern "C" int AHardwareBuffer_recvHandleFromUnixSocket(int socket_fd,
                                                          AHardwareBuffer** out) {
  if (out == nullptr) return -EINVAL;
  *out = nullptr;
  HardwareBufferWire wire{};
  const intptr_t read =
      darwin_art_bionic_socket_broker_read(socket_fd, &wire, sizeof(wire));
  if (read != sizeof(wire) || wire.magic != 0x44414842u) return -EIO;
  IOSurfaceRef surface = IOSurfaceLookup(wire.surface_id);
  if (surface == nullptr) return -ENOENT;
  *out = WrapSurface(surface, wire.description);
  if (*out == nullptr) {
    CFRelease(surface);
    return -ENOMEM;
  }
  return 0;
}
