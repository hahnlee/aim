#pragma once

#include "receiver_input_consumer.h"
#include "receiver_registry.h"

#include <jni.h>

namespace darwin_art::input {
struct ReceiverPacketConsumptionContext final {
  JNIEnv* env;
  std::shared_ptr<InputReceiver> receiver;
};
// Production packet-to-original-InputEventReceiver invocation port. No
// routing FIFO mutation, descriptor handling, or finish/ACK policy here.
InputTransportConsumptionResult ConsumeOriginalReceiverPacket(
    void*, const InputRoutingRecipientHandle&, const DarwinArtInputPacket&,
    ReceiverPacketOrigin, InputRoutingPacketLease*);
}  // namespace darwin_art::input
