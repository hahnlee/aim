#include "wire_channel_lifetime.h"

#include <limits>

namespace darwin_art::binder {

std::atomic<uint64_t> WireChannelLifetime::next_generation_{1};

std::shared_ptr<WireChannelLifetime> WireChannelLifetime::Create() noexcept {
  constexpr uint64_t kMaxGeneration = std::numeric_limits<uint64_t>::max();

  // Reserve generations with a CAS, including the terminal max value as an
  // exhaustion marker.  Valid identities are [1, max-1]; after the marker is
  // observed the allocator is permanently closed and never wraps.
  uint64_t generation = next_generation_.load(std::memory_order_relaxed);
  for (;;) {
    if (generation >= kMaxGeneration) {
      return nullptr;
    }
    if (next_generation_.compare_exchange_weak(
            generation, generation + 1, std::memory_order_relaxed,
            std::memory_order_relaxed)) {
      break;
    }
  }

  try {
    // Construct through this member so the private constructor remains
    // private; make_shared's allocator machinery is not a friend.
    return std::shared_ptr<WireChannelLifetime>(new WireChannelLifetime(generation));
  } catch (...) {
    // The reservation is intentionally consumed.  Reusing it could authorize
    // an operation admitted against an owner whose allocation failed.
    return nullptr;
  }
}

bool WireChannelLifetime::Live() const noexcept {
  return live_.load(std::memory_order_acquire);
}

bool WireChannelLifetime::Matches(uint64_t generation) const noexcept {
  return generation_ == generation &&
         live_.load(std::memory_order_acquire);
}

bool WireChannelLifetime::Seal() noexcept {
  return live_.exchange(false, std::memory_order_acq_rel);
}

}  // namespace darwin_art::binder
