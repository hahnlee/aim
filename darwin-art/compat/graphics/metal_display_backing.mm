#include "metal_display_backing.h"

#if !__has_feature(objc_arc)
#error "MetalDisplayBacking requires ARC native resource ownership"
#endif

#import <Foundation/Foundation.h>

#include <limits>
#include <utility>

namespace darwin_art::graphics {
namespace {

constexpr std::uint32_t kBytesPerPixel = 4;
constexpr std::size_t kRowAlignment = 64;
constexpr std::uint32_t kMaximumDimension = 16384;
constexpr std::uint32_t kBgraPixelFormat =
    (static_cast<std::uint32_t>('B') << 24) |
    (static_cast<std::uint32_t>('G') << 16) |
    (static_cast<std::uint32_t>('R') << 8) |
    static_cast<std::uint32_t>('A');

bool IsValidDimension(std::uint32_t value) {
  return value != 0 && value <= kMaximumDimension;
}

std::size_t AlignRowBytes(std::size_t value) {
  return (value + kRowAlignment - 1) & ~(kRowAlignment - 1);
}

}  // namespace

MetalDisplayBacking::~MetalDisplayBacking() noexcept { Reset(); }

MetalDisplayBacking::MetalDisplayBacking(MetalDisplayBacking&& other) noexcept
    : surface_(other.surface_),
      texture_(other.texture_),
      bytes_per_row_(other.bytes_per_row_) {
  other.surface_ = nullptr;
  other.texture_ = nil;
  other.bytes_per_row_ = 0;
}

MetalDisplayBacking& MetalDisplayBacking::operator=(
    MetalDisplayBacking&& other) noexcept {
  if (this == &other) return *this;
  Reset();
  surface_ = other.surface_;
  texture_ = other.texture_;
  bytes_per_row_ = other.bytes_per_row_;
  other.surface_ = nullptr;
  other.texture_ = nil;
  other.bytes_per_row_ = 0;
  return *this;
}

IOSurfaceRef MetalDisplayBacking::ReleaseSurface() noexcept {
  return std::exchange(surface_, nullptr);
}

void MetalDisplayBacking::Reset() noexcept {
  // Release the Objective-C object before its backing IOSurface, matching the
  // texture's dependency on the IOSurface while both are owned here.
  texture_ = nil;
  if (surface_ != nullptr) {
    CFRelease(surface_);
    surface_ = nullptr;
  }
  bytes_per_row_ = 0;
}

MetalDisplayBackingResult AllocateMetalDisplayBacking(
    id<MTLDevice> device, std::uint32_t width, std::uint32_t height,
    MetalDisplayBacking* out) {
  if (device == nil || out == nullptr || !IsValidDimension(width) ||
      !IsValidDimension(height)) {
    return MetalDisplayBackingResult::kInvalidArgument;
  }

  const std::size_t minimum_row_bytes =
      static_cast<std::size_t>(width) * kBytesPerPixel;
  if (minimum_row_bytes > std::numeric_limits<std::size_t>::max() -
                              (kRowAlignment - 1)) {
    return MetalDisplayBackingResult::kAllocationFailed;
  }
  const std::size_t requested_row_bytes = AlignRowBytes(minimum_row_bytes);
  if (requested_row_bytes >
      std::numeric_limits<std::size_t>::max() / height) {
    return MetalDisplayBackingResult::kAllocationFailed;
  }

  @autoreleasepool {
    NSDictionary* properties = @{
      (__bridge NSString*)kIOSurfaceWidth : @(width),
      (__bridge NSString*)kIOSurfaceHeight : @(height),
      (__bridge NSString*)kIOSurfaceBytesPerElement : @(kBytesPerPixel),
      (__bridge NSString*)kIOSurfaceBytesPerRow : @(requested_row_bytes),
      (__bridge NSString*)kIOSurfacePixelFormat : @(kBgraPixelFormat),
      (__bridge NSString*)kIOSurfaceIsGlobal : @YES,
    };
    IOSurfaceRef surface =
        IOSurfaceCreate((__bridge CFDictionaryRef)properties);
    if (surface == nullptr)
      return MetalDisplayBackingResult::kAllocationFailed;
    // Own the CF +1 before any validation or texture allocation can fail.
    MetalDisplayBacking candidate(surface, nil, 0);

    const std::size_t actual_bytes_per_row = IOSurfaceGetBytesPerRow(surface);
    const std::size_t allocation_size = IOSurfaceGetAllocSize(surface);
    if (actual_bytes_per_row < minimum_row_bytes ||
        actual_bytes_per_row >
            std::numeric_limits<std::size_t>::max() / height ||
        allocation_size < actual_bytes_per_row * height) {
      return MetalDisplayBackingResult::kAllocationFailed;
    }
    const std::size_t surface_width = IOSurfaceGetWidth(surface);
    const std::size_t surface_height = IOSurfaceGetHeight(surface);
    const std::size_t surface_bytes_per_element =
        IOSurfaceGetBytesPerElement(surface);
    if (surface_width == 0 || surface_height == 0 ||
        surface_width != static_cast<std::size_t>(width) ||
        surface_height != static_cast<std::size_t>(height) ||
        surface_bytes_per_element != kBytesPerPixel ||
        static_cast<std::uint32_t>(IOSurfaceGetPixelFormat(surface)) !=
            kBgraPixelFormat) {
      return MetalDisplayBackingResult::kAllocationFailed;
    }

    MTLTextureDescriptor* descriptor =
        [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                                 MTLPixelFormatBGRA8Unorm
                                                         width:surface_width
                                                        height:surface_height
                                                     mipmapped:NO];
    descriptor.storageMode = MTLStorageModeShared;
    descriptor.usage = MTLTextureUsageShaderRead;
    id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor
                                                      iosurface:surface
                                                          plane:0];
    if (texture == nil) {
      return MetalDisplayBackingResult::kMetalUnavailable;
    }

    // Keep all native resources in a temporary owner until every validation
    // and Metal operation succeeds.  Assignment is the commit that replaces
    // the caller's prior backing; failures above leave *out untouched.
    candidate.texture_ = texture;
    candidate.bytes_per_row_ = actual_bytes_per_row;
    *out = std::move(candidate);
    return MetalDisplayBackingResult::kOk;
  }
}

}  // namespace darwin_art::graphics
