#include "input_routing.h"
#include "input_routing_actions_internal.h"
#include "input_routing_domain.h"
#include "input_routing_state_internal.h"
#include "input_window_state.h"

#include <algorithm>
#include <atomic>
#include <cassert>
#include <deque>
#include <limits>
#include <mutex>
#include <stdexcept>
#include <unordered_map>
#include <utility>
#include <vector>

namespace darwin_art::input {
namespace {
using routing_internal::InputRoutingStateData;


}  // namespace

// Only the routing owner creates valid tickets, while holding its mutex.
// Callers can copy/query a ticket but cannot forge its identity or cutoff.
struct InputRoutingTicketAccess {
  static InputRoutingRetirementTicket Make(
      InputRoutingHandle routing, ReceiverId id, uint64_t generation,
      InputRoutingEndpointHandle endpoint, InputRoutingRecipientHandle recipient) {
    InputRoutingRetirementTicket ticket;
    ticket.routing_ = std::move(routing);
    ticket.id_ = id;
    ticket.cutoffgeneration_ = generation;
    ticket.originalendpoint_ = std::move(endpoint);
    ticket.recipient_ = std::move(recipient);
    return ticket;
  }
};

struct InputRoutingNotificationSubscription {
  InputRoutingHandle owner;
  InputRoutingNotificationCallbacks callbacks;
};

// A persistent weak chain makes notification snapshots allocation-free.
// Links never retain subscriber policy/context or the routing state.
struct InputRoutingNotificationLink {
  std::weak_ptr<InputRoutingNotificationSubscription> subscription;
  std::shared_ptr<const InputRoutingNotificationLink> next;
};

namespace {


void NotifyOutsideLocks(InputRoutingHandle state,
                        InputRoutingNotificationKind kind, ReceiverId id,
                        uint64_t generation,
                        const InputRoutingEndpointHandle& endpoint) {
  if (state == nullptr) return;
  InputRoutingNotification notification;
  std::shared_ptr<const InputRoutingNotificationLink> targets;
  {
    std::lock_guard<std::mutex> lock(state->data.mutex);
    notification.kind = kind;
    notification.id = id;
    notification.generation = generation;
    notification.originalendpoint = endpoint;
    notification.revision = ++state->data.revision;
    targets = state->data.notification_subscriptions;
  }
  // The callback is intentionally outside both routing and global focus
  // locks.  The weak token prevents a late completion from retaining a
  // receiver/channel owner through its callback context.
  for (auto link = targets; link != nullptr; link = link->next) {
    const auto subscription = link->subscription.lock();
    if (subscription == nullptr || subscription->owner != state) continue;
    if (subscription->callbacks.on_notification == nullptr)
      continue;
    // Pin the context through invocation; expired() alone races destruction.
    // A missing token is invalid, not permission to borrow an unowned pointer.
    const auto context_pin = subscription->callbacks.context_token.lock();
    if (context_pin == nullptr) continue;
    try {
      subscription->callbacks.on_notification(subscription->callbacks.context,
                                               notification);
    } catch (...) {
      // A notification is advisory. Never let an observer exception cross a
      // transport/looper boundary or alter the already-linearized state.
    }
  }
}


bool QueuePacketLocked(InputRoutingStateData* state,
                       const darwin_art::DarwinArtInputPacket& packet,
                       ReceiverId consumer_id = 0, uint64_t generation = 0,
                       InputRoutingRecipientHandle recipient = {},
                       RootKeyRoutingFence key_fence = {}) {
  if (state == nullptr) return false;
  if (recipient == nullptr && consumer_id != 0 && generation != 0) {
    if (state->recipient != nullptr && state->generation == generation &&
        state->recipient->id == consumer_id) {
      recipient = state->recipient;
    } else {
      for (const auto& historical : state->recipient_generations) {
        if (historical.generation == generation && historical.recipient != nullptr &&
            historical.recipient->id == consumer_id) {
          recipient = historical.recipient;
          break;
        }
      }
    }
  }
  if (state->packets.size() >= routing_internal::kMaxRoutingPackets) {
    if (packet.kind == darwin_art::DarwinArtInputPacketKind::kPointer &&
        packet.pointer.action == DARWIN_ART_POINTER_MOVE &&
        !state->packets.empty() &&
        state->packets.back().claim_id == 0 &&
        state->packets.back().packet.kind ==
            darwin_art::DarwinArtInputPacketKind::kPointer &&
        state->packets.back().packet.pointer.action == DARWIN_ART_POINTER_MOVE) {
      state->packets.back().packet = packet;
      state->packets.back().consumer_id = consumer_id;
      state->packets.back().generation = generation;
      state->packets.back().recipient = std::move(recipient);
      state->packets.back().claim_id = 0;
      state->packets.back().key_fence = std::move(key_fence);
    } else {
      return false;
    }
  } else {
    state->packets.push_back(
        {packet, consumer_id, generation, std::move(recipient), 0,
         std::move(key_fence)});
  }
  state->pending_input.store(true, std::memory_order_release);
  return true;
}

bool SameEndpoint(const InputRoutingEndpointHandle& first,
                  const InputRoutingEndpointHandle& second) {
  return first != nullptr && second != nullptr && first->transport != nullptr &&
         second->transport != nullptr &&
         first->transport.get() == second->transport.get();
}

bool SameRecipient(const InputRoutingRecipientHandle& first,
                   const InputRoutingRecipientHandle& second) {
  return first != nullptr && second != nullptr && first.get() == second.get();
}

// A cutoff spans multiple epochs of one publication, never older publications
// that happened to reuse the same numeric ID and immutable endpoint object.
bool IsRetiredRecipientLocked(const InputRoutingStateData& state,
                              ReceiverId consumer_id, uint64_t generation,
                              const InputRoutingEndpointHandle& endpoint = {}) {
  if (state.recipient != nullptr && state.generation == generation &&
      state.recipient->id == consumer_id &&
      (endpoint == nullptr || state.recipient->originalendpoint == endpoint))
    return false;
  return std::any_of(
      state.recipient_generations.begin(), state.recipient_generations.end(),
      [&](const auto& item) {
        return item.generation == generation &&
               item.recipient != nullptr && item.recipient->id == consumer_id &&
               (endpoint == nullptr ||
                item.recipient->originalendpoint == endpoint);
      });
}

void RecordRetiredRecipientLocked(
    InputRoutingStateData* state, ReceiverId consumer_id, uint64_t generation,
    const InputRoutingEndpointHandle& endpoint) {
  if (state == nullptr || state->recipient == nullptr || consumer_id == 0 ||
      generation == 0 || generation != state->generation ||
      state->recipient->id != consumer_id ||
      state->recipient->originalendpoint != endpoint)
    return;
  const bool present = std::any_of(
      state->recipient_generations.begin(), state->recipient_generations.end(),
      [&](const auto& item) {
        return item.generation == generation &&
               SameRecipient(item.recipient, state->recipient);
      });
  if (!present) {
    // Publication/generation preparation reserves this slot before exposing
    // the epoch. Logical revocation never allocates its own history storage.
    assert(state->recipient_generations.size() < state->recipient_generations.capacity());
    state->recipient_generations.push_back({state->recipient, generation});
  }
}

template <typename T>
void ReserveExtraCapacity(std::vector<T>* values, size_t extra) {
  if (extra > values->max_size() - values->size())
    throw std::length_error("input routing epoch capacity exhausted");
  const size_t required = values->size() + extra;
  if (required <= values->capacity()) return;
  const size_t grown = values->capacity() <= values->max_size() / 2
      ? values->capacity() * 2 : values->max_size();
  values->reserve(std::max(required, grown));
}

// Fallible preparation must precede policy mutations, including across both
// channels in a focus handoff. Leave one unused slot for the successor epoch.
void PrepareGenerationAdvanceLocked(InputRoutingStateData* state) {
  if (state == nullptr || state->recipient == nullptr) return;
  const bool recorded = std::any_of(state->recipient_generations.begin(),
      state->recipient_generations.end(), [&](const auto& epoch) {
        return epoch.generation == state->generation &&
               SameRecipient(epoch.recipient, state->recipient);
      });
  ReserveExtraCapacity(&state->recipient_generations, recorded ? 1 : 2);
}

// Commit only: caller has prepared storage, or cleared the recipient as part
// of exact revocation. No history allocation is permitted after the cutoff.
void AdvancePreparedGenerationLocked(InputRoutingStateData* state) {
  if (state == nullptr) return;
  RecordRetiredRecipientLocked(state, state->consumer_id, state->generation,
                               state->endpoint);
  ++state->generation;
  assert(state->recipient == nullptr ||
         state->recipient_generations.size() < state->recipient_generations.capacity());
}

}  // namespace

namespace routing_internal {
bool HasRoutingPacketCapacityLocked(const InputRoutingStateData& state) {
  return state.packets.size() < kMaxRoutingPackets;
}

bool RoutingEpochBelongsToTicketLocked(
    const InputRoutingStateData& data,
    const InputRoutingRetirementTicket& ticket, ReceiverId id,
    uint64_t generation) {
  if (id != ticket.ConsumerId() || generation == 0 ||
      generation > ticket.CutoffGeneration() || ticket.Recipient() == nullptr)
    return false;
  if (generation == data.generation && SameRecipient(data.recipient, ticket.Recipient()))
    return true;
  return std::any_of(data.recipient_generations.begin(), data.recipient_generations.end(),
      [&](const auto& epoch) {
        return epoch.generation == generation && SameRecipient(epoch.recipient, ticket.Recipient());
      });
}

bool QueueRoutingPacketLocked(
    InputRoutingStateData* state,
    const darwin_art::DarwinArtInputPacket& packet, ReceiverId consumer_id,
    uint64_t generation, InputRoutingRecipientHandle recipient,
    RootKeyRoutingFence key_fence) {
  return QueuePacketLocked(state, packet, consumer_id, generation,
                           std::move(recipient), std::move(key_fence));
}
}  // namespace routing_internal

void routing_internal::NotifyInputRoutingState(
    const InputRoutingHandle& state, InputRoutingNotificationKind kind,
    ReceiverId id, uint64_t generation,
    const InputRoutingEndpointHandle& endpoint) {
  NotifyOutsideLocks(state, kind, id, generation, endpoint);
}

InputRoutingRecipientHandle PrepareInputRoutingRecipient(
    const InputRoutingHandle& state, ReceiverId id,
    InputRoutingEndpointHandle original_endpoint) {
  if (state == nullptr || id == 0) return {};
  try {
    return std::make_shared<const InputRoutingRecipient>(
        state, id, std::move(original_endpoint));
  } catch (...) {
    return {};
  }
}

InputRoutingPublicationResult PublishInputRoutingRecipient(
    const InputRoutingRecipientHandle& recipient, ReceiverId expected_current_id) {
  InputRoutingPublicationResult result;
  if (recipient == nullptr || recipient->routing.expired() || recipient->id == 0)
    return result;
  const auto state = recipient->routing.lock();
  if (state == nullptr) return result;
  ReceiverId retired_id = 0;
  uint64_t retired_generation = 0;
  InputRoutingEndpointHandle retired_endpoint;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> lock(state->data.mutex);
    if (expected_current_id != 0 &&
        state->data.consumer_id != expected_current_id) {
      result.status = InputRoutingPublicationStatus::kCurrentMismatch;
      return result;
    }
    if (recipient->published_) {
      result.status = InputRoutingPublicationStatus::kAlreadyPublished;
      return result;
    }
    if (state->data.recipient != nullptr)
      PrepareGenerationAdvanceLocked(&state->data);
    else
      ReserveExtraCapacity(&state->data.recipient_generations, 1);
    if (state->data.recipient != nullptr) {
      retired_id = state->data.recipient->id;
      retired_generation = state->data.generation;
      retired_endpoint = state->data.recipient->originalendpoint;
      RecordRetiredRecipientLocked(&state->data, retired_id,
                                   retired_generation, retired_endpoint);
      result.predecessor = InputRoutingTicketAccess::Make(state, retired_id,
          retired_generation, retired_endpoint, state->data.recipient);
      (void)routing_internal::RetireActiveRoutingStreamLocked(&state->data);
      domain.ClearFocus(state);
      domain.ClearCapture(state);
    }
    // Publication is one lock-protected transaction: predecessor is sealed
    // before successor becomes visible to routing or focus.
    if (state->data.consumer_id != recipient->id ||
        state->data.endpoint != recipient->originalendpoint ||
        !SameRecipient(state->data.recipient, recipient)) {
      if (state->data.recipient != nullptr)
        AdvancePreparedGenerationLocked(&state->data);
      state->data.recipient = recipient;
      state->data.consumer_id = recipient->id;
      state->data.endpoint = recipient->originalendpoint;
      state->data.transport_ready = false;
    }
    recipient->published_ = true;
    result.status = InputRoutingPublicationStatus::kPublished;
  }
  if (retired_id != 0)
    NotifyOutsideLocks(state, InputRoutingNotificationKind::kRetirement,
                       retired_id, retired_generation, retired_endpoint);
  return result;
}

