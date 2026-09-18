#pragma once

#include "input_routing.h"
#include "input_event_origin.h"
#include "input_transport_pump.h"

#include <cstddef>

namespace darwin_art::input {

// Invocation/rejection is synchronous, finishInputEvent is not. This port
// retains the original publication identity through event construction/JNI.
using ReceiverPacketConsumption = InputTransportConsumptionResult (*)(
    void*, const InputRoutingRecipientHandle&, const DarwinArtInputPacket&,
    ReceiverPacketOrigin, InputRoutingPacketLease*);

// One admitted owner-Looper turn. Owns packet/control ordering and its fair
// packet budget, not descriptors, JNI references, or eventual finish ACKs.
class ReceiverInputConsumer final {
 public:
  ReceiverInputConsumer(InputRoutingHandle routing,
                        InputRoutingRecipientHandle recipient,
                        ReceiverPacketConsumption consume, void* context,
                        size_t budget = 64);
  ReceiverInputConsumer(const ReceiverInputConsumer&) = delete;
  ReceiverInputConsumer& operator=(const ReceiverInputConsumer&) = delete;

  InputTransportConsumptionResult ConsumePacket(const DarwinArtInputPacket&);
  // Before a window/focus control, service earlier local routing packets.
  // Deferred means the control itself has NOT been invoked/consumed.
  InputTransportConsumptionResult BeforeControl();
  InputTransportConsumptionResult DrainLocal();
  size_t ConsumedPackets() const { return consumed_; }
  bool BudgetRetryNeeded() const { return budget_retry_; }
  bool CompletionFailed() const { return completion_failed_; }

 private:
  InputTransportConsumptionResult DrainLocalAdmitted();
  InputTransportConsumptionResult Invoke(
      const InputRoutingRecipientHandle&, const DarwinArtInputPacket&,
      ReceiverPacketOrigin, InputRoutingPacketLease* = nullptr);
  const InputRoutingHandle routing_;
  const InputRoutingRecipientHandle recipient_;
  const ReceiverPacketConsumption consume_;
  void* const context_;
  const size_t budget_;
  size_t consumed_ = 0;
  bool admitted_ = false;
  bool stopped_ = false;
  bool budget_retry_ = false;
  bool completion_failed_ = false;
};
}  // namespace darwin_art::input
