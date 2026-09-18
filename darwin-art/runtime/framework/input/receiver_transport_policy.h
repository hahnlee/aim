#pragma once

#include "input_routing.h"
#include "input_transport_pump.h"
#include <memory>
#include <jni.h>

namespace darwin_art::input {
class InputChannelResources;
struct InputReceiver;
class ReceiverInputConsumer;

// Borrowed synchronous pump context. Resources and the original publication
// token are pinned through the owner's admitted Looper callback, never rebuilt
// from the channel's current numeric receiver ID.
struct ReceiverTransportPolicy final {
  std::shared_ptr<InputChannelResources> channel;
  InputRoutingRecipientHandle recipient;
  void (*wake_pending)() = nullptr;
  JNIEnv* env = nullptr;
  std::shared_ptr<InputReceiver> receiver;
  ReceiverInputConsumer* consumer = nullptr;
};
InputTransportPumpCallbacks ReceiverTransportCallbacks(ReceiverTransportPolicy* policy);
// Remote HANGUP may accompany the final readable FIFO prefix. Local wake
// descriptor loss and genuine ERROR/INVALID are not ordinary stream EOF.
bool CanDrainReceiverTransport(const std::shared_ptr<InputChannelResources>& channel,
                              int fd, int events);
}  // namespace darwin_art::input