// Caller holds global focus and routing locks. Capturing a conditional detach
// ticket and sealing its recipient must be one transaction.
static bool RetireInputRoutingRecipientLocked(
    const InputRoutingRetirementTicket& ticket,
    InputRoutingDomainTransaction& domain) {
  const auto state = ticket.Routing();
  if (state == nullptr || ticket.ConsumerId() == 0 || ticket.CutoffGeneration() == 0)
    return false;
  bool changed = false;
  bool detached = false;
  {
    const bool exact_current =
        state->data.consumer_id == ticket.ConsumerId() &&
        state->data.endpoint == ticket.OriginalEndpoint() &&
        state->data.generation == ticket.CutoffGeneration() &&
        SameRecipient(state->data.recipient, ticket.Recipient());
    if (exact_current) {
      // Retain the exact epoch while its publication identity is still live.
      // Clearing recipient first makes AdvancePreparedGenerationLocked unable to
      // record it, and the envelope purge below then misses first-epoch input.
      // This allocation precedes any revocation so failure leaves it intact.
      RecordRetiredRecipientLocked(&state->data, ticket.ConsumerId(),
                                   ticket.CutoffGeneration(),
                                   ticket.OriginalEndpoint());
      domain.ClearFocus(state);
      domain.ClearCapture(state);
      (void)routing_internal::RetireActiveRoutingStreamLocked(&state->data);
      state->data.recipient.reset();
      state->data.consumer_id = 0;
      state->data.transport_ready = false;
      AdvancePreparedGenerationLocked(&state->data);
      detached = true;
      changed = true;
    }
    // Detach and purge only the exact recipient generations covered by this
    // ticket. A later same-id publication, even on the same transport, is not
    // touched. Admitted sends remain for their completion fence.
    const auto packet_count = state->data.packets.size();
    state->data.packets.erase(
        std::remove_if(state->data.packets.begin(), state->data.packets.end(),
                       [&](const auto& packet) {
                         if (packet.claim_id != 0) return false;
                         return routing_internal::RoutingEpochBelongsToTicketLocked(state->data, ticket,
                             packet.consumer_id, packet.generation);
                       }),
        state->data.packets.end());
    changed |= state->data.packets.size() != packet_count;
    // The action owner retires exact stream/action epochs. Recipient history and
    // the local packet FIFO remain owned by this routing transaction.
    changed |= routing_internal::RetireRoutingActionsForRecipientLocked(
        &state->data, ticket);
    state->data.pending_input.store(!state->data.packets.empty(),
                                    std::memory_order_release);
  }
  return changed || detached;
}

