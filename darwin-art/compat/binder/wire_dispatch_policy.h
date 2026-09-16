#pragma once

#include <cstdint>

namespace darwin_art::binder {

// UNKNOWN_TRANSACTION and a remote endpoint exception reject one Binder call;
// they do not tear down the process-wide Binder transport. The channel only
// fails when its acknowledgement cannot be delivered.
struct WireDispatchDecision {
  int32_t response_status;
  bool keep_channel;
};

constexpr WireDispatchDecision DecideWireDispatch(bool transaction_handled,
                                                   bool response_sent) {
  return {
      .response_status = transaction_handled ? 0 : -1,
      .keep_channel = response_sent,
  };
}

}  // namespace darwin_art::binder
