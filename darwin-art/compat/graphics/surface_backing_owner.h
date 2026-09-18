#pragma once

#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>

namespace darwin_art::graphics {

// An immutable retained tuple shared by the AppKit owner and producer
// snapshots.  This type owns only the native backing resources; allocation,
// IPC, scanout, and Ganesh policy remain outside this boundary.
class SurfaceBackingHandle final {
 public:
  // The constructor borrows both native objects and retains them for the
  // lifetime of the handle.  Callers must provide a coherent physical extent
  // and stride; Publish validates those facts before making a handle visible.
  SurfaceBackingHandle(IOSurfaceRef surface, id<MTLTexture> texture,
                       std::uint32_t physical_width,
                       std::uint32_t physical_height,
                       std::uint32_t logical_width,
                       std::uint32_t logical_height, std::size_t bytes_per_row,
                       std::uint64_t revision) noexcept;
  ~SurfaceBackingHandle() noexcept;

  SurfaceBackingHandle(const SurfaceBackingHandle&) = delete;
  SurfaceBackingHandle& operator=(const SurfaceBackingHandle&) = delete;

  IOSurfaceRef surface() const noexcept { return surface_; }
  id<MTLTexture> texture() const noexcept { return texture_; }
  std::uint32_t physical_width() const noexcept { return physical_width_; }
  std::uint32_t physical_height() const noexcept { return physical_height_; }
  std::uint32_t logical_width() const noexcept { return logical_width_; }
  std::uint32_t logical_height() const noexcept { return logical_height_; }
  std::size_t bytes_per_row() const noexcept { return bytes_per_row_; }
  std::uint64_t revision() const noexcept { return revision_; }

 private:
  IOSurfaceRef surface_ = nullptr;
  id<MTLTexture> texture_ = nil;
  std::uint32_t physical_width_ = 0;
  std::uint32_t physical_height_ = 0;
  std::uint32_t logical_width_ = 0;
  std::uint32_t logical_height_ = 0;
  std::size_t bytes_per_row_ = 0;
  std::uint64_t revision_ = 0;
};

class SurfaceBackingOwner final {
 public:
  using Handle = std::shared_ptr<const SurfaceBackingHandle>;

  SurfaceBackingOwner() = default;
  ~SurfaceBackingOwner() noexcept;

  SurfaceBackingOwner(const SurfaceBackingOwner&) = delete;
  SurfaceBackingOwner& operator=(const SurfaceBackingOwner&) = delete;

  // Returns a retained immutable snapshot.  The returned handle may outlive
  // this owner and is safe for producer-side metadata reads.
  Handle Acquire() const;

  // Publishes replacement only when expected is the exact currently
  // published handle (or null for the first publication).  A replacement
  // must advance the backing revision.  Rejected candidates remain owned by
  // the caller and are released after this method returns.
  bool Publish(const Handle& expected, Handle replacement);

  // Closes permanently.  The current handle is retired after unlocking;
  // subsequent Publish calls are rejected even if a caller still holds an
  // older snapshot.
  bool Close() noexcept;

  bool closed() const noexcept;

 private:
  mutable std::mutex mutex_;
  Handle current_;
  bool closed_ = false;
};

// Construct and validate before any external output transaction. Allocation
// failure or incoherent native resources leaves the caller's publication intact.
SurfaceBackingOwner::Handle RetainSurfaceBacking(
    IOSurfaceRef surface, id<MTLTexture> texture, std::uint32_t physical_width,
    std::uint32_t physical_height, std::uint32_t logical_width,
    std::uint32_t logical_height, std::size_t bytes_per_row,
    std::uint64_t revision) noexcept;

}  // namespace darwin_art::graphics