bool RetireInputRoutingRecipient(const InputRoutingRetirementTicket& ticket) {
  if (ticket.Routing() == nullptr || ticket.ConsumerId() == 0 ||
      ticket.CutoffGeneration() == 0) return false;
  bool changed;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> lock(ticket.Routing()->data.mutex);
    changed = RetireInputRoutingRecipientLocked(ticket, domain);
  }
  if (changed)
    NotifyOutsideLocks(ticket.Routing(), InputRoutingNotificationKind::kRetirement,
                       ticket.ConsumerId(), ticket.CutoffGeneration(), ticket.OriginalEndpoint());
  return changed;
}

InputRoutingRecipientRetirementResult RetireInputRoutingRecipient(
    const InputRoutingRecipientHandle& recipient) {
  InputRoutingRecipientRetirementResult result;
  if (recipient == nullptr || recipient->id == 0) return result;
  const auto state = recipient->Routing();
  if (state == nullptr) return result;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> lock(state->data.mutex);
    uint64_t cutoff = 0;
    if (SameRecipient(state->data.recipient, recipient)) {
      cutoff = state->data.generation;
      result.status = InputRoutingRecipientRetirementStatus::kDetached;
    } else {
      for (const auto& epoch : state->data.recipient_generations) {
        if (SameRecipient(epoch.recipient, recipient))
          cutoff = std::max(cutoff, epoch.generation);
      }
      result.status = cutoff == 0
          ? InputRoutingRecipientRetirementStatus::kNotPublished
          : InputRoutingRecipientRetirementStatus::kHistorical;
    }
    if (cutoff == 0) return result;
    result.ticket = InputRoutingTicketAccess::Make(
        state, recipient->id, cutoff, recipient->originalendpoint, recipient);
    result.changed = RetireInputRoutingRecipientLocked(result.ticket, domain);
  }
  if (result.changed)
    NotifyOutsideLocks(state, InputRoutingNotificationKind::kRetirement,
                       result.ticket.ConsumerId(), result.ticket.CutoffGeneration(),
                       result.ticket.OriginalEndpoint());
  return result;
}

