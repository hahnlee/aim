#pragma once

#include <cstdint>

namespace darwin_art::binder {

// Transport-independent identity for an admitted endpoint generation.  The
// owner is deliberately JNI-free: callers may retain this interface across
// routing and domain locks without performing endpoint or resource I/O.
class EndpointLifetime {
 public:
  virtual ~EndpointLifetime() = default;

  virtual uint64_t Generation() const noexcept = 0;
  virtual bool Live() const noexcept = 0;
  virtual bool Matches(uint64_t generation) const noexcept = 0;
};

}  // namespace darwin_art::binder
