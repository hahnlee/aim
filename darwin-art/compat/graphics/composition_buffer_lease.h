#pragma once

#include <android/hardware_buffer.h>

#include <cstdint>
#include <optional>

namespace darwin_art::graphics {

using AhbRetainProc = void (*)(AHardwareBuffer*);
using AhbReleaseProc = void (*)(AHardwareBuffer*);
using AhbDescribeProc = void (*)(const AHardwareBuffer*, AHardwareBuffer_Desc*);
using IosurfaceLookupProc = void* (*)(AHardwareBuffer*);

struct CompositionBufferLeaseOps {
  AhbRetainProc retain = nullptr;
  AhbReleaseProc release = nullptr;
  AhbDescribeProc describe = nullptr;
  IosurfaceLookupProc iosurface = nullptr;
};

// Acquiring a lease retains the borrowed input AHardwareBuffer exactly once.
// The copied description is immutable, and iosurface() is a borrowed view that
// remains valid only while this non-moved lease is alive. The callback table is
// consulted during acquisition, while its release callback is copied into the
// lease for its lifetime; the caller must therefore provide callbacks with
// process lifetime. The lease has no EGL-image or registry dependency.
class CompositionBufferLease {
 public:
  static std::optional<CompositionBufferLease> Acquire(
      AHardwareBuffer* buffer, const CompositionBufferLeaseOps& ops);

  CompositionBufferLease(const CompositionBufferLease&) = delete;
  CompositionBufferLease& operator=(const CompositionBufferLease&) = delete;
  CompositionBufferLease(CompositionBufferLease&& other) noexcept;
  CompositionBufferLease& operator=(CompositionBufferLease&& other) noexcept;
  ~CompositionBufferLease();

  AHardwareBuffer* buffer() const { return buffer_; }
  const AHardwareBuffer_Desc& description() const { return description_; }
  void* iosurface() const { return iosurface_; }

 private:
  CompositionBufferLease(AHardwareBuffer* buffer,
                         const AHardwareBuffer_Desc& description,
                         void* iosurface, AhbReleaseProc release)
      : buffer_(buffer), description_(description), iosurface_(iosurface),
        release_(release) {}

  void Reset();

  AHardwareBuffer* buffer_ = nullptr;
  AHardwareBuffer_Desc description_{};
  void* iosurface_ = nullptr;
  AhbReleaseProc release_ = nullptr;
};

}  // namespace darwin_art::graphics