InputRoutingNotificationSubscriptionHandle SubscribeInputRoutingNotifications(
    const InputRoutingHandle& state,
    InputRoutingNotificationCallbacks callbacks) {
  if (state == nullptr || callbacks.on_notification == nullptr ||
      callbacks.context_token.expired())
    return {};
  InputRoutingNotificationSubscriptionHandle subscription;
  try {
    subscription = std::make_shared<InputRoutingNotificationSubscription>();
    subscription->owner = state;
    subscription->callbacks = std::move(callbacks);
    auto link = std::make_shared<InputRoutingNotificationLink>();
    link->subscription = subscription;
    std::lock_guard<std::mutex> lock(state->data.mutex);
    auto& head = state->data.notification_subscriptions;
    while (head != nullptr && head->subscription.expired()) head = head->next;
    link->next = head;
    head = std::move(link);
  } catch (...) {
    return {};
  }
  return subscription;
}

InputRoutingRetirementQuery QueryInputRoutingRetirement(
    const InputRoutingRetirementTicket& ticket) {
  InputRoutingRetirementQuery result;
  const auto state = ticket.Routing();
  if (state == nullptr || ticket.ConsumerId() == 0 || ticket.CutoffGeneration() == 0) {
    result.status = InputRoutingRetirementStatus::kTerminal;
    return result;
  }
  std::lock_guard<std::mutex> lock(state->data.mutex);
  result.revision = state->data.revision;
  const bool terminal = routing_internal::IsRoutingEndpointTerminatedLocked(
      state->data, ticket.OriginalEndpoint());
  const auto actions = routing_internal::QueryRoutingActionLedgerLocked(
      state->data, ticket);

  result.admitted_send = actions.admitted;
  if (terminal) result.status = InputRoutingRetirementStatus::kTerminal;
  else if (actions.admitted) result.status = InputRoutingRetirementStatus::kAdmittedSend;
  else if (actions.exact_action && !actions.runnable)
    result.status = InputRoutingRetirementStatus::kFifoHeadBlocked;
  else if (actions.runnable) result.status = InputRoutingRetirementStatus::kRunnable;
  else if (actions.exact_action)
    result.status = InputRoutingRetirementStatus::kTransportCapacity;
  else if (actions.unallocated_cancellation)
    result.status = actions.fifo_empty
                        ? InputRoutingRetirementStatus::kRunnable
                        : InputRoutingRetirementStatus::kFifoHeadBlocked;
  else
    result.status = InputRoutingRetirementStatus::kSettled;
  return result;
}

