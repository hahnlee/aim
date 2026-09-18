#pragma once

#include "iosurface_backing.h"
#include "output_lifetime_protocol.h"

#include <atomic>
#include <map>
#include <memory>
#include <mutex>
#include <utility>

namespace darwin_art::surfaceflinger {

// An admitted job shares this immutable storage/identity snapshot. Replacing
// or retiring an output invalidates admission without freeing an in-flight
// backing reference. It does not retire independent Android layers.
class OutputEpoch {
 public:
  bool current() const { return current_.load(std::memory_order_acquire); }
  const OutputRequest& identity() const { return identity_; }
  const std::shared_ptr<const IosurfaceBacking>& backing() const { return backing_; }

 private:
  friend class OutputRegistry;
  OutputEpoch(OutputRequest identity, std::shared_ptr<const IosurfaceBacking> backing)
      : identity_(identity), backing_(std::move(backing)) {}
  const OutputRequest identity_;
  const std::shared_ptr<const IosurfaceBacking> backing_;
  std::atomic<bool> current_{true};
};

struct OutputTransition {
  OutputResponse response{};
  std::shared_ptr<const OutputEpoch> previous;
  std::shared_ptr<const OutputEpoch> current;
};

// Output registration only, not a second layer/transaction manager. The
// accept-loop owns the descriptors; their numeric identities cannot be
// reused until Drop has invalidated the corresponding registration.
class OutputRegistry {
 public:
  static constexpr size_t kCapacity = 128;
  OutputRegistry();
  ~OutputRegistry();
  OutputTransition Apply(int connection, const OutputRequest& request);
  OutputTransition Drop(int connection);
  std::shared_ptr<const OutputEpoch> Admit(uint32_t iosurface_id) const;

 private:
  mutable std::mutex mutex_;
  uint64_t server_instance_ = 0;
  uint64_t next_serial_ = 1;
  std::map<int, std::shared_ptr<OutputEpoch>> owners_;
  std::map<uint32_t, std::shared_ptr<OutputEpoch>> by_surface_;
};

}  // namespace darwin_art::surfaceflinger
