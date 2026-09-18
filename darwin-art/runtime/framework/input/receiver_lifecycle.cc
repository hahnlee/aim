#include "receiver_lifecycle.h"

#include "channel_endpoint.h"
#include "pending_receiver_retirement.h"
#include "receiver_jni_resources.h"
#include "receiver_retirement_driver.h"

#include <utility>

namespace darwin_art::input {
namespace {
struct RetirementResourceHooks final {
  std::shared_ptr<ChannelEndpoint> endpoint;
  bool (*refresh_writable)(const InputRoutingHandle&);
};
bool RefreshWritable(void* context, const InputRoutingHandle& routing) noexcept {
  const auto* resources = static_cast<RetirementResourceHooks*>(context);
  try {
    return resources->refresh_writable != nullptr &&
           resources->refresh_writable(routing);
  } catch (...) {
    return false;  // The driver retains this failed resource obligation.
  }
}
bool WakeLocal(void* context) noexcept {
  const auto* resources = static_cast<RetirementResourceHooks*>(context);
  return resources->endpoint->WakeLocal();
}
}  // namespace

bool PrepareReceiverRetirement(
    InputReceiver& receiver, InputRoutingRecipientHandle recipient,
    std::shared_ptr<ChannelEndpoint> endpoint, InputRoutingHandle routing,
    bool (*refresh_writable)(const InputRoutingHandle&)) {
  if (receiver.pending_retirement != nullptr || recipient == nullptr ||
      endpoint == nullptr || routing == nullptr || receiver.looper == nullptr ||
      receiver.registry_id == 0)
    return false;
  const auto transport = endpoint->Transport();
  if (transport == nullptr) return false;
  ReceiverRoutingLifecycle lifecycle(recipient);
  auto record = PreparePendingReceiverRetirement(
      lifecycle, receiver.admission.RetainRetirement(),
      receiver.binding.RetainRetirement(), transport);
  auto resources = std::make_shared<RetirementResourceHooks>(
      RetirementResourceHooks{std::move(endpoint), refresh_writable});
  const int remote_fd = transport->RemoteEndpointFd();
  auto driver = ReceiverRetirementDriver::Prepare(
      record, receiver.looper, transport,
      remote_fd >= 0 ? remote_fd : transport->ReadFd(),
      receiver.registry_id, std::move(routing),
      {.refresh_writable = RefreshWritable, .wake_local = WakeLocal,
       .context = resources.get(), .context_owner = resources,
       .context_token = resources});
  if (driver == nullptr) return false;
  // Install rollback handles before any remaining preparation can fail.
  receiver.pending_retirement = record;
  receiver.retirement_driver = driver;
  receiver.routing_recipient = recipient;
  return AttachPendingReceiverRetirementDriver(record, std::move(driver)) &&
         EnlistPendingReceiverRetirement(record);
}

ReceiverRoutingPublishResult PublishReceiverRetirement(InputReceiver& receiver) {
  if (receiver.pending_retirement == nullptr) return {};
  return PublishPendingReceiverRetirement(receiver.pending_retirement);
}

bool CloseReceiverRetirement(InputReceiver& receiver) noexcept {
  return receiver.retirement_driver == nullptr || receiver.retirement_driver->Close();
}

ReceiverInitializationAdmission::ReceiverInitializationAdmission(
    JNIEnv* env, std::shared_ptr<InputReceiver> receiver)
    : env_(env), receiver_(std::move(receiver)),
      admitted_(receiver_->admission.Admit()) {}

ReceiverInitializationAdmission::~ReceiverInitializationAdmission() {
  if (!committed_) {
    (void)CloseReceiverRetirement(*receiver_);
    (void)RetireInputReceiver(receiver_->registry_id);
    RetireReceiverResources(env_, receiver_);
  }
  if (admitted_) ReleaseReceiverAdmission(env_, receiver_.get());
}

void ReceiverInitializationAdmission::Commit() {
  committed_ = true;
  if (admitted_) {
    admitted_ = false;
    ReleaseReceiverAdmission(env_, receiver_.get());
  }
}
}  // namespace darwin_art::input
