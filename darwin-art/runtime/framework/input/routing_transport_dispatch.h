#pragma once

#include "input_routing.h"
#include <cstddef>

namespace darwin_art::input {
struct RoutingTransportDrainResult {
  size_t settled = 0;
  bool remote_admitted = false;
  bool local_queued = false;
  bool backpressured = false;
  bool terminal = false;
  bool continuation_needed = false;
};

struct RoutingTransportSubmitResult {
  DarwinArtInputEnqueueResult result =
      DarwinArtInputEnqueueResult::kNoFocusedChannel;
  bool wake_local = false;
  bool refresh_writable = false;
  bool continuation_needed = false;
};
// Executes only routing-owned FIFO heads. No JNI, channel registry or looper
// policy lives here; the caller refreshes interest and wakes local delivery.
RoutingTransportDrainResult DrainInputRoutingTransport(
    const InputRoutingHandle& routing, size_t budget = 256);

// Reserve and submit one already-routed packet through the same FIFO head
// machinery used by writable retries. The admission is moved exactly once;
// all status decisions use the immutable lease snapshot, never moved fields.
RoutingTransportSubmitResult SubmitInputRoutingAdmission(
    InputRoutingAdmission&& admission);
}  // namespace darwin_art::input
