#pragma once

#include "../darwin_surface_bridge.h"

#include <memory>

// Policy-free owner for one versioned sink binding. The surface provider
// stores this object in a shared_ptr and snapshots it before invoking a
// callback. Keeping the retain/release seam here makes its lifetime contract
// testable without linking AppKit or importing Android policy.
struct DarwinArtSurfaceInputSinkBinding {
  DarwinArtSurfaceInputSink sink{};

  explicit DarwinArtSurfaceInputSinkBinding(
      const DarwinArtSurfaceInputSink& source)
      : sink(source) {
    if (sink.context != nullptr && sink.retain_context != nullptr) {
      sink.retain_context(sink.context);
    }
  }

  ~DarwinArtSurfaceInputSinkBinding() {
    if (sink.context != nullptr && sink.release_context != nullptr) {
      sink.release_context(sink.context);
    }
  }

  DarwinArtSurfaceInputSinkBinding(
      const DarwinArtSurfaceInputSinkBinding&) = delete;
  DarwinArtSurfaceInputSinkBinding& operator=(
      const DarwinArtSurfaceInputSinkBinding&) = delete;
};

inline std::shared_ptr<DarwinArtSurfaceInputSinkBinding>
MakeDarwinArtSurfaceInputSinkBinding(const DarwinArtSurfaceInputSink& sink) {
  return std::make_shared<DarwinArtSurfaceInputSinkBinding>(sink);
}