InputRoutingRetirementQuery QueryInputRoutingRetirement(
    const InputRoutingRetirementTicket& ticket,
    const InputRoutingNotificationSubscriptionHandle& subscription) {
  // The subscription argument documents the required ordering: callers
  // subscribe first, then query the revision they will observe.
  if (subscription == nullptr) {
    InputRoutingRetirementQuery result;
    result.status = InputRoutingRetirementStatus::kTerminal;
    return result;
  }
  return QueryInputRoutingRetirement(ticket);
}

InputRoutingHandle CreateInputRoutingState() {
  auto state = std::make_shared<InputRoutingState>();
  state->data.recipient_generations.reserve(1);
  auto domain = LockInputRoutingDomain();
  domain.Register(state);
  return state;
}

ReceiverId SetInputRoutingConsumer(const InputRoutingHandle& state,
                                   ReceiverId consumer_id,
                                   InputRoutingEndpointHandle endpoint,
                                   ReceiverId expected_current_id) {
  if (state == nullptr) return 0;
  ReceiverId replaced = 0;
  {
    std::lock_guard<std::mutex> lock(state->data.mutex);
    replaced = state->data.consumer_id;
    if (expected_current_id != 0 && replaced != expected_current_id) return 0;
    // Preserve the historical compatibility meaning of an omitted endpoint
    // for an idempotent update. New publications always carry an explicit
    // immutable endpoint token.
    if (consumer_id != 0 && consumer_id == replaced && endpoint == nullptr)
      return replaced;
  }
  if (consumer_id == 0) {
    InputRoutingRetirementTicket ticket;
    bool changed = false;
    {
      auto domain = LockInputRoutingDomain();
      std::lock_guard<std::mutex> lock(state->data.mutex);
      if (expected_current_id != 0 &&
          state->data.consumer_id != expected_current_id) return 0;
      ticket = InputRoutingTicketAccess::Make(state, state->data.consumer_id,
          state->data.generation, state->data.endpoint, state->data.recipient);
      if (ticket.ConsumerId() != 0) changed = RetireInputRoutingRecipientLocked(ticket, domain);
    }
    if (changed)
      NotifyOutsideLocks(state, InputRoutingNotificationKind::kRetirement,
                         ticket.ConsumerId(), ticket.CutoffGeneration(), ticket.OriginalEndpoint());
    return changed ? ticket.ConsumerId() : 0;
  }
  auto recipient = PrepareInputRoutingRecipient(state, consumer_id,
                                                std::move(endpoint));
  if (recipient == nullptr) return 0;
  // Return the predecessor actually replaced by this transaction, not the
  // stale ID observed before preparation or a rejected publication's ID.
  return PublishInputRoutingRecipient(recipient, expected_current_id).predecessor.ConsumerId();
}

