#pragma once

#include "../darwin_surface_bridge.h"

#include <memory>

// Policy-free owner for one versioned sink binding. The surface provider
// stores this object in a shared_ptr and snapshots it before invoking a
// callback. Keeping the retain/release seam here makes its lifetime contract
// testable without linking AppKit or importing Android policy.
struct AimSurfaceInputSinkBinding {
  AimSurfaceInputSink sink{};

  explicit AimSurfaceInputSinkBinding(
      const AimSurfaceInputSink& source)
      : sink(source) {
    if (sink.context != nullptr && sink.retain_context != nullptr) {
      sink.retain_context(sink.context);
    }
  }

  ~AimSurfaceInputSinkBinding() {
    if (sink.context != nullptr && sink.release_context != nullptr) {
      sink.release_context(sink.context);
    }
  }

  AimSurfaceInputSinkBinding(
      const AimSurfaceInputSinkBinding&) = delete;
  AimSurfaceInputSinkBinding& operator=(
      const AimSurfaceInputSinkBinding&) = delete;
};

inline std::shared_ptr<AimSurfaceInputSinkBinding>
MakeAimSurfaceInputSinkBinding(const AimSurfaceInputSink& sink) {
  return std::make_shared<AimSurfaceInputSinkBinding>(sink);
}
