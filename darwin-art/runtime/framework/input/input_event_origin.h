#pragma once

#include <cstdint>

namespace darwin_art::input {

// The Android consumer distinguishes host-routed packets from packets owned
// by an imported InputChannel publisher. Only the latter owes a wire ACK.
enum class ReceiverPacketOrigin { kLocalQueue, kImportedChannel };

struct InputEventOrigin final {
  ReceiverPacketOrigin kind = ReceiverPacketOrigin::kLocalQueue;
  uint64_t sequence = 0;
};

}  // namespace darwin_art::input