bool RetireInputRoutingConsumer(const InputRoutingHandle& state,
                                ReceiverId consumer_id) {
  if (state == nullptr || consumer_id == 0) return false;
  InputRoutingRecipientHandle recipient;
  {
    std::lock_guard<std::mutex> lock(state->data.mutex);
    if (state->data.consumer_id == consumer_id) {
      recipient = state->data.recipient;
    } else {
      // A conditional Set(0) may already have detached this publication.
      // Reuse the retained exact epoch, never the successor/current endpoint.
      for (auto it = state->data.recipient_generations.rbegin();
           it != state->data.recipient_generations.rend(); ++it) {
        if (it->recipient != nullptr && it->recipient->id == consumer_id) {
          recipient = it->recipient;
          break;
        }
      }
    }
  }
  if (recipient == nullptr) return false;
  return RetireInputRoutingRecipient(recipient).changed;
}

bool InputRoutingConsumerMatches(const InputRoutingHandle& state,
                                 ReceiverId consumer_id) {
  if (state == nullptr || consumer_id == 0) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return state->data.consumer_id == consumer_id;
}

bool IsInputRoutingRecipientCurrent(
    const InputRoutingHandle& state,
    const InputRoutingRecipientHandle& recipient) {
  if (state == nullptr || recipient == nullptr || recipient->Routing() != state)
    return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return state->data.consumer_id == recipient->id &&
         SameRecipient(state->data.recipient, recipient) &&
         state->data.endpoint == recipient->originalendpoint;
}

bool InputRoutingTransportReady(const InputRoutingHandle& state) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return state->data.transport_ready;
}

void SetInputRoutingTransportReady(const InputRoutingHandle& state,
                                   ReceiverId consumer_id, bool ready) {
  if (state == nullptr) return;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> lock(state->data.mutex);
    if (state->data.consumer_id != consumer_id ||
        state->data.transport_ready == ready)
      return;
    PrepareGenerationAdvanceLocked(&state->data);
    AdvancePreparedGenerationLocked(&state->data);
    state->data.transport_ready = ready;
    (void)routing_internal::RetireActiveRoutingStreamLocked(&state->data);
    domain.ClearCapture(state);
  }
}

InputRoutingEndpointHandle GetInputRoutingEndpoint(
    const InputRoutingHandle& state) {
  if (state == nullptr) return {};
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return state->data.endpoint;
}

void TerminateInputRoutingTransport(
    const InputRoutingHandle& state,
    const InputRoutingEndpointHandle& endpoint) {
  if (state == nullptr || endpoint == nullptr) return;
  bool cleaned = false;
  ReceiverId notification_id = 0;
  uint64_t notification_generation = 0;
  {
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> lock(state->data.mutex);
  const bool known_endpoint =
      routing_internal::HasRoutingEndpointReferencesLocked(state->data, endpoint);
  if (!known_endpoint) return;
  const bool current_endpoint = SameEndpoint(state->data.endpoint, endpoint);
  if (current_endpoint) PrepareGenerationAdvanceLocked(&state->data);
  cleaned = true;
  (void)routing_internal::MarkRoutingEndpointTerminatedLocked(&state->data, endpoint);

  if (current_endpoint) {
    AdvancePreparedGenerationLocked(&state->data);
    routing_internal::RevokeFocusCacheLocked(&state->data);
    state->data.transport_ready = false;
    domain.ClearFocus(state);
    domain.ClearCapture(state);
  }

  // The action owner retires streams and non-admitted actions for this
  // exact endpoint. The local packet FIFO remains owned here.
  routing_internal::TerminateRoutingActionsForEndpointLocked(&state->data,
                                                              endpoint);
  state->data.packets.erase(
      std::remove_if(state->data.packets.begin(), state->data.packets.end(),
                     [&](const auto& packet) {
                       if (packet.claim_id != 0) return false;
                       return std::any_of(
                           state->data.recipient_generations.begin(),
                           state->data.recipient_generations.end(),
                           [&](const auto& retired) {
                             return retired.generation == packet.generation &&
                                    retired.recipient != nullptr &&
                                    retired.recipient->id == packet.consumer_id &&
                                    SameEndpoint(retired.recipient->originalendpoint,
                                                 endpoint);
                           });
                     }),
      state->data.packets.end());
  state->data.pending_input.store(!state->data.packets.empty(),
                                  std::memory_order_release);
  notification_id = state->data.consumer_id;
  notification_generation = state->data.generation;
  }
  if (cleaned)
    NotifyOutsideLocks(state, InputRoutingNotificationKind::kTerminalCleanup,
                       notification_id, notification_generation,
                       endpoint);
}

