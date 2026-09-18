#include "composition_buffer_lease.h"

#include <utility>

namespace darwin_art::graphics {

std::optional<CompositionBufferLease> CompositionBufferLease::Acquire(
    AHardwareBuffer* buffer, const CompositionBufferLeaseOps& ops) {
  if (buffer == nullptr || ops.retain == nullptr || ops.release == nullptr ||
      ops.describe == nullptr || ops.iosurface == nullptr) {
    return std::nullopt;
  }
  ops.retain(buffer);
  AHardwareBuffer_Desc description{};
  ops.describe(buffer, &description);
  void* iosurface = ops.iosurface(buffer);
  if (description.width == 0 || description.height == 0 ||
      iosurface == nullptr) {
    ops.release(buffer);
    return std::nullopt;
  }
  return CompositionBufferLease(buffer, description, iosurface, ops.release);
}

CompositionBufferLease::CompositionBufferLease(
    CompositionBufferLease&& other) noexcept
    : buffer_(other.buffer_),
      description_(other.description_),
      iosurface_(other.iosurface_),
      release_(other.release_) {
  other.buffer_ = nullptr;
  other.iosurface_ = nullptr;
  other.release_ = nullptr;
}

CompositionBufferLease& CompositionBufferLease::operator=(
    CompositionBufferLease&& other) noexcept {
  if (this == &other) return *this;
  Reset();
  buffer_ = other.buffer_;
  description_ = other.description_;
  iosurface_ = other.iosurface_;
  release_ = other.release_;
  other.buffer_ = nullptr;
  other.iosurface_ = nullptr;
  other.release_ = nullptr;
  return *this;
}

CompositionBufferLease::~CompositionBufferLease() { Reset(); }

void CompositionBufferLease::Reset() {
  if (buffer_ != nullptr && release_ != nullptr) release_(buffer_);
  buffer_ = nullptr;
  iosurface_ = nullptr;
  release_ = nullptr;
}

}  // namespace darwin_art::graphics
