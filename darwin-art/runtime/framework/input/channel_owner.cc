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

using darwin_art::input::InputChannelResources;

void WakeLocalInputRouting(const std::shared_ptr<InputChannelResources>& channel) {
  if (channel != nullptr) (void)channel->Endpoint()->WakeLocal();
}

bool EnsureRoutingScheduler(const std::shared_ptr<InputChannelResources>& channel,
                            void* looper) {
  return channel != nullptr && channel->BindContinuation(
      looper, darwin_art::input::RefreshInputReceiverWritable);
}

bool RequestRoutingContinuation(
    const std::shared_ptr<InputChannelResources>& channel) {
  return channel != nullptr && channel->RequestContinuation();
}

void WakePendingInputRouting() {
  const auto channels = darwin_art::input::SnapshotChannelResources();
  for (const auto& channel : channels) {
    const auto drained = darwin_art::input::DrainInputRoutingTransport(channel->Routing());
    if (drained.remote_admitted)
      (void)darwin_art::input::RefreshInputReceiverWritable(channel->Routing());
    if (drained.continuation_needed && !RequestRoutingContinuation(channel))
      std::cerr << "ART InputChannel: routing continuation was not scheduled\n";
    if (darwin_art::input::InputRoutingHasPending(channel->Routing()))
      WakeLocalInputRouting(channel);
  }
}

darwin_art::DarwinArtInputEnqueueResult EnqueueRoutedPacket(
    darwin_art::input::InputRoutingAdmission admission) {
  const auto& routing = admission.state;
  const auto channel = darwin_art::input::FindChannelResourcesForRouting(routing);
  if (channel == nullptr)
    return darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel;
  const auto submitted = darwin_art::input::SubmitInputRoutingAdmission(
      std::move(admission));
  if (submitted.refresh_writable)
    (void)darwin_art::input::RefreshInputReceiverWritable(channel->Routing());
  if (submitted.wake_local) WakeLocalInputRouting(channel);
  if (submitted.continuation_needed && !RequestRoutingContinuation(channel))
    std::cerr << "ART InputChannel: routing continuation was not scheduled\n";
  return submitted.result;
}
}  // namespace

namespace darwin_art {

DarwinArtInputEnqueueResult EnqueueFrameworkPointerPacket(
    const DarwinArtPointerEventV2& packet) {
  darwin_art::input::InputRoutingAdmission admission;
  const auto result = darwin_art::input::RouteFrameworkPointerPacket(
      packet, &admission);
  if (result != DarwinArtInputEnqueueResult::kQueued ||
      admission.state == nullptr)
    return result;
  return EnqueueRoutedPacket(std::move(admission));
}

}  // namespace darwin_art


namespace darwin_art::input {
DarwinArtInputEnqueueResult SubmitFrameworkInputAdmission(InputRoutingAdmission admission) {
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

}  // namespace darwin_art::input
