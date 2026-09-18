#pragma once

#import <Metal/Metal.h>

#include "graphics/surface_backing_owner.h"

#include <cstddef>
#include <cstdint>
#include <string>
#include <utility>

namespace darwin_art::graphics {

// Owns one optional readback job for the diagnostic scanout artifact. The
// normal presentation path never allocates this object's GPU buffer unless
// DARWIN_ART_DIAGNOSTIC_FRAME_PREFIX is enabled and the process-wide throttle
// admits a frame. It contains no window, Android, or scanout policy.
class ScanoutDiagnosticCapture final {
 public:
  ScanoutDiagnosticCapture() noexcept = default;
  ~ScanoutDiagnosticCapture() noexcept = default;

  ScanoutDiagnosticCapture(const ScanoutDiagnosticCapture&) = delete;
  ScanoutDiagnosticCapture& operator=(const ScanoutDiagnosticCapture&) = delete;
  ScanoutDiagnosticCapture(ScanoutDiagnosticCapture&& other) noexcept;
  ScanoutDiagnosticCapture& operator=(ScanoutDiagnosticCapture&&) = delete;

  // Copies the exact physical backing texture into a shared readback buffer
  // on encoder. The returned job keeps backing alive until Complete().
  static ScanoutDiagnosticCapture Encode(
      SurfaceBackingOwner::Handle backing, id<MTLDevice> device,
      id<MTLBlitCommandEncoder> encoder) noexcept;

  bool valid() const noexcept { return pixels_ != nil; }

  // Must be called after the command buffer is committed. Only an enabled
  // job waits for completion and emits the existing PNG/log artifact.
  void Complete(id<MTLCommandBuffer> command_buffer) noexcept;

 private:
  ScanoutDiagnosticCapture(SurfaceBackingOwner::Handle backing,
                           id<MTLBuffer> pixels, std::size_t stride,
                           std::uint32_t width, std::uint32_t height,
                           std::string prefix, std::uint64_t sequence) noexcept
      : backing_(std::move(backing)),
        pixels_(pixels),
        stride_(stride),
        width_(width),
        height_(height),
        prefix_(std::move(prefix)),
        sequence_(sequence) {}

  SurfaceBackingOwner::Handle backing_;
  id<MTLBuffer> pixels_ = nil;
  std::size_t stride_ = 0;
  std::uint32_t width_ = 0;
  std::uint32_t height_ = 0;
  std::string prefix_;
  std::uint64_t sequence_ = 0;
};

}  // namespace darwin_art::graphics
