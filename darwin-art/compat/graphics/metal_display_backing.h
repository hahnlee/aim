#pragma once

#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>

#include <cstddef>
#include <cstdint>

namespace darwin_art::graphics {

enum class MetalDisplayBackingResult : std::uint8_t {
  kOk,
  kInvalidArgument,
  kAllocationFailed,
  kMetalUnavailable,
};

// Owns one display-sized BGRA8 IOSurface and its shader-read Metal view.
// The IOSurface reference is +1 and the texture is strongly retained.  This
// owner contains no window, color, coordinate, or display policy.
class MetalDisplayBacking final {
 public:
  MetalDisplayBacking() = default;
  ~MetalDisplayBacking() noexcept;

  MetalDisplayBacking(const MetalDisplayBacking&) = delete;
  MetalDisplayBacking& operator=(const MetalDisplayBacking&) = delete;

  MetalDisplayBacking(MetalDisplayBacking&& other) noexcept;
  MetalDisplayBacking& operator=(MetalDisplayBacking&& other) noexcept;

  IOSurfaceRef surface() const noexcept { return surface_; }
  id<MTLTexture> texture() const noexcept { return texture_; }
  std::size_t bytes_per_row() const noexcept { return bytes_per_row_; }

  // Transfers this owner's +1 IOSurface reference to the caller.  The Metal
  // texture remains owned by this object and is released normally.
  // The recipient must retain the transferred surface while this texture is
  // alive; normal commit strongly copies texture() before transferring it.
  IOSurfaceRef ReleaseSurface() noexcept;

  void Reset() noexcept;

 private:
  MetalDisplayBacking(IOSurfaceRef surface, id<MTLTexture> texture,
                      std::size_t bytes_per_row) noexcept
      : surface_(surface), texture_(texture), bytes_per_row_(bytes_per_row) {}

  IOSurfaceRef surface_ = nullptr;
  id<MTLTexture> texture_ = nil;
  std::size_t bytes_per_row_ = 0;

  friend MetalDisplayBackingResult AllocateMetalDisplayBacking(
      id<MTLDevice> device, std::uint32_t width, std::uint32_t height,
      MetalDisplayBacking* out);
};

MetalDisplayBackingResult AllocateMetalDisplayBacking(
    id<MTLDevice> device, std::uint32_t width, std::uint32_t height,
    MetalDisplayBacking* out);

}  // namespace darwin_art::graphics
