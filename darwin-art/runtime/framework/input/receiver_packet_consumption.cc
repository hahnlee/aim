#include "receiver_packet_consumption.h"

#include "channel_resources.h"
#include "packet_dispatch.h"
#include "receiver_jni_resources.h"
#include "view_root_input_jni.h"

namespace darwin_art::input {
InputTransportConsumptionResult ConsumeOriginalReceiverPacket(
    void* opaque, const InputRoutingRecipientHandle& original,
    const DarwinArtInputPacket& packet, ReceiverPacketOrigin origin,
    InputRoutingPacketLease* local_lease) {
  auto* context = static_cast<ReceiverPacketConsumptionContext*>(opaque);
  if (context == nullptr || context->env == nullptr ||
      context->env->ExceptionCheck())
    return InputTransportConsumptionResult::kDeferred;
  const auto& receiver = context->receiver;
  if (receiver == nullptr || original == nullptr || receiver->channel == nullptr ||
      receiver->disposed.load(std::memory_order_acquire) ||
      receiver->routing_recipient.lock() != original ||
      !IsInputRoutingRecipientCurrent(receiver->channel->Routing(), original))
    return InputTransportConsumptionResult::kConsumedStop;
  // A general InputEventReceiver is valid too; ViewRoot is not an admission
  // requirement for the receiver API. The JNI dispatch owner handles mode
  // changes only for the WindowInputEventReceiver where applicable.
  const OriginalInputReceiverDispatchContext dispatch{
      receiver, original, &DispatchFrameworkInputEventResultForReceiver,
      {origin, 0}, local_lease};
  const auto result = DispatchInputPacketToReceiver(context->env, dispatch, packet);
  if (result.rejected) return InputTransportConsumptionResult::kConsumed;
  if (!result.invoked) return InputTransportConsumptionResult::kDeferred;
  return context->env->ExceptionCheck() ||
                 receiver->disposed.load(std::memory_order_acquire)
             ? InputTransportConsumptionResult::kConsumedStop
             : InputTransportConsumptionResult::kConsumed;
}
}  // namespace darwin_art::input
