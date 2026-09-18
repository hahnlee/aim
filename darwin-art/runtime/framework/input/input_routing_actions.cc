#include "input_routing_actions_internal.h"
#include "input_routing_domain.h"
#include "input_routing_state_internal.h"
#include "root_key_routing.h"

#include <algorithm>
#include <cassert>
#include <limits>
#include <mutex>
#include <stdexcept>
#include <utility>

namespace darwin_art::input {

struct RoutingActionLeaseState {
  InputRoutingHandle owner;
  std::shared_ptr<routing_internal::InputRoutingStateData::ActionRecord> action;
};

struct RoutingActionLeaseAccess {
  static InputRoutingCancellationLease MoveCancellation(
      InputRoutingInflightLease& lease);
  static void Set(
      InputRoutingInflightLease* lease, const InputRoutingHandle& owner,
      const std::shared_ptr<routing_internal::InputRoutingStateData::ActionRecord>& action);
  static void Set(
      InputRoutingCancellationLease* lease, const InputRoutingHandle& owner,
      const std::shared_ptr<routing_internal::InputRoutingStateData::ActionRecord>& action);
  static std::shared_ptr<RoutingActionLeaseState> Get(
      const InputRoutingInflightLease& lease);
  static std::shared_ptr<RoutingActionLeaseState> Get(
      const InputRoutingCancellationLease& lease);
  static void Clear(InputRoutingInflightLease* lease);
  static void Clear(InputRoutingCancellationLease* lease);
};

namespace {
using routing_internal::InputRoutingStateData;

bool SameEndpoint(const InputRoutingEndpointHandle& first,
                  const InputRoutingEndpointHandle& second) {
  return first != nullptr && second != nullptr && first->transport != nullptr &&
         second->transport != nullptr &&
         first->transport.get() == second->transport.get();
}

void Notify(const InputRoutingHandle& state, InputRoutingNotificationKind kind,
            ReceiverId id, uint64_t generation,
            const InputRoutingEndpointHandle& endpoint) {
  routing_internal::NotifyInputRoutingState(state, kind, id, generation,
                                            endpoint);
}

bool IsTerminatedEndpointLocked(const InputRoutingStateData& state,
                                const InputRoutingEndpointHandle& endpoint) {
  return endpoint != nullptr &&
         std::any_of(state.terminated_endpoints.begin(),
                     state.terminated_endpoints.end(),
                     [&](const auto& item) { return SameEndpoint(item, endpoint); });
}

bool HasAdmittedOldSendLocked(const InputRoutingStateData& data,
                              uint64_t generation) {
  return std::any_of(data.actions.begin(), data.actions.end(),
                     [&](const auto& item) {
                       return item->send_admitted && item->generation == generation;
                     });
}

bool HasInflightGenerationLocked(const InputRoutingStateData& data,
                                 uint64_t generation) {
  return std::any_of(data.actions.begin(), data.actions.end(),
                     [&](const auto& item) {
                       return item->kind == InputRoutingStateData::ActionKind::kPacket &&
                              item->generation == generation;
                     });
}

bool HasPendingCancellationGenerationLocked(const InputRoutingStateData& data,
                                            uint64_t generation) {
  return std::any_of(data.actions.begin(), data.actions.end(),
                     [&](const auto& item) {
                       return item->kind == InputRoutingStateData::ActionKind::kCancellation &&
                              item->generation == generation;
                     });
}

void EraseSettledStreamLocked(InputRoutingStateData* data,
                              uint64_t generation) {
  if (data == nullptr) return;
  const auto found = data->streams.find(generation);
  if (found == data->streams.end() || found->second.active ||
      found->second.cancel_pending ||
      HasInflightGenerationLocked(*data, generation) ||
      HasPendingCancellationGenerationLocked(*data, generation))
    return;
  data->streams.erase(found);
}

void PruneTerminatedEndpointsLocked(InputRoutingStateData* data) {
  if (data == nullptr) return;
  data->terminated_endpoints.erase(
      std::remove_if(data->terminated_endpoints.begin(),
                     data->terminated_endpoints.end(),
                     [&](const auto& endpoint) {
                       const bool stream = std::any_of(
                           data->streams.begin(), data->streams.end(),
                           [&](const auto& item) {
                             return SameEndpoint(item.second.endpoint, endpoint);
                           });
                       const bool action = std::any_of(
                           data->actions.begin(), data->actions.end(),
                           [&](const auto& item) {
                             return SameEndpoint(item->endpoint, endpoint);
                           });
                       return !SameEndpoint(data->endpoint, endpoint) &&
                              !stream && !action;
                     }),
      data->terminated_endpoints.end());
}

void QueueLedgerCancellationLocked(InputRoutingStateData* data,
                                    InputRoutingStateData::StreamLedger* ledger) {
  if (data == nullptr || ledger == nullptr || ledger->cancel_queued ||
      !ledger->active)
    return;
  ledger->cancel_pending = true;
  if (IsTerminatedEndpointLocked(*data, ledger->endpoint)) {
    ledger->cancel_pending = false;
    ledger->cancel_queued = true;
    ledger->active = false;
    return;
  }
  if (data->actions.size() >= routing_internal::kMaxRoutingPackets) return;
  DarwinArtPointerEventV2 cancel = ledger->last_pointer;
  cancel.action = DARWIN_ART_POINTER_CANCEL;
  cancel.sequence = cancel.sequence == std::numeric_limits<uint64_t>::max()
                        ? cancel.sequence : cancel.sequence + 1;
  cancel.pressure = 0.0f;
  cancel.size_value = 0.0f;
  darwin_art::DarwinArtInputPacket packet;
  packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
  packet.pointer = cancel;
  std::shared_ptr<InputRoutingStateData::ActionRecord> action;
  try {
    action = std::make_shared<InputRoutingStateData::ActionRecord>();
    action->packet = packet;
    action->consumer_id = ledger->consumer_id;
    action->generation = ledger->generation;
    action->local_delivery = !ledger->remote;
    action->kind = InputRoutingStateData::ActionKind::kCancellation;
    action->id = data->next_lease_id++;
    action->endpoint = ledger->endpoint;
    data->actions.push_back(action);
  } catch (...) {
    return;
  }
  ledger->cancel_queued = true;
  ledger->cancel_pending = false;
  ledger->active = false;
}

bool RetireStreamGenerationLocked(InputRoutingStateData* data,
                                  uint64_t generation) {
  if (data == nullptr || generation == 0) return false;
  if (data->active_stream_generation == generation)
    data->active_stream_generation = 0;
  auto ledger = data->streams.find(generation);
  if (ledger == data->streams.end()) return false;
  if (HasAdmittedOldSendLocked(*data, generation))
    ledger->second.cancel_pending = true;
  else
    QueueLedgerCancellationLocked(data, &ledger->second);
  return true;
}

void MaybeQueueDeferredCancellationLocked(InputRoutingStateData* data,
                                           uint64_t generation) {
  if (data == nullptr) return;
  auto found = data->streams.find(generation);
  if (found == data->streams.end() || !found->second.cancel_pending ||
      HasAdmittedOldSendLocked(*data, generation)) return;
  QueueLedgerCancellationLocked(data, &found->second);
}

void RecordAcceptedPointerLocked(InputRoutingStateData* data,
                                 const InputRoutingAdmission& admission) {
  if (data == nullptr || admission.packet.kind !=
                             darwin_art::DarwinArtInputPacketKind::kPointer)
    return;
  auto& ledger = data->streams[admission.generation];
  ledger.generation = admission.generation;
  ledger.consumer_id = admission.consumer_id;
  ledger.remote = !admission.local_transport_ready;
  ledger.last_pointer = admission.packet.pointer;
  ledger.endpoint = admission.endpoint;
  if (admission.pointer_down) {
    ledger.active = true;
    ledger.cancel_pending = false;
    ledger.cancel_queued = false;
    data->active_stream_generation = admission.generation;
  } else if (admission.pointer_end) {
    ledger.active = false;
    if (data->active_stream_generation == admission.generation)
      data->active_stream_generation = 0;
  }
}

void QueueInflightCancellationLocked(
    InputRoutingStateData* data,
    const InputRoutingStateData::ActionRecord& inflight) {
  if (data == nullptr || inflight.packet.kind !=
                             darwin_art::DarwinArtInputPacketKind::kPointer ||
      !inflight.pointer_down || IsTerminatedEndpointLocked(*data, inflight.endpoint))
    return;
  auto ledger = data->streams.find(inflight.generation);
  if (ledger != data->streams.end() &&
      ledger->second.consumer_id == inflight.consumer_id &&
      SameEndpoint(ledger->second.endpoint, inflight.endpoint) &&
      (ledger->second.cancel_pending || ledger->second.cancel_queued)) {
    if (!ledger->second.cancel_queued)
      ledger->second.last_pointer = inflight.packet.pointer;
    return;
  }
  if (std::any_of(data->actions.begin(), data->actions.end(),
                  [&](const auto& action) {
                    return action->kind == InputRoutingStateData::ActionKind::kCancellation &&
                           action->generation == inflight.generation &&
                           action->consumer_id == inflight.consumer_id &&
                           SameEndpoint(action->endpoint, inflight.endpoint);
                  }) ||
      data->actions.size() >= routing_internal::kMaxRoutingPackets)
    return;
  DarwinArtPointerEventV2 cancel = inflight.packet.pointer;
  cancel.action = DARWIN_ART_POINTER_CANCEL;
  cancel.sequence = cancel.sequence == std::numeric_limits<uint64_t>::max()
                        ? cancel.sequence : cancel.sequence + 1;
  cancel.pressure = 0.0f;
  cancel.size_value = 0.0f;
  darwin_art::DarwinArtInputPacket packet;
  packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
  packet.pointer = cancel;
  std::shared_ptr<InputRoutingStateData::ActionRecord> action;
  try {
    action = std::make_shared<InputRoutingStateData::ActionRecord>();
  } catch (...) {
    return;
  }
  action->packet = packet;
  action->consumer_id = inflight.consumer_id;
  action->generation = inflight.generation;
  action->local_delivery = inflight.local_delivery;
  action->kind = InputRoutingStateData::ActionKind::kCancellation;
  action->id = data->next_lease_id++;
  action->endpoint = inflight.endpoint;
  action->key_fence = inflight.key_fence;
  // Preserve the original allocation-failure contract: an unretained late
  // CANCEL must not be silently treated as a settled obligation.
  data->actions.push_back(std::move(action));
}

void ApplyAcceptedRoutingPointerLocked(
    InputRoutingStateData* data, const InputRoutingAdmission& admission,
    InputRoutingDomainTransaction& domain) {
  RecordAcceptedPointerLocked(data, admission);
  domain.CommitAcceptedPointer(admission);
}
}  // namespace

InputRoutingCancellationLease RoutingActionLeaseAccess::MoveCancellation(
    InputRoutingInflightLease& lease) {
  InputRoutingCancellationLease cancellation;
  cancellation.state_ = std::move(lease.state_);
  return cancellation;
}

void RoutingActionLeaseAccess::Set(
    InputRoutingInflightLease* lease, const InputRoutingHandle& owner,
    const std::shared_ptr<InputRoutingStateData::ActionRecord>& action) {
  if (lease == nullptr) return;
  lease->state_ = std::make_shared<RoutingActionLeaseState>(
      RoutingActionLeaseState{owner, action});
}

void RoutingActionLeaseAccess::Set(
    InputRoutingCancellationLease* lease, const InputRoutingHandle& owner,
    const std::shared_ptr<InputRoutingStateData::ActionRecord>& action) {
  if (lease == nullptr) return;
  lease->state_ = std::make_shared<RoutingActionLeaseState>(
      RoutingActionLeaseState{owner, action});
}

std::shared_ptr<RoutingActionLeaseState> RoutingActionLeaseAccess::Get(
    const InputRoutingInflightLease& lease) { return lease.state_; }
std::shared_ptr<RoutingActionLeaseState> RoutingActionLeaseAccess::Get(
    const InputRoutingCancellationLease& lease) { return lease.state_; }
void RoutingActionLeaseAccess::Clear(InputRoutingInflightLease* lease) {
  if (lease != nullptr) lease->state_.reset();
}
void RoutingActionLeaseAccess::Clear(InputRoutingCancellationLease* lease) {
  if (lease != nullptr) lease->state_.reset();
}

namespace {
void ReleaseRoutingActionLease(
    const std::shared_ptr<RoutingActionLeaseState>& lease) noexcept {
  if (lease == nullptr || lease->owner == nullptr || lease->action == nullptr)
    return;
  const auto owner = lease->owner;
  const auto action = lease->action;
  bool released = false;
  {
    std::lock_guard<std::mutex> lock(owner->data.mutex);
    if (!action->send_admitted && action->claimed) {
      action->claimed = false;
      released = true;
    }
  }
  if (released)
    Notify(owner, InputRoutingNotificationKind::kLeaseReleased,
           action->consumer_id, action->generation, action->endpoint);
}

}  // namespace

InputRoutingInflightLease::~InputRoutingInflightLease() { ReleaseRoutingActionLease(state_); }
InputRoutingInflightLease::InputRoutingInflightLease(InputRoutingInflightLease&& other) noexcept
    : state_(std::move(other.state_)) {}
InputRoutingInflightLease& InputRoutingInflightLease::operator=(InputRoutingInflightLease&& other) noexcept {
  if (this == &other) return *this;
  InputRoutingInflightLease previous;
  previous.state_ = std::move(state_);
  state_ = std::move(other.state_);
  return *this;
}
const darwin_art::DarwinArtInputPacket* InputRoutingInflightLease::Packet() const {
  return state_ != nullptr && state_->action != nullptr ? &state_->action->packet : nullptr;
}
InputRoutingEndpointHandle InputRoutingInflightLease::Endpoint() const {
  return state_ != nullptr && state_->action != nullptr ? state_->action->endpoint : InputRoutingEndpointHandle{};
}
ReceiverId InputRoutingInflightLease::ConsumerId() const { return state_ && state_->action ? state_->action->consumer_id : 0; }
uint64_t InputRoutingInflightLease::Generation() const { return state_ && state_->action ? state_->action->generation : 0; }
int32_t InputRoutingInflightLease::OffsetX() const { return state_ && state_->action ? state_->action->offset_x : 0; }
int32_t InputRoutingInflightLease::OffsetY() const { return state_ && state_->action ? state_->action->offset_y : 0; }
bool InputRoutingInflightLease::PointerDown() const { return state_ && state_->action && state_->action->pointer_down; }
bool InputRoutingInflightLease::PointerEnd() const { return state_ && state_->action && state_->action->pointer_end; }
bool InputRoutingInflightLease::LocalDelivery() const { return state_ && state_->action && state_->action->local_delivery; }
InputRoutingInflightLease::operator bool() const { return state_ != nullptr && state_->action != nullptr; }

InputRoutingCancellationLease::~InputRoutingCancellationLease() { ReleaseRoutingActionLease(state_); }
InputRoutingCancellationLease::InputRoutingCancellationLease(InputRoutingCancellationLease&& other) noexcept
    : state_(std::move(other.state_)) {}
InputRoutingCancellationLease& InputRoutingCancellationLease::operator=(InputRoutingCancellationLease&& other) noexcept {
  if (this == &other) return *this;
  InputRoutingCancellationLease previous;
  previous.state_ = std::move(state_);
  state_ = std::move(other.state_);
  return *this;
}
const darwin_art::DarwinArtInputPacket* InputRoutingCancellationLease::Packet() const {
  return state_ != nullptr && state_->action != nullptr ? &state_->action->packet : nullptr;
}
InputRoutingEndpointHandle InputRoutingCancellationLease::Endpoint() const {
  return state_ != nullptr && state_->action != nullptr ? state_->action->endpoint : InputRoutingEndpointHandle{};
}
ReceiverId InputRoutingCancellationLease::ConsumerId() const { return state_ && state_->action ? state_->action->consumer_id : 0; }
uint64_t InputRoutingCancellationLease::Generation() const { return state_ && state_->action ? state_->action->generation : 0; }
InputRoutingCancellationLease::operator bool() const { return state_ != nullptr && state_->action != nullptr; }

namespace routing_internal {
bool IsRoutingEndpointTerminatedLocked(const InputRoutingStateData& state,
                                       const InputRoutingEndpointHandle& endpoint) {
  return IsTerminatedEndpointLocked(state, endpoint);
}
bool HasRoutingEndpointReferencesLocked(const InputRoutingStateData& state,
                                        const InputRoutingEndpointHandle& endpoint) {
  if (SameEndpoint(state.endpoint, endpoint) ||
      std::any_of(state.streams.begin(), state.streams.end(),
                  [&](const auto& item) { return SameEndpoint(item.second.endpoint, endpoint); }))
    return true;
  return std::any_of(state.actions.begin(), state.actions.end(),
                     [&](const auto& item) { return SameEndpoint(item->endpoint, endpoint); });
}
bool MarkRoutingEndpointTerminatedLocked(InputRoutingStateData* state,
                                         const InputRoutingEndpointHandle& endpoint) {
  if (state == nullptr || endpoint == nullptr) return false;
  const bool already = IsTerminatedEndpointLocked(*state, endpoint);
  if (already) return false;
  if (state->terminated_endpoints.size() == state->terminated_endpoints.max_size())
    throw std::length_error("input routing terminal endpoint capacity exhausted");
  if (state->terminated_endpoints.size() == state->terminated_endpoints.capacity())
    state->terminated_endpoints.reserve(std::max(
        state->terminated_endpoints.size() + 1,
        state->terminated_endpoints.capacity() <= state->terminated_endpoints.max_size() / 2
            ? state->terminated_endpoints.capacity() * 2
            : state->terminated_endpoints.max_size()));
  state->terminated_endpoints.push_back(endpoint);
  return true;
}
bool RetireActiveRoutingStreamLocked(InputRoutingStateData* data) {
  if (data == nullptr || data->active_stream_generation == 0) return false;
  return RetireStreamGenerationLocked(data, data->active_stream_generation);
}
bool RetireRoutingActionsForRecipientLocked(
    InputRoutingStateData* data, const InputRoutingRetirementTicket& ticket) {
  if (data == nullptr) return false;
  bool changed = false;
  for (auto it = data->streams.begin(); it != data->streams.end();) {
    auto& stream = it->second;
    if (!RoutingEpochBelongsToTicketLocked(*data, ticket, stream.consumer_id,
                                           stream.generation) ||
        stream.endpoint != ticket.OriginalEndpoint()) {
      ++it;
      continue;
    }
    changed = true;
    const bool action = std::any_of(data->actions.begin(), data->actions.end(),
        [&](const auto& item) {
          return item->consumer_id == stream.consumer_id &&
                 item->generation == stream.generation &&
                 item->endpoint == stream.endpoint;
        });
    if (!action && !stream.active && !stream.cancel_pending && !stream.cancel_queued)
      it = data->streams.erase(it);
    else
      ++it;
  }
  for (const auto& action : data->actions) {
    if (RoutingEpochBelongsToTicketLocked(*data, ticket, action->consumer_id,
                                          action->generation) &&
        action->endpoint == ticket.OriginalEndpoint())
      changed = true;
  }
  const auto before = data->actions.size();
  data->actions.erase(std::remove_if(data->actions.begin(), data->actions.end(),
      [&](const auto& action) {
        return RoutingEpochBelongsToTicketLocked(*data, ticket,
                   action->consumer_id, action->generation) &&
               action->endpoint == ticket.OriginalEndpoint() &&
               !action->send_admitted &&
               action->kind != InputRoutingStateData::ActionKind::kCancellation;
      }), data->actions.end());
  changed |= before != data->actions.size();
  return changed;
}

RoutingActionLedgerQuery QueryRoutingActionLedgerLocked(
    const InputRoutingStateData& data,
    const InputRoutingRetirementTicket& ticket) {
  RoutingActionLedgerQuery result;
  result.fifo_empty = data.actions.empty();
  for (const auto& [generation, stream] : data.streams) {
    if (RoutingEpochBelongsToTicketLocked(data, ticket, stream.consumer_id, generation) &&
        stream.endpoint == ticket.OriginalEndpoint() && stream.cancel_pending)
      result.unallocated_cancellation = true;
  }
  for (const auto& action : data.actions) {
    if (!RoutingEpochBelongsToTicketLocked(data, ticket, action->consumer_id,
                                           action->generation) ||
        action->endpoint != ticket.OriginalEndpoint())
      continue;
    result.exact_action = true;
    result.admitted |= action->send_admitted;
    if (action == data.actions.front())
      result.runnable = !action->claimed && !IsTerminatedEndpointLocked(data, action->endpoint) &&
                  (!action->local_delivery || HasRoutingPacketCapacityLocked(data));
  }
  return result;
}

void TerminateRoutingActionsForEndpointLocked(
    InputRoutingStateData* data, const InputRoutingEndpointHandle& endpoint) {
  if (data == nullptr || endpoint == nullptr) return;
  for (auto& item : data->streams)
    if (SameEndpoint(item.second.endpoint, endpoint))
      (void)RetireStreamGenerationLocked(data, item.first);
  data->actions.erase(std::remove_if(data->actions.begin(), data->actions.end(),
      [&](const auto& action) {
        return SameEndpoint(action->endpoint, endpoint) && !action->send_admitted;
      }), data->actions.end());
  for (auto it = data->streams.begin(); it != data->streams.end();) {
    const uint64_t generation = it->first;
    if (SameEndpoint(it->second.endpoint, endpoint)) {
      ++it;
      EraseSettledStreamLocked(data, generation);
    } else {
      ++it;
    }
  }
  PruneTerminatedEndpointsLocked(data);
}

bool HasPendingRoutingActionOrCancellationLocked(const InputRoutingStateData& data) {
  if (!data.actions.empty()) return true;
  return std::any_of(data.streams.begin(), data.streams.end(),
                     [](const auto& item) { return item.second.cancel_pending; });
}
bool HasRoutingActionsLocked(const InputRoutingStateData& data) {
  return !data.actions.empty();
}
bool IsRoutingActionHeadRunnableLocked(const InputRoutingStateData& data) {
  if (data.actions.empty()) return false;
  const auto& head = data.actions.front();
  return !head->claimed && !IsTerminatedEndpointLocked(data, head->endpoint) &&
         (!head->local_delivery || HasRoutingPacketCapacityLocked(data));
}
bool RetryRoutingCancellationsLocked(InputRoutingStateData* data) {
  if (data == nullptr) return false;
  bool queued = std::any_of(data->actions.begin(), data->actions.end(),
      [](const auto& action) {
        return action->kind == InputRoutingStateData::ActionKind::kCancellation;
      });
  for (auto& item : data->streams) {
    auto& ledger = item.second;
    if (ledger.cancel_pending && !HasAdmittedOldSendLocked(*data, ledger.generation)) {
      const auto count = data->actions.size();
      QueueLedgerCancellationLocked(data, &ledger);
      queued |= data->actions.size() > count;
    }
  }
  return queued;
}
}  // namespace routing_internal

bool ReserveInputRoutingPacket(InputRoutingAdmission&& admission, bool local_delivery,
                               InputRoutingInflightLease* lease) {
  if (lease == nullptr || admission.state == nullptr) return false;
  // A failed nested reservation must not destroy or unclaim the caller's
  // existing lease while attempting to overwrite its output.
  if (RoutingActionLeaseAccess::Get(*lease) != nullptr) return false;
  *lease = {};
  const auto state = admission.state;
  const RootKeyRoutingPin root_pin(admission.key_fence);
  auto domain = LockInputRoutingDomain();
  RootKeyRoutingGuard root_guard(domain, root_pin);
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (!root_guard.ValidateReadiness(state->data.focus_cache, state,
                                   state->data.recipient)) return false;
  if (state->data.generation != admission.generation ||
      IsTerminatedEndpointLocked(state->data, admission.endpoint) ||
      state->data.consumer_id == 0 || state->data.consumer_id != admission.consumer_id ||
      state->data.actions.size() >= routing_internal::kMaxRoutingPackets)
    return false;
  if (admission.focus_ready && !InputFocusEpochEvaluator::IsFocusReady(
          domain.FocusEpochDomain(), state->data.focus_cache,
          admission.focus_recipient, admission.focus_epoch,
          admission.focus_cache_revision)) return false;
  if (admission.legacy_key_focus && !InputFocusEpochEvaluator::IsLegacyKeyAdmission(
          domain.FocusEpochDomain(), state->data.focus_cache.present)) return false;
  if (std::any_of(state->data.streams.begin(), state->data.streams.end(),
                  [&](const auto& item) {
                    return item.second.cancel_pending &&
                           SameEndpoint(item.second.endpoint, admission.endpoint);
                  })) return false;
  try {
    auto action = std::make_shared<InputRoutingStateData::ActionRecord>();
    action->packet = admission.packet;
    action->consumer_id = admission.consumer_id;
    action->generation = admission.generation;
    action->offset_x = admission.offset_x;
    action->offset_y = admission.offset_y;
    action->pointer_down = admission.pointer_down;
    action->pointer_end = admission.pointer_end;
    action->local_delivery = local_delivery;
    action->focus_ready = admission.focus_ready;
    action->legacy_key_focus = admission.legacy_key_focus;
    action->focus_epoch = admission.focus_epoch;
    action->focus_cache_revision = admission.focus_cache_revision;
    action->focus_recipient = admission.focus_recipient;
    action->key_fence = admission.key_fence;
    action->id = state->data.next_lease_id++;
    action->endpoint = admission.endpoint;
    RoutingActionLeaseAccess::Set(lease, state, action);
    try {
      state->data.actions.push_back(action);
    } catch (...) {
      RoutingActionLeaseAccess::Clear(lease);
      throw;
    }
  } catch (...) {
    return false;
  }
  return true;
}

bool AcquireInputRoutingHead(const InputRoutingHandle& state,
                             InputRoutingInflightLease* lease) {
  if (state == nullptr || lease == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (state->data.actions.empty()) return false;
  const auto prior = RoutingActionLeaseAccess::Get(*lease);
  const auto action = prior != nullptr && prior->owner == state && prior->action != nullptr
                          ? prior->action : state->data.actions.front();
  if (prior != nullptr && (prior->owner != state || prior->action == nullptr)) return false;
  if (action->claimed || state->data.actions.front() != action ||
      IsTerminatedEndpointLocked(state->data, action->endpoint)) return false;
  action->claimed = true;
  if (prior == nullptr) {
    try { RoutingActionLeaseAccess::Set(lease, state, action); }
    catch (...) { action->claimed = false; return false; }
  }
  return true;
}

bool BeginInputRoutingTransportSend(InputRoutingInflightLease& lease) {
  const auto holder = RoutingActionLeaseAccess::Get(lease);
  if (holder == nullptr || holder->owner == nullptr || holder->action == nullptr) return false;
  const auto state = holder->owner;
  const RootKeyRoutingPin root_pin(holder->action->key_fence);
  auto domain = LockInputRoutingDomain();
  RootKeyRoutingGuard root_guard(domain, root_pin);
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (!root_guard.ValidateReadiness(state->data.focus_cache, state,
                                   state->data.recipient)) return false;
  const auto& action = holder->action;
  if (state->data.actions.empty() || state->data.actions.front() != action ||
      IsTerminatedEndpointLocked(state->data, action->endpoint) ||
      (action->kind == InputRoutingStateData::ActionKind::kPacket &&
       (state->data.generation != action->generation ||
        state->data.consumer_id != action->consumer_id || state->data.consumer_id == 0)) ||
      !action->claimed || action->send_admitted) return false;
  if (action->focus_ready && !InputFocusEpochEvaluator::IsFocusReady(
          domain.FocusEpochDomain(), state->data.focus_cache, action->focus_recipient,
          action->focus_epoch, action->focus_cache_revision)) return false;
  if (action->legacy_key_focus && !InputFocusEpochEvaluator::IsLegacyKeyAdmission(
          domain.FocusEpochDomain(), state->data.focus_cache.present)) return false;
  action->send_admitted = true;
  return true;
}
bool AdmitInputRoutingTransportSend(const InputRoutingInflightLease& lease) {
  return BeginInputRoutingTransportSend(const_cast<InputRoutingInflightLease&>(lease));
}

InputRoutingPacketCompletionStatus CompleteInputRoutingPacketWithStatus(
    InputRoutingInflightLease&& lease, InputRoutingDeliveryResult result) {
  const auto holder = RoutingActionLeaseAccess::Get(lease);
  if (holder == nullptr || holder->owner == nullptr || holder->action == nullptr)
    return InputRoutingPacketCompletionStatus::kTerminal;
  const auto state = holder->owner;
  const auto action = holder->action;
  if (action->kind == InputRoutingStateData::ActionKind::kCancellation) {
    const bool completed = CompleteInputRoutingCancellation(
        RoutingActionLeaseAccess::MoveCancellation(lease),
        result == InputRoutingDeliveryResult::kAccepted
            ? InputRoutingCancellationResult::kAccepted
            : result == InputRoutingDeliveryResult::kTerminal
                  ? InputRoutingCancellationResult::kTerminal
                  : InputRoutingCancellationResult::kBackpressured);
    if (completed) return InputRoutingPacketCompletionStatus::kAccepted;
    if (result == InputRoutingDeliveryResult::kBackpressured ||
        (result == InputRoutingDeliveryResult::kAccepted && action->local_delivery))
      return InputRoutingPacketCompletionStatus::kBackpressured;
    return InputRoutingPacketCompletionStatus::kTerminal;
  }
  if (result == InputRoutingDeliveryResult::kBackpressured) {
    {
      std::lock_guard<std::mutex> lock(state->data.mutex);
      if (state->data.actions.empty() || state->data.actions.front() != action)
        return InputRoutingPacketCompletionStatus::kTerminal;
      action->claimed = false;
      action->send_admitted = false;
      RoutingActionLeaseAccess::Clear(&lease);
    }
    Notify(state, InputRoutingNotificationKind::kActionUnclaimed,
           action->consumer_id, action->generation, action->endpoint);
    return InputRoutingPacketCompletionStatus::kBackpressured;
  }
  if (result != InputRoutingDeliveryResult::kAccepted) {
    {
      auto domain = LockInputRoutingDomain();
      std::lock_guard<std::mutex> lock(state->data.mutex);
      if (state->data.actions.empty() || state->data.actions.front() != action ||
          !action->claimed) return InputRoutingPacketCompletionStatus::kTerminal;
      state->data.actions.pop_front();
      MaybeQueueDeferredCancellationLocked(&state->data, action->generation);
      EraseSettledStreamLocked(&state->data, action->generation);
      PruneTerminatedEndpointsLocked(&state->data);
      RoutingActionLeaseAccess::Clear(&lease);
    }
    // Terminal settlement also releases action capacity. Pending root keys
    // must see this genuine progress, with every routing lock already gone.
    Notify(state, InputRoutingNotificationKind::kPacketCompletion,
           action->consumer_id, action->generation, action->endpoint);
    return InputRoutingPacketCompletionStatus::kTerminal;
  }
  InputRoutingAdmission admission;
  bool stale = false;
  const RootKeyRoutingPin root_pin(action->key_fence);
  {
    auto domain = LockInputRoutingDomain();
    RootKeyRoutingGuard root_guard(domain, root_pin);
    std::lock_guard<std::mutex> lock(state->data.mutex);
    if (state->data.actions.empty() || state->data.actions.front() != action ||
        !action->claimed) return InputRoutingPacketCompletionStatus::kTerminal;
    admission.state = state;
    admission.packet = action->packet;
    admission.generation = action->generation;
    admission.consumer_id = action->consumer_id;
    admission.endpoint = action->endpoint;
    admission.offset_x = action->offset_x;
    admission.offset_y = action->offset_y;
    admission.pointer_down = action->pointer_down;
    admission.pointer_end = action->pointer_end;
    admission.local_transport_ready = action->local_delivery;
    admission.focus_ready = action->focus_ready;
    admission.legacy_key_focus = action->legacy_key_focus;
    admission.focus_epoch = action->focus_epoch;
    admission.focus_cache_revision = action->focus_cache_revision;
    admission.focus_recipient = action->focus_recipient;
    admission.key_fence = action->key_fence;
    stale = !root_guard.ValidateReadiness(state->data.focus_cache, state,
                                         state->data.recipient) ||
            state->data.generation != action->generation ||
            state->data.consumer_id != action->consumer_id ||
            state->data.consumer_id == 0 ||
            (action->legacy_key_focus && !InputFocusEpochEvaluator::IsLegacyKeyAdmission(
                domain.FocusEpochDomain(), state->data.focus_cache.present)) ||
            (action->focus_ready && !InputFocusEpochEvaluator::IsFocusReady(
                domain.FocusEpochDomain(), state->data.focus_cache,
                action->focus_recipient, action->focus_epoch,
                action->focus_cache_revision));
    if (!stale && action->local_delivery && !routing_internal::QueueRoutingPacketLocked(
            &state->data, action->packet, action->consumer_id, action->generation,
            {}, action->key_fence)) {
      action->claimed = false;
      action->send_admitted = false;
      RoutingActionLeaseAccess::Clear(&lease);
      return InputRoutingPacketCompletionStatus::kBackpressured;
    }
    state->data.actions.pop_front();
    if (!stale) ApplyAcceptedRoutingPointerLocked(&state->data, admission, domain);
    else if (!action->local_delivery) QueueInflightCancellationLocked(&state->data, *action);
    MaybeQueueDeferredCancellationLocked(&state->data, action->generation);
    EraseSettledStreamLocked(&state->data, action->generation);
    PruneTerminatedEndpointsLocked(&state->data);
    RoutingActionLeaseAccess::Clear(&lease);
  }
  Notify(state, InputRoutingNotificationKind::kPacketCompletion,
         action->consumer_id, action->generation, action->endpoint);
  return stale ? InputRoutingPacketCompletionStatus::kTerminal
               : InputRoutingPacketCompletionStatus::kAccepted;
}

bool CompleteInputRoutingPacket(InputRoutingInflightLease&& lease,
                                InputRoutingDeliveryResult result) {
  return CompleteInputRoutingPacketWithStatus(std::move(lease), result) ==
         InputRoutingPacketCompletionStatus::kAccepted;
}
bool CommitInputRoutingPacket(InputRoutingAdmission&& admission, bool accepted,
                              bool local_delivery) {
  const auto state = admission.state;
  InputRoutingInflightLease lease;
  if (!ReserveInputRoutingPacket(std::move(admission), local_delivery, &lease) ||
      !AcquireInputRoutingHead(state, &lease)) return false;
  return CompleteInputRoutingPacket(
      std::move(lease), accepted ? InputRoutingDeliveryResult::kAccepted
                                 : InputRoutingDeliveryResult::kTerminal);
}
bool AcquireInputRoutingRetry(const InputRoutingHandle& state,
                              InputRoutingInflightLease* lease) {
  return AcquireInputRoutingHead(state, lease);
}
bool InputRoutingHasPendingCancellation(const InputRoutingHandle& state) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return std::any_of(state->data.actions.begin(), state->data.actions.end(),
      [](const auto& action) {
        return action->kind == InputRoutingStateData::ActionKind::kCancellation;
      });
}
bool AcquireInputRoutingCancellation(const InputRoutingHandle& state,
                                     InputRoutingCancellationLease* lease) {
  if (state == nullptr || lease == nullptr) return false;
  if (RoutingActionLeaseAccess::Get(*lease) != nullptr) return false;
  *lease = {};
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (state->data.actions.empty() ||
      state->data.actions.front()->kind != InputRoutingStateData::ActionKind::kCancellation ||
      state->data.actions.front()->claimed) return false;
  auto& pending = state->data.actions.front();
  pending->claimed = true;
  try { RoutingActionLeaseAccess::Set(lease, state, pending); }
  catch (...) { pending->claimed = false; return false; }
  return true;
}
bool CompleteInputRoutingCancellation(InputRoutingCancellationLease&& lease,
                                      InputRoutingCancellationResult result) {
  const auto holder = RoutingActionLeaseAccess::Get(lease);
  if (holder == nullptr || holder->owner == nullptr || holder->action == nullptr) return false;
  const auto state = holder->owner;
  const auto action = holder->action;
  bool settled = false;
  {
    std::lock_guard<std::mutex> lock(state->data.mutex);
    if (state->data.actions.empty() || state->data.actions.front() != action ||
        action->kind != InputRoutingStateData::ActionKind::kCancellation ||
        !action->claimed) return false;
    if (result == InputRoutingCancellationResult::kAccepted ||
        result == InputRoutingCancellationResult::kTerminal) {
      if (result == InputRoutingCancellationResult::kAccepted && action->local_delivery &&
          !routing_internal::QueueRoutingPacketLocked(
              &state->data, action->packet, action->consumer_id, action->generation,
              {}, action->key_fence)) {
        action->claimed = false;
        action->send_admitted = false;
        RoutingActionLeaseAccess::Clear(&lease);
        return false;
      }
      state->data.actions.pop_front();
      settled = true;
    } else {
      action->claimed = false;
      action->send_admitted = false;
    }
    if (settled) EraseSettledStreamLocked(&state->data, action->generation);
    PruneTerminatedEndpointsLocked(&state->data);
    RoutingActionLeaseAccess::Clear(&lease);
  }
  Notify(state, settled ? InputRoutingNotificationKind::kCancellationCompletion
                        : InputRoutingNotificationKind::kActionUnclaimed,
         action->consumer_id, action->generation, action->endpoint);
  return result == InputRoutingCancellationResult::kAccepted && settled;
}
bool CompleteInputRoutingCancellation(InputRoutingCancellationLease&& lease,
                                      bool accepted) {
  return CompleteInputRoutingCancellation(
      std::move(lease), accepted ? InputRoutingCancellationResult::kAccepted
                                 : InputRoutingCancellationResult::kBackpressured);
}

}  // namespace darwin_art::input
