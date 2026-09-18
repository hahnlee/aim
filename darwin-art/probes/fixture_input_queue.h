#pragma once

#include "darwin_surface_bridge.h"

#include <cstddef>
#include <deque>
#include <mutex>

namespace darwin_art_graphics_fixture {

struct FixtureQueuedInput {
  enum class Kind : uint32_t { kPointer, kKey };
  Kind kind = Kind::kPointer;
  DarwinArtPointerEventV2 pointer{};
  DarwinArtKeyEventV1 key{};
};

// Fixture-only queue. Production never polls this object: it submits directly
// to Android ingress. The queue owns boundedness, MOVE coalescing, and its
// pending edge acknowledgement.
class FixtureInputQueue {
 public:
  static constexpr size_t kMaximumPackets = 256;

  DarwinArtSurfaceInputResult enqueue_pointer(
      const DarwinArtPointerEventV2& event) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (packets_.size() >= kMaximumPackets) {
      if (event.action == DARWIN_ART_POINTER_MOVE && !packets_.empty() &&
          packets_.back().kind == FixtureQueuedInput::Kind::kPointer &&
          packets_.back().pointer.action == DARWIN_ART_POINTER_MOVE) {
        packets_.back().pointer = event;
        pending_ = true;
        return DARWIN_ART_SURFACE_INPUT_QUEUED;
      }
      return DARWIN_ART_SURFACE_INPUT_BACKPRESSURED;
    }
    FixtureQueuedInput packet;
    packet.kind = FixtureQueuedInput::Kind::kPointer;
    packet.pointer = event;
    packets_.push_back(packet);
    pending_ = true;
    return DARWIN_ART_SURFACE_INPUT_QUEUED;
  }

  DarwinArtSurfaceInputResult enqueue_key(const DarwinArtKeyEventV1& event) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (packets_.size() >= kMaximumPackets) {
      return DARWIN_ART_SURFACE_INPUT_BACKPRESSURED;
    }
    FixtureQueuedInput packet;
    packet.kind = FixtureQueuedInput::Kind::kKey;
    packet.key = event;
    packets_.push_back(packet);
    pending_ = true;
    return DARWIN_ART_SURFACE_INPUT_QUEUED;
  }

  bool take(FixtureQueuedInput* packet) {
    if (packet == nullptr) return false;
    std::lock_guard<std::mutex> lock(mutex_);
    if (packets_.empty()) {
      pending_ = false;
      return false;
    }
    *packet = packets_.front();
    packets_.pop_front();
    return true;
  }

  bool acknowledge_if_empty() {
    std::lock_guard<std::mutex> lock(mutex_);
    if (!packets_.empty()) return false;
    pending_ = false;
    return true;
  }

  bool pending() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return pending_;
  }

 private:
  mutable std::mutex mutex_;
  std::deque<FixtureQueuedInput> packets_;
  bool pending_ = false;
};

}  // namespace darwin_art_graphics_fixture
