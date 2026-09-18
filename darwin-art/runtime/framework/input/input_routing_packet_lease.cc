#include "input_routing.h"
#include "input_routing_state_internal.h"
#include "root_key_routing.h"

#include <cstddef>
#include <mutex>
#include <utility>

namespace darwin_art::input {
namespace {

using routing_internal::InputRoutingStateData;
using routing_internal::kMaxRoutingPackets;

bool ClaimIdInUseLocked(const InputRoutingStateData& data, uint64_t id) {
  for (const auto& packet : data.packets) {
    if (packet.claim_id == id) return true;
  }
  return false;
}

uint64_t AllocatePacketClaimIdLocked(InputRoutingStateData* data) {
  if (data == nullptr) return 0;
  // next_lease_id is shared with transport action leases. Skip zero and any
  // still-live queue claim so a stale lease can never complete a new head.
  for (size_t attempts = 0; attempts != data->packets.size() + 2; ++attempts) {
    uint64_t candidate = data->next_lease_id++;
    if (candidate == 0) continue;
    if (!ClaimIdInUseLocked(*data, candidate)) return candidate;
  }
  return 0;
}

}  // namespace

struct InputRoutingPacketLeaseState {
  InputRoutingHandle owner;
  InputRoutingRecipientHandle recipient;
  // Keep the payload independent of deque element lifetime. Retirement and
  // replacement may erase/mutate queue storage only after this claim settles.
  darwin_art::DarwinArtInputPacket packet;
  uint64_t generation = 0;
  uint64_t claim_id = 0;
  RootKeyRoutingFence key_fence;
  bool java_admitted = false;
  // An admitted synchronous Java call keeps the promoted root authority alive
  // through invocation. Dropping a final provider pin can run release callbacks;
  // that must not happen between final admission and CallVoidMethod.
  RootKeyAuthorityHandle delivery_authority;
};

namespace {

bool ReleaseClaim(const std::shared_ptr<InputRoutingPacketLeaseState>& holder,
                  bool settle_stale,
                  bool* settled_stale = nullptr) {
  if (holder == nullptr || holder->owner == nullptr ||
      holder->recipient == nullptr || holder->claim_id == 0)
    return false;
  bool released = false;
  bool settled = false;
  bool released_capacity = false;
  {
    std::lock_guard<std::mutex> lock(holder->owner->data.mutex);
    if (!holder->owner->data.packets.empty()) {
      auto& head = holder->owner->data.packets.front();
      if (head.claim_id == holder->claim_id &&
          head.recipient == holder->recipient) {
        // A deferred lease normally rolls back non-destructively. Once the
        // publication has been replaced/retired, however, its exact record
        // can no longer be invoked by any owner. Retirement is an implicit
        // terminal rejection for that record; settle it here so successor
        // traffic is not permanently stranded behind a stale head.
        const bool still_current =
            holder->owner->data.recipient == holder->recipient &&
            holder->owner->data.consumer_id == holder->recipient->id &&
            holder->owner->data.endpoint == holder->recipient->originalendpoint &&
            holder->owner->data.generation == holder->generation;
        if (settle_stale && !still_current) {
          released_capacity = holder->owner->data.packets.size() >=
                              kMaxRoutingPackets;
          holder->owner->data.packets.pop_front();
          holder->owner->data.pending_input.store(
              !holder->owner->data.packets.empty(), std::memory_order_release);
          settled = true;
        } else {
          head.claim_id = 0;
          released = true;
        }
      }
    }
  }
  if (settled_stale != nullptr) *settled_stale = settled;
  if (settled) {
    try {
      if (released_capacity) {
        routing_internal::NotifyInputRoutingState(
            holder->owner, InputRoutingNotificationKind::kLocalCapacity,
            holder->recipient->id, holder->generation,
            holder->recipient->originalendpoint);
      }
      routing_internal::NotifyInputRoutingState(
          holder->owner, InputRoutingNotificationKind::kPacketCompletion,
          holder->recipient->id, holder->generation,
          holder->recipient->originalendpoint);
    } catch (...) {
      // Settlement has already been linearized; advisory callbacks cannot
      // make stale traffic live again.
    }
  }
  if (released) {
    try {
      routing_internal::NotifyInputRoutingState(
          holder->owner, InputRoutingNotificationKind::kLeaseReleased,
          holder->recipient->id, holder->generation,
          holder->recipient->originalendpoint);
    } catch (...) {
      // Lease rollback is noexcept; notification is advisory only.
    }
  }
  return released;
}

}  // namespace

InputRoutingPacketLease::~InputRoutingPacketLease() {
  (void)ReleaseClaim(state_, true);
}

InputRoutingPacketLease::InputRoutingPacketLease(
    InputRoutingPacketLease&& other) noexcept
    : state_(std::move(other.state_)) {}

InputRoutingPacketLease& InputRoutingPacketLease::operator=(
    InputRoutingPacketLease&& other) noexcept {
  if (this == &other) return *this;
  InputRoutingPacketLease previous;
  previous.state_ = std::move(state_);
  state_ = std::move(other.state_);
  return *this;
}

const darwin_art::DarwinArtInputPacket* InputRoutingPacketLease::Packet() const {
  return state_ == nullptr || state_->recipient == nullptr ? nullptr
                                                            : &state_->packet;
}

InputRoutingRecipientHandle InputRoutingPacketLease::Recipient() const {
  return state_ == nullptr ? InputRoutingRecipientHandle{} : state_->recipient;
}

InputRoutingEndpointHandle InputRoutingPacketLease::Endpoint() const {
  return state_ == nullptr || state_->recipient == nullptr
             ? InputRoutingEndpointHandle{}
             : state_->recipient->originalendpoint;
}

RootKeyRoutingFence InputRoutingPacketLease::KeyFence() const {
  return state_ == nullptr ? RootKeyRoutingFence{} : state_->key_fence;
}

bool BeginInputRoutingPacketDelivery(InputRoutingPacketLease& lease,
    const InputRoutingRecipientHandle& expected_recipient,
    const InputRoutingHandle& expected_routing) {
  const auto holder = lease.state_;
  if (!holder || !holder->owner || !holder->recipient || !holder->claim_id)
    return false;
  if (holder->recipient != expected_recipient || holder->owner != expected_routing)
    return false;
  const RootKeyRoutingPin root_pin(holder->key_fence);
  auto domain = LockInputRoutingDomain();
  RootKeyRoutingGuard root_guard(domain, root_pin);
  std::lock_guard<std::mutex> lock(holder->owner->data.mutex);
  const auto& data = holder->owner->data;
  if (holder->java_admitted || data.packets.empty() ||
      data.packets.front().claim_id != holder->claim_id ||
      data.packets.front().recipient != holder->recipient ||
      data.packets.front().key_fence.ticket != holder->key_fence.ticket ||
      data.recipient != holder->recipient ||
      (root_pin.required && data.generation != holder->generation) ||
      data.consumer_id != holder->recipient->id ||
      data.endpoint != holder->recipient->originalendpoint ||
      !root_guard.ValidateReadiness(data.focus_cache, holder->owner,
                                   holder->recipient)) return false;
  holder->delivery_authority = root_pin.authority;
  holder->java_admitted = true;
  return true;
}

bool InputRoutingPacketLease::Complete() {
  return Complete(InputRoutingDeliveryResult::kAccepted);
}

bool InputRoutingPacketLease::Complete(bool invoked) {
  return Complete(invoked ? InputRoutingDeliveryResult::kAccepted
                          : InputRoutingDeliveryResult::kTerminal);
}

bool InputRoutingPacketLease::Complete(InputRoutingDeliveryResult result) {
  if (state_ == nullptr || state_->owner == nullptr ||
      state_->recipient == nullptr || state_->claim_id == 0)
    return false;
  const auto holder = state_;
  if (result == InputRoutingDeliveryResult::kBackpressured) {
    (void)ReleaseClaim(holder, false);
    state_.reset();
    return false;
  }

  bool consumed = false;
  bool released_capacity = false;
  {
    std::lock_guard<std::mutex> lock(holder->owner->data.mutex);
    if (!holder->owner->data.packets.empty()) {
      const auto& head = holder->owner->data.packets.front();
      if (head.claim_id == holder->claim_id &&
          head.recipient == holder->recipient) {
        released_capacity = holder->owner->data.packets.size() >=
                            kMaxRoutingPackets;
        holder->owner->data.packets.pop_front();
        holder->owner->data.pending_input.store(
            !holder->owner->data.packets.empty(), std::memory_order_release);
        consumed = true;
      }
    }
  }
  state_.reset();
  if (consumed) {
    // Capacity is advertised only after explicit completion, never on claim
    // or lease destruction. Keep this advisory and outside the queue lock.
    if (released_capacity) {
      routing_internal::NotifyInputRoutingState(
          holder->owner, InputRoutingNotificationKind::kLocalCapacity,
          holder->recipient->id, holder->generation,
          holder->recipient->originalendpoint);
    }
    routing_internal::NotifyInputRoutingState(
        holder->owner, InputRoutingNotificationKind::kPacketCompletion,
        holder->recipient->id, holder->generation,
        holder->recipient->originalendpoint);
  }
  return consumed;
}

InputRoutingPacketLease::operator bool() const {
  return Packet() != nullptr;
}

bool AcquireInputRoutingPacketLease(
    const InputRoutingHandle& state,
    const InputRoutingRecipientHandle& expected_recipient,
    InputRoutingPacketLease* lease) {
  if (state == nullptr || expected_recipient == nullptr || lease == nullptr)
    return false;
  // A live destination lease is itself an outstanding claim.  Do not move it
  // aside on a failed nested acquisition (which would silently unclaim the
  // original head and permit duplicate consumption).
  if (lease->state_ != nullptr) return false;
  if (expected_recipient->Routing() != state) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (state->data.packets.empty()) return false;
  auto& head = state->data.packets.front();
  if (head.claim_id != 0 || head.recipient != expected_recipient ||
      head.consumer_id != expected_recipient->id ||
      head.generation == 0)
    return false;
  const uint64_t claim_id = AllocatePacketClaimIdLocked(&state->data);
  if (claim_id == 0) return false;
  head.claim_id = claim_id;
  try {
    lease->state_ = std::make_shared<InputRoutingPacketLeaseState>(
        InputRoutingPacketLeaseState{state, expected_recipient, head.packet,
                                     head.generation, claim_id, head.key_fence,
                                     false, {}});
  } catch (...) {
    head.claim_id = 0;
    return false;
  }
  return true;
}

}  // namespace darwin_art::input