bool PublishInputRoutingWmsFrame(const InputRoutingHandle& state, int32_t left,
                                int32_t top, int32_t right, int32_t bottom,
                                bool visible) {
  if (state == nullptr) return false;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> lock(state->data.mutex);
    auto next_window = state->data.window;
    const auto transition = next_window.PublishWmsFrame(
        {left, top, right, bottom}, visible);
    const bool revoke = transition.Revoked();
    if (revoke) {
      PrepareGenerationAdvanceLocked(&state->data);
      AdvancePreparedGenerationLocked(&state->data);
      routing_internal::RevokeFocusCacheLocked(&state->data);
    }
    state->data.window = next_window;
    if (!transition.is_eligible) {
      domain.ClearFocus(state);
      domain.ClearCapture(state);
      (void)routing_internal::RetireActiveRoutingStreamLocked(&state->data);
    }
    return revoke;
  }
}

bool UpdateInputRoutingReceiverGeometry(const InputRoutingHandle& state,
                                        int32_t left, int32_t top,
                                        int32_t right, int32_t bottom,
                                        ReceiverId expected_id) {
  if (state == nullptr || expected_id == 0) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (state->data.consumer_id != expected_id) return false;
  return state->data.window.UpdateReceiverFrame({left, top, right, bottom});
}

bool EnqueueInputRoutingPacket(
    const InputRoutingHandle& state,
    const darwin_art::DarwinArtInputPacket& packet, ReceiverId expected_id) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (expected_id != 0 && state->data.consumer_id != expected_id) return false;
  return QueuePacketLocked(&state->data, packet,
                           expected_id == 0 ? state->data.consumer_id : expected_id,
                           state->data.generation);
}

bool DequeueInputRoutingPacketEnvelope(
    const InputRoutingHandle& state, InputRoutingPacketEnvelope* envelope,
    ReceiverId expected_id) {
  if (state == nullptr || envelope == nullptr) return false;
  // Pin the owner and copy epoch metadata before observers can reenter or
  // release their caller's routing handle / envelope.
  const auto owner = state;
  bool released_capacity = false;
  ReceiverId id = 0;
  uint64_t generation = 0;
  {
    std::lock_guard<std::mutex> lock(owner->data.mutex);
    if (owner->data.packets.empty()) {
      owner->data.pending_input.store(false, std::memory_order_release);
      return false;
    }
    if (expected_id != 0 && owner->data.packets.front().consumer_id != 0 &&
        owner->data.packets.front().consumer_id != expected_id)
      return false;
    const auto& queued = owner->data.packets.front();
    // A packet lease owns this head until explicit completion. Legacy dequeue
    // cannot steal it and defeat deferred invocation.
    if (queued.claim_id != 0) return false;
    envelope->packet = queued.packet;
    envelope->consumer_id = id = queued.consumer_id;
    envelope->generation = generation = queued.generation;
    released_capacity = owner->data.packets.size() == routing_internal::kMaxRoutingPackets;
    owner->data.packets.pop_front();
    owner->data.pending_input.store(!owner->data.packets.empty(),
                                    std::memory_order_release);
  }
  if (released_capacity)
    NotifyOutsideLocks(owner, InputRoutingNotificationKind::kLocalCapacity,
                       id, generation, nullptr);
  return true;
}

bool RequeueInputRoutingPacketEnvelopeFront(
    const InputRoutingHandle& state,
    const InputRoutingPacketEnvelope& envelope) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  // A non-destructive packet lease owns the head until it completes or
  // rolls back. Legacy requeue cannot insert a duplicate in front of it.
  if (!state->data.packets.empty() &&
      state->data.packets.front().claim_id != 0)
    return false;
  if (envelope.consumer_id != state->data.consumer_id ||
      envelope.generation != state->data.generation ||
      IsRetiredRecipientLocked(state->data, envelope.consumer_id,
                               envelope.generation))
    return false;
  if (state->data.packets.size() >= routing_internal::kMaxRoutingPackets) return false;
  state->data.packets.push_front(
      {envelope.packet, envelope.consumer_id, envelope.generation,
       state->data.recipient, 0, {}});
  state->data.pending_input.store(true, std::memory_order_release);
  return true;
}

