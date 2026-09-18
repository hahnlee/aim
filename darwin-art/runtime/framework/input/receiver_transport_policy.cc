#include "receiver_transport_policy.h"
#include "channel_resources.h"
#include "channel_endpoint.h"
#include "receiver_focus_control.h"
#include "receiver_input_consumer.h"
#include "input_channel_jni_resources.h"
#include <new>

namespace darwin_art::input {
namespace {
InputTransportConsumptionResult ConsumePacket(void* context, const DarwinArtInputPacket& packet) {
  const auto* policy = static_cast<ReceiverTransportPolicy*>(context);
  if (policy == nullptr || policy->recipient == nullptr || policy->consumer == nullptr)
    return InputTransportConsumptionResult::kDeferred;
  return policy->consumer->ConsumePacket(packet);
}
InputTransportConsumptionResult ApplyWindow(void* context, int32_t left, int32_t top,
                 int32_t right, int32_t bottom, bool visible) {
  const auto* policy = static_cast<ReceiverTransportPolicy*>(context);
  if (policy == nullptr || policy->channel == nullptr || policy->consumer == nullptr)
    return InputTransportConsumptionResult::kDeferred;
  const auto ready = policy->consumer->BeforeControl();
  if (ready != InputTransportConsumptionResult::kConsumed) return ready;
  PublishInputRoutingWmsFrame(policy->channel->Routing(), left, top, right, bottom, visible);
  if (policy->wake_pending != nullptr) policy->wake_pending();
  return InputTransportConsumptionResult::kConsumed;
}
FocusControlCallbackResult ApplyFocus(void* context, uint64_t epoch, bool focused) noexcept {
  const auto* policy = static_cast<ReceiverTransportPolicy*>(context);
  if (policy == nullptr || policy->consumer == nullptr)
    return FocusControlCallbackResult::kDeferred;
  try {
    const auto ready = policy->consumer->BeforeControl();
    if (ready != InputTransportConsumptionResult::kConsumed) return ready;
    return ConsumeReceiverFocusControl(policy->env, policy->receiver,
                                       policy->recipient, {epoch, focused});
  } catch (const std::bad_alloc&) {
    ThrowInputChannelOutOfMemory(policy->env);
  } catch (...) {
    if (policy->env != nullptr && !policy->env->ExceptionCheck()) {
      jclass type = policy->env->FindClass("java/lang/RuntimeException");
      if (type != nullptr) {
        policy->env->ThrowNew(type, "Native ordered input consumption failed");
        policy->env->DeleteLocalRef(type);
      }
    }
  }
  // No focus invocation occurred: retain this head with the reported error.
  return FocusControlCallbackResult::kDeferred;
}
}
InputTransportPumpCallbacks ReceiverTransportCallbacks(ReceiverTransportPolicy* policy) {
  // Peer ACKs belong to publisher observation, never Java nativeFinish state.
  return {.on_focus = ApplyFocus, .context = policy,
          .on_packet_consumption = ConsumePacket,
          .on_window_consumption = ApplyWindow};
}
bool CanDrainReceiverTransport(const std::shared_ptr<InputChannelResources>& channel,
                              int fd, int events) {
  constexpr int kError = 0x0004, kHangup = 0x0008, kInvalid = 0x0010;
  if (channel == nullptr || fd < 0 || (events & (kError | kInvalid)) != 0)
    return false;
  const auto endpoint = channel->Endpoint();
  const auto transport = endpoint == nullptr ? nullptr : endpoint->Transport();
  return transport != nullptr &&
         ((events & kHangup) == 0 || fd == transport->RemoteEndpointFd());
}
}  // namespace darwin_art::input
