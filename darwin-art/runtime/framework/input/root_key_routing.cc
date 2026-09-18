#include "root_key_routing.h"
#include <new>

namespace darwin_art::input {
RootKeyRoutingPin::RootKeyRoutingPin(const RootKeyRoutingFence& value) noexcept
    : fence(value), required(value.required || value.ticket != nullptr) {
  if (fence.ticket) {
    authority = fence.ticket->Authority();
    route = fence.ticket->Routing();
  }
}
RootKeyRoutingGuard::RootKeyRoutingGuard(InputRoutingDomainTransaction& domain,
    const RootKeyRoutingPin& pin) noexcept : domain_(domain), pin_(pin) {
  if (pin.required && pin.authority)
    authority_guard_.emplace(LockRootKeyAuthorityForRouting(domain, *pin.authority));
}
bool RootKeyRoutingGuard::ValidateRoot(const InputRoutingHandle& route) const noexcept {
  if (!pin_.required) return true;
  return pin_.route == route && pin_.fence.ticket && authority_guard_ &&
      authority_guard_->ValidateTicket(*pin_.fence.ticket);
}
bool RootKeyRoutingGuard::ValidateReadiness(
    const InputFocusEpochEvaluator::ChannelCache& cache,
    const InputRoutingHandle& route,
    const InputRoutingRecipientHandle& recipient) const noexcept {
  if (!pin_.required) return true;
  return ValidateRoot(route) && recipient && recipient == pin_.fence.focus_recipient &&
      pin_.fence.focus_epoch == pin_.fence.ticket->DecisionEpoch() &&
      InputFocusEpochEvaluator::IsFocusReady(domain_.FocusEpochDomain(), cache,
          pin_.fence.focus_recipient, pin_.fence.focus_epoch, pin_.fence.focus_cache_revision);
}
bool ValidateRootKeyAdmissionForRetry(const InputRoutingAdmission& admission) noexcept {
  RootKeyRoutingPin pin(admission.key_fence);
  InputRoutingSelectionSnapshot selection;
  if (!pin.required || !pin.authority || !pin.route || pin.route != admission.state)
    return false;
  auto domain = LockInputRoutingDomain();
  RootKeyRoutingGuard guard(domain, pin);
  if (!guard.ValidateRoot(admission.state)) return false;
  selection = SnapshotInputRoutingSelection(admission.state);
  return selection.generation == admission.generation &&
      selection.consumer_id == admission.consumer_id &&
      selection.recipient == admission.focus_recipient &&
      InputFocusEpochEvaluator::IsFocusReady(domain.FocusEpochDomain(), admission.state,
          selection.recipient, selection.focus_ready, selection.focus_grant,
          selection.focus_revoked, selection.focus_epoch, selection.focus_cache_revision,
          selection.focus_ready_recipient, selection.focus_ready_epoch,
          selection.focus_ready_cache_revision, admission.focus_epoch,
          admission.focus_cache_revision);
}
darwin_art::DarwinArtInputEnqueueResult RouteRootFrameworkKeyPacket(
    const RootKeyAuthorityHandle& authority, const DarwinArtKeyEventV1& packet,
    InputRoutingAdmission* admission, const DecisionRecordHandle& expected_record) {
  using Result = darwin_art::DarwinArtInputEnqueueResult;
  if (!admission || !authority) return Result::kNoFocusedChannel;
  *admission = {};
  RootKeyAuthorityTicket snapshot;
  {
    auto domain = LockInputRoutingDomain();
    auto root_guard = LockRootKeyAuthorityForRouting(domain, *authority);
    snapshot = root_guard.TrySnapshot();
  }
  if (!snapshot || (expected_record && snapshot.Decision() != expected_record))
    return Result::kNoFocusedChannel;
  RootKeyRoutingFence fence;
  fence.required = true;
  try { fence.ticket = std::make_shared<const RootKeyAuthorityTicket>(snapshot); }
  catch (const std::bad_alloc&) { return Result::kBackpressured; }
  RootKeyRoutingPin pin(fence);  // weak promotions OUTSIDE the domain
  InputRoutingSelectionSnapshot selection;  // release resource pins after unlock
  if (!pin.authority || !pin.route) return Result::kNoFocusedChannel;
  {
    auto domain = LockInputRoutingDomain();
    RootKeyRoutingGuard root_guard(domain, pin);
    if (!root_guard.ValidateRoot(pin.route)) return Result::kNoFocusedChannel;
    selection = SnapshotInputRoutingSelection(pin.route);
    const uint64_t epoch = snapshot.DecisionEpoch();
    if (!InputFocusEpochEvaluator::IsFocusReady(domain.FocusEpochDomain(), pin.route,
          selection.recipient, selection.focus_ready, selection.focus_grant,
          selection.focus_revoked, selection.focus_epoch, selection.focus_cache_revision,
          selection.focus_ready_recipient, selection.focus_ready_epoch,
          selection.focus_ready_cache_revision, epoch, selection.focus_cache_revision) ||
        !ValidateInputRoutingSelection(pin.route, selection, admission))
      return Result::kNoFocusedChannel;
    admission->focus_ready = true;
    admission->focus_epoch = epoch;
    admission->focus_cache_revision = selection.focus_cache_revision;
    admission->focus_recipient = selection.focus_recipient;
    fence.focus_epoch = epoch;
    fence.focus_cache_revision = selection.focus_cache_revision;
    fence.focus_recipient = selection.focus_recipient;
  }
  admission->key_fence = std::move(fence);
  admission->packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
  admission->packet.key = packet;
  return Result::kQueued;
}
}  // namespace darwin_art::input
