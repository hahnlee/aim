#include "receiver_retirement_barrier.h"

#include <mutex>
#include <stdexcept>
#include <utility>

namespace darwin_art::input {

struct ReceiverRetirementBarrier::Control {
  Control(const ReceiverRoutingLifecycle& owner,
          ReceiverAdmission::RetirementHandle gate,
          ReceiverEndpointBinding::RetirementHandle pumps,
          std::shared_ptr<InputTransport> transport)
      : lifecycle(owner.Retain()), admission(std::move(gate)),
        binding(std::move(pumps)), original_transport(std::move(transport)) {
    const auto endpoint = lifecycle.OriginalEndpoint();
    if (endpoint == nullptr || original_transport == nullptr ||
        endpoint->transport != original_transport)
      throw std::invalid_argument("receiver retirement requires its exact transport");
  }
  ReceiverRoutingLifecycle lifecycle;
  const ReceiverAdmission::RetirementHandle admission;
  const ReceiverEndpointBinding::RetirementHandle binding;
  const std::shared_ptr<InputTransport> original_transport;
  std::mutex mutex;
  bool polling = false;
  // Only the elected poll accesses the fence; never a provider under mutex.
  bool fence_captured = false;
  InputTransportTxFence fence;
};

ReceiverRetirementBarrier::ReceiverRetirementBarrier(
    const ReceiverRoutingLifecycle& lifecycle,
    ReceiverAdmission::RetirementHandle admission,
    ReceiverEndpointBinding::RetirementHandle binding,
    std::shared_ptr<InputTransport> original_transport)
    : control_(std::make_shared<Control>(lifecycle, std::move(admission),
                                        std::move(binding), std::move(original_transport))) {}

ReceiverRetirementBarrier ReceiverRetirementBarrier::Retain() const {
  return ReceiverRetirementBarrier(control_);
}

ReceiverRetirementBarrierQuery ReceiverRetirementBarrier::Poll() {
  const auto control = control_;
  {
    std::lock_guard lock(control->mutex);
    if (control->polling) return {.status = ReceiverRetirementBarrierStatus::kBusy};
    control->polling = true;
  }
  struct ReleaseElection {
    const std::shared_ptr<Control>& control;
    ~ReleaseElection() {
      std::lock_guard lock(control->mutex);
      control->polling = false;
    }
  } release{control};
  ReceiverRetirementBarrierQuery query;
  const auto lifecycle = control->lifecycle.Snapshot();
  if (!lifecycle.logical_closed || lifecycle.operation_pending) return query;
  if (lifecycle.retirement.status == InputRoutingRecipientRetirementStatus::kInvalidRecipient) {
    query.status = ReceiverRetirementBarrierStatus::kInvalid;
    return query;
  }
  if (lifecycle.retirement.status != InputRoutingRecipientRetirementStatus::kNotPublished) {
    if (lifecycle.retirement.ticket.Routing() == nullptr ||
        lifecycle.retirement.ticket.Recipient() == nullptr ||
        lifecycle.retirement.ticket.OriginalEndpoint() != control->lifecycle.OriginalEndpoint()) {
      query.status = ReceiverRetirementBarrierStatus::kInvalid;
      return query;
    }
    query.routing = QueryInputRoutingRetirement(lifecycle.retirement.ticket);
  }
  if (query.routing.admitted_send ||
      (query.routing.status != InputRoutingRetirementStatus::kSettled &&
       query.routing.status != InputRoutingRetirementStatus::kTerminal)) {
    query.status = ReceiverRetirementBarrierStatus::kRouting;
    return query;
  }
  if (!control->admission.IsQuiescent()) {
    query.status = ReceiverRetirementBarrierStatus::kAdmission;
    return query;
  }
  if (!control->binding.IsQuiescent()) {
    query.status = ReceiverRetirementBarrierStatus::kBinding;
    return query;
  }
  if (!control->fence_captured) {
    control->fence = control->original_transport->CaptureAcceptedTxFence();
    control->fence_captured = true;
  }
  query.fence_captured = true;
  query.tx = control->original_transport->QueryTxFence(control->fence);
  switch (query.tx) {
    case InputTransportTxFenceStatus::kFlushed:
    case InputTransportTxFenceStatus::kTerminal:
      query.status = ReceiverRetirementBarrierStatus::kReady;
      break;
    case InputTransportTxFenceStatus::kPending:
      query.status = ReceiverRetirementBarrierStatus::kTx;
      break;
    case InputTransportTxFenceStatus::kInvalid:
      query.status = ReceiverRetirementBarrierStatus::kInvalid;
      break;
  }
  return query;
}

}  // namespace darwin_art::input
