#include "surface_backing_owner.h"

#if !__has_feature(objc_arc)
#error "SurfaceBackingHandle requires ARC native resource ownership"
#endif

#include <utility>
#include <new>

namespace darwin_art::graphics {

namespace {

bool IsCoherent(const SurfaceBackingOwner::Handle& handle) noexcept {
  if (!handle || handle->surface() == nullptr || handle->texture() == nil ||
      handle->physical_width() == 0 || handle->physical_height() == 0 ||
      handle->logical_width() == 0 || handle->logical_height() == 0 ||
      handle->bytes_per_row() == 0 || handle->revision() == 0) {
    return false;
  }
  return IOSurfaceGetWidth(handle->surface()) == handle->physical_width() &&
      IOSurfaceGetHeight(handle->surface()) == handle->physical_height() &&
      IOSurfaceGetBytesPerRow(handle->surface()) == handle->bytes_per_row() &&
      handle->texture().iosurface == handle->surface() &&
      handle->texture().width == handle->physical_width() &&
      handle->texture().height == handle->physical_height();
}

}  // namespace

SurfaceBackingHandle::SurfaceBackingHandle(
    IOSurfaceRef surface, id<MTLTexture> texture, std::uint32_t physical_width,
    std::uint32_t physical_height, std::uint32_t logical_width,
    std::uint32_t logical_height, std::size_t bytes_per_row,
    std::uint64_t revision) noexcept
    : surface_(surface),
      texture_(texture),
      physical_width_(physical_width),
      physical_height_(physical_height),
      logical_width_(logical_width),
      logical_height_(logical_height),
      bytes_per_row_(bytes_per_row),
      revision_(revision) {
  if (surface_ != nullptr) {
    CFRetain(surface_);
  }
}

SurfaceBackingHandle::~SurfaceBackingHandle() noexcept {
  // Release the texture before the IOSurface it samples, matching the
  // dependency order used by MetalDisplayBacking.
  texture_ = nil;
  if (surface_ != nullptr) {
    CFRelease(surface_);
    surface_ = nullptr;
  }
}

SurfaceBackingOwner::~SurfaceBackingOwner() noexcept {
  Handle retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    closed_ = true;
    retired = std::move(current_);
  }
  retired.reset();
}

SurfaceBackingOwner::Handle SurfaceBackingOwner::Acquire() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return current_;
}

bool SurfaceBackingOwner::Publish(const Handle& expected, Handle replacement) {
  if (!IsCoherent(replacement)) {
    return false;
  }

  Handle retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_ || current_ != expected ||
        (expected != nullptr &&
         replacement->revision() <= expected->revision())) {
      return false;
    }
    retired = std::move(current_);
    current_ = std::move(replacement);
  }
  // Do not run the final shared-handle/resource release under mutex_.
  retired.reset();
  return true;
}

bool SurfaceBackingOwner::Close() noexcept {
  Handle retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_) {
      return false;
    }
    closed_ = true;
    retired = std::move(current_);
  }
  retired.reset();
  return true;
}

bool SurfaceBackingOwner::closed() const noexcept {
  std::lock_guard<std::mutex> lock(mutex_);
  return closed_;
}

SurfaceBackingOwner::Handle RetainSurfaceBacking(
    IOSurfaceRef surface, id<MTLTexture> texture, std::uint32_t physical_width,
    std::uint32_t physical_height, std::uint32_t logical_width,
    std::uint32_t logical_height, std::size_t bytes_per_row,
    std::uint64_t revision) noexcept {
  try {
    auto candidate = std::make_shared<const SurfaceBackingHandle>(surface,
        texture, physical_width, physical_height, logical_width, logical_height,
        bytes_per_row, revision);
    return IsCoherent(candidate) ? candidate : nullptr;
  } catch (const std::bad_alloc&) {
    return nullptr;
  }
}

}  // namespace darwin_art::graphics
