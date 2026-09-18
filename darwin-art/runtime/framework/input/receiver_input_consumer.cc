#include "receiver_input_consumer.h"

#include "input_routing_packet_lease.h"

#include <stdexcept>
#include <utility>

namespace darwin_art::input {
namespace {
class Admission final {
 public:
  explicit Admission(bool& admitted) : admitted_(admitted) { admitted_ = true; }
  ~Admission() { admitted_ = false; }
 private:
  bool& admitted_;
};
}  // namespace

ReceiverInputConsumer::ReceiverInputConsumer(
    InputRoutingHandle routing, InputRoutingRecipientHandle recipient,
    ReceiverPacketConsumption consume, void* context, size_t budget)
    : routing_(std::move(routing)), recipient_(std::move(recipient)),
      consume_(consume), context_(context), budget_(budget) {}

InputTransportConsumptionResult ReceiverInputConsumer::Invoke(
    const InputRoutingRecipientHandle& recipient,
    const DarwinArtInputPacket& packet, ReceiverPacketOrigin origin,
    InputRoutingPacketLease* local_lease) {
  if (stopped_ || consume_ == nullptr)
    return InputTransportConsumptionResult::kDeferred;
  if (consumed_ >= budget_) {
    budget_retry_ = true;
    return InputTransportConsumptionResult::kDeferred;
  }
  const auto result = consume_(context_, recipient, packet, origin, local_lease);
  switch (result) {
    case InputTransportConsumptionResult::kDeferred:
      return result;
    case InputTransportConsumptionResult::kConsumedStop:
      stopped_ = true;
      [[fallthrough]];
    case InputTransportConsumptionResult::kConsumed:
      ++consumed_;
      return result;
  }
  throw std::invalid_argument("invalid receiver packet consumption result");
}

InputTransportConsumptionResult ReceiverInputConsumer::DrainLocalAdmitted() {
  if (routing_ == nullptr || recipient_ == nullptr || stopped_)
    return InputTransportConsumptionResult::kDeferred;
  if (!IsInputRoutingRecipientCurrent(routing_, recipient_)) {
    stopped_ = true;
    return InputTransportConsumptionResult::kConsumedStop;
  }
  while (HasInputRoutingPackets(routing_)) {
    InputRoutingPacketLease head;
    if (!AcquireInputRoutingPacketLease(routing_, recipient_, &head))
      return InputTransportConsumptionResult::kDeferred;
    const auto result = Invoke(head.Recipient(), *head.Packet(),
                               ReceiverPacketOrigin::kLocalQueue, &head);
    if (result == InputTransportConsumptionResult::kDeferred) return result;
    if (!head.Complete()) {
      // Invocation cannot be retried. The callback owner must retire the
      // broken consumer rather than permitting duplicate Java delivery.
      completion_failed_ = stopped_ = true;
      return InputTransportConsumptionResult::kDeferred;
    }
    if (result == InputTransportConsumptionResult::kConsumedStop)
      return InputTransportConsumptionResult::kDeferred;
  }
  if (!IsInputRoutingRecipientCurrent(routing_, recipient_)) {
    stopped_ = true;
    return InputTransportConsumptionResult::kConsumedStop;
  }
  return InputTransportConsumptionResult::kConsumed;
}

InputTransportConsumptionResult ReceiverInputConsumer::DrainLocal() {
  if (admitted_) return InputTransportConsumptionResult::kDeferred;
  Admission admission(admitted_);
  return DrainLocalAdmitted();
}

InputTransportConsumptionResult ReceiverInputConsumer::BeforeControl() {
  return DrainLocal();
}

InputTransportConsumptionResult ReceiverInputConsumer::ConsumePacket(
    const DarwinArtInputPacket& packet) {
  if (admitted_) return InputTransportConsumptionResult::kDeferred;
  Admission admission(admitted_);
  const auto local = DrainLocalAdmitted();
  if (local != InputTransportConsumptionResult::kConsumed) return local;
  return Invoke(recipient_, packet, ReceiverPacketOrigin::kImportedChannel);
}
}  // namespace darwin_art::input