bool DequeueInputRoutingPacket(const InputRoutingHandle& state,
                               darwin_art::DarwinArtInputPacket* packet,
                               ReceiverId expected_id) {
  if (packet == nullptr) return false;
  InputRoutingPacketEnvelope envelope;
  if (!DequeueInputRoutingPacketEnvelope(state, &envelope, expected_id))
    return false;
  *packet = envelope.packet;
  return true;
}

bool RequeueInputRoutingPacketFront(
    const InputRoutingHandle& state,
    const darwin_art::DarwinArtInputPacket& packet, ReceiverId expected_id) {
  InputRoutingPacketEnvelope envelope;
  envelope.packet = packet;
  envelope.consumer_id = expected_id;
  if (state != nullptr) {
    {
      std::lock_guard<std::mutex> lock(state->data.mutex);
      envelope.generation = state->data.generation;
      if (envelope.consumer_id == 0)
        envelope.consumer_id = state->data.consumer_id;
    }
  }
  return RequeueInputRoutingPacketEnvelopeFront(state, envelope);
}

bool InputRoutingHasPending(const InputRoutingHandle& state) {
  if (state == nullptr) return false;
  if (state->data.pending_input.load(std::memory_order_acquire)) return true;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return routing_internal::HasPendingRoutingActionOrCancellationLocked(state->data);
}

bool HasInputRoutingPackets(const InputRoutingHandle& state) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return !state->data.packets.empty();
}

bool InputRoutingHasRunnableAction(const InputRoutingHandle& state) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return routing_internal::IsRoutingActionHeadRunnableLocked(state->data);
}

bool RetryInputRoutingCancellations(const InputRoutingHandle& state) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  return routing_internal::RetryRoutingCancellationsLocked(&state->data);
}

void ClearInputRoutingPending(const InputRoutingHandle& state) {
  if (state == nullptr) return;
  std::lock_guard<std::mutex> lock(state->data.mutex);
  if (state->data.packets.empty() &&
      !routing_internal::HasRoutingActionsLocked(state->data))
    state->data.pending_input.store(false, std::memory_order_release);
}

InputRoutingSelectionSnapshot SnapshotInputRoutingSelection(const InputRoutingHandle& channel) {
  if (!channel) return {};
  std::lock_guard<std::mutex> lock(channel->data.mutex);
  const auto& data = channel->data;
  return {data.window.Eligible() && data.consumer_id != 0 &&
              !routing_internal::IsRoutingEndpointTerminatedLocked(data, data.endpoint),
          data.window.Frame(), data.focus_order, data.generation,
          data.consumer_id, data.recipient, data.endpoint, data.transport_ready,
          data.focus_cache.focus_ready, data.focus_cache.grant,
          data.focus_cache.revoked, data.focus_cache.focus_epoch,
          data.focus_cache.revision, data.recipient,
          data.focus_cache.ready_recipient,
          data.focus_cache.ready_epoch, data.focus_cache.ready_cache_revision,
          data.focus_cache.present};
}

bool ValidateInputRoutingSelection(const InputRoutingHandle& channel,
    const InputRoutingSelectionSnapshot& selection, InputRoutingAdmission* admission) {
  if (!channel || !admission || !selection.eligible) return false;
  std::lock_guard<std::mutex> lock(channel->data.mutex);
  const auto& data = channel->data;
  if (!data.window.Eligible() || data.consumer_id == 0 ||
      routing_internal::IsRoutingEndpointTerminatedLocked(data, data.endpoint) ||
      data.generation != selection.generation || data.consumer_id != selection.consumer_id ||
      data.endpoint != selection.endpoint || data.recipient != selection.recipient) return false;
  admission->state = channel;
  admission->generation = data.generation;
  admission->consumer_id = data.consumer_id;
  admission->endpoint = data.endpoint;
  admission->local_transport_ready = data.transport_ready;
  return true;
}

namespace routing_internal {
void PrepareFocusGenerationLocked(InputRoutingStateData* data) {
  PrepareGenerationAdvanceLocked(data);
}
bool HasPendingRecipientPacketsLocked(
    const InputRoutingStateData& data,
    const InputRoutingRecipientHandle& recipient) {
  return std::any_of(data.packets.begin(), data.packets.end(),
                     [&](const auto& packet) {
                       return packet.recipient == recipient;
                     });
}
void AdvanceFocusGenerationLocked(InputRoutingStateData* data, bool retire_stream) {
  if (retire_stream) (void)routing_internal::RetireActiveRoutingStreamLocked(data);
  AdvancePreparedGenerationLocked(data);
}
bool FocusEndpointTerminatedLocked(const InputRoutingStateData& data) {
  return routing_internal::IsRoutingEndpointTerminatedLocked(data, data.endpoint);
}
}  // namespace routing_internal
}  // namespace darwin_art::input
