#pragma once

#include "endpoint_lifetime.h"

#include <atomic>
#include <cstdint>
#include <memory>

namespace darwin_art::binder {

class WireChannelLifetimeTestPeer;

// The admission identity retained by a wire endpoint's owner.  This object is
// deliberately independent of the endpoint transport: an endpoint may be
// replaced, but an admitted operation can only remain authorized while this
// exact generation is live.
class WireChannelLifetime final : public EndpointLifetime {
 public:
  // Returns null if the bounded generation space is exhausted or the owner
  // allocation fails.  A generation reserved before an allocation failure is
  // intentionally not reused.
  static std::shared_ptr<WireChannelLifetime> Create() noexcept;

  uint64_t Generation() const noexcept override { return generation_; }
  bool Live() const noexcept override;
  bool Matches(uint64_t generation) const noexcept override;

  // Returns true only for the call that changes the owner from live to sealed.
  // All later calls are harmless and return false.
  bool Seal() noexcept;

 private:
  explicit WireChannelLifetime(uint64_t generation) noexcept
      : generation_(generation), live_(true) {}

  // Test-only access is provided by a friend definition in the isolated test
  // translation unit.  No production/test reset or allocator hook is exposed.
  friend class WireChannelLifetimeTestPeer;

  static std::atomic<uint64_t> next_generation_;
  const uint64_t generation_;
  std::atomic<bool> live_;
};

}  // namespace darwin_art::binder
