#include "channel_owner.h"
#include "input_channel_jni.h"
#include "channel_endpoint.h"
#include "channel_resources.h"
#include "input_routing.h"
#include "routing_transport_dispatch.h"
#include "channel_routing_continuation.h"

#include <iostream>
#include <memory>
#include <utility>
#include "receiver_jni.h"

namespace {

using aim::input::InputChannelResources;

void WakeLocalInputRouting(const std::shared_ptr<InputChannelResources>& channel) {
  if (channel != nullptr) (void)channel->Endpoint()->WakeLocal();
}

bool EnsureRoutingScheduler(const std::shared_ptr<InputChannelResources>& channel,
                            void* looper) {
  return channel != nullptr && channel->BindContinuation(
      looper, aim::input::RefreshInputReceiverWritable);
}

bool RequestRoutingContinuation(
    const std::shared_ptr<InputChannelResources>& channel) {
  return channel != nullptr && channel->RequestContinuation();
}

void WakePendingInputRouting() {
  const auto channels = aim::input::SnapshotChannelResources();
  for (const auto& channel : channels) {
    const auto drained = aim::input::DrainInputRoutingTransport(channel->Routing());
    if (drained.remote_admitted)
      (void)aim::input::RefreshInputReceiverWritable(channel->Routing());
    if (drained.continuation_needed && !RequestRoutingContinuation(channel))
      std::cerr << "ART InputChannel: routing continuation was not scheduled\n";
    if (aim::input::InputRoutingHasPending(channel->Routing()))
      WakeLocalInputRouting(channel);
  }
}

aim::AimInputEnqueueResult EnqueueRoutedPacket(
    aim::input::InputRoutingAdmission admission) {
  const auto& routing = admission.state;
  const auto channel = aim::input::FindChannelResourcesForRouting(routing);
  if (channel == nullptr)
    return aim::AimInputEnqueueResult::kNoFocusedChannel;
  const auto submitted = aim::input::SubmitInputRoutingAdmission(
      std::move(admission));
  if (submitted.refresh_writable)
    (void)aim::input::RefreshInputReceiverWritable(channel->Routing());
  if (submitted.wake_local) WakeLocalInputRouting(channel);
  if (submitted.continuation_needed && !RequestRoutingContinuation(channel))
    std::cerr << "ART InputChannel: routing continuation was not scheduled\n";
  return submitted.result;
}
}  // namespace

namespace aim {

AimInputEnqueueResult EnqueueFrameworkPointerPacket(
    const AimPointerEventV2& packet) {
  aim::input::InputRoutingAdmission admission;
  std::vector<aim::input::InputRoutingAdmission> outside;
  const auto result = aim::input::RouteFrameworkPointerPacket(
      packet, &admission, &outside);
  if (result != AimInputEnqueueResult::kQueued ||
      admission.state == nullptr)
    return result;
  // ACTION_OUTSIDE is best effort for its watcher; only the touched window's
  // DOWN decides whether the host packet was accepted.
  for (auto& event : outside) (void)EnqueueRoutedPacket(std::move(event));
  return EnqueueRoutedPacket(std::move(admission));
}

}  // namespace aim


namespace aim::input {
AimInputEnqueueResult SubmitFrameworkInputAdmission(InputRoutingAdmission admission) {
  return EnqueueRoutedPacket(std::move(admission));
}

bool RegisterInputNatives(JNIEnv* env, InputChannelParcelBridge bridge) {
  JavaVM* vm = nullptr;
  if (env == nullptr || env->GetJavaVM(&vm) != JNI_OK || vm == nullptr ||
      bridge.version != 1 || bridge.read == nullptr || bridge.write == nullptr) {
    return false;
  }
  if (!RegisterInputChannelJni(env, bridge)) return false;
  return RegisterInputReceiverNatives(
      env, vm, ReceiverChannelOps{
               .refresh_writable = RefreshInputReceiverWritable,
               .ensure_scheduler = EnsureRoutingScheduler,
               .wake_pending = WakePendingInputRouting,
           });
}

}  // namespace aim::input
