#include "input_routing_focus.h"
#include "input_routing_domain.h"
#include "input_routing_state_internal.h"

#include <algorithm>
#include <mutex>

namespace darwin_art::input {
namespace {
using Evaluator = InputFocusEpochEvaluator;
using Data = routing_internal::InputRoutingStateData;

bool SameRecipient(const InputRoutingRecipientHandle& left,
                   const InputRoutingRecipientHandle& right) noexcept {
  return left != nullptr && right != nullptr && left.get() == right.get();
}

Evaluator::Route CurrentRoute(const InputRoutingHandle& state,
                              const InputRoutingRecipientHandle& expected,
                              const Data& data) noexcept {
  const bool exact = SameRecipient(data.recipient, expected) &&
                     data.consumer_id == expected->id &&
                     data.endpoint == expected->originalendpoint;
  return {state, expected, exact,
          exact && !routing_internal::FocusEndpointTerminatedLocked(data),
          exact && data.window.Eligible()};
}

void SetResultIdentity(InputRoutingFocusResult* result,
                       const InputRoutingRecipientHandle& expected,
                       FocusControl control) noexcept {
  if (result == nullptr) return;
  result->recipient = expected;
  result->control = control;
}

InputRoutingFocusResult ApplyLocked(
    const InputRoutingRecipientHandle& expected, FocusControl control,
    bool replay) {
  InputRoutingFocusResult result;
  SetResultIdentity(&result, expected, control);
  if (expected == nullptr || expected->id == 0) return result;
  const auto state = expected->Routing();
  if (state == nullptr) return result;

  auto domain = LockInputRoutingDomain();
  const auto previous = domain.Focused();
  std::unique_lock<std::mutex> state_lock(state->data.mutex,
                                          std::defer_lock);
  std::unique_lock<std::mutex> previous_lock;
  if (previous != nullptr && previous != state) {
    previous_lock = std::unique_lock<std::mutex>(previous->data.mutex,
                                                 std::defer_lock);
    // Lock both channels before any preparation.  Domain serialization plus
    // std::lock preserves the domain -> channel order and avoids A/B vs B/A
    // handoff deadlocks.
    std::lock(state_lock, previous_lock);
  } else {
    state_lock.lock();
  }
  // A recipient token is the authority.  Never reconstruct it from the
  // mutable numeric consumer id or endpoint fields.
  if (!SameRecipient(state->data.recipient, expected) ||
      state->data.consumer_id != expected->id ||
      state->data.endpoint != expected->originalendpoint) {
    result.decision = Evaluator::Decision::kRejectedNotCurrentRecipient;
    return result;
  }
  // BeforeControl drains the FIFO as a fast path, but another producer can
  // enqueue an earlier local packet immediately afterward.  Keep the domain
  // and channel locks through this check so focus cannot overtake that exact
  // recipient's packet; the next owner-Looper turn retries the control.
  const bool pending_local = HasPendingRecipientPacketsLocked(state->data, expected);
  if (pending_local) {
    result.decision = Evaluator::Decision::kDeferredPendingLocalInput;
    return result;
  }

  const auto route = CurrentRoute(state, expected, state->data);
  Evaluator evaluator;
  Evaluator::Evaluation evaluation;
  const auto decision = replay
      ? evaluator.EvaluateReplay(route, route, domain.FocusEpochDomain(),
                                 state->data.focus_cache, &evaluation)
      : evaluator.Evaluate(control, route, route, domain.FocusEpochDomain(),
                           state->data.focus_cache, &evaluation);
  result.decision = decision;
  if (!evaluation.Accepted() || decision == Evaluator::Decision::kDuplicate)
    return result;

  // Capture() pins its owner while this transaction is alive.  Besides
  // preserving lifetime through unlock, doing this before Commit keeps every
  // potentially-fallible domain preparation ahead of the authoritative write.
  (void)domain.Captured();
  if (decision == Evaluator::Decision::kAcceptedGain ||
      decision == Evaluator::Decision::kAcceptedReplay) {
    if (previous != state) {
      routing_internal::PrepareFocusGenerationLocked(&state->data);
      if (previous != nullptr)
        routing_internal::PrepareFocusGenerationLocked(&previous->data);
    }
  } else if (previous == state) {
    routing_internal::PrepareFocusGenerationLocked(&state->data);
  }
  // Replay's epoch is read from the cache plan, not from its zero-valued
  // transport argument.  The exact value is carried through JNI commit.
  result.control = FocusControl{evaluation.plan.epoch, evaluation.plan.focused};

  // All fallible reservations have completed.  Commit is allocation-free and
  // the domain/channel locks serialize it with replacement and revocation.
  if (!evaluator.Commit(domain.FocusEpochDomain(), state->data.focus_cache,
                        std::move(evaluation.plan))) {
    result.decision = Evaluator::Decision::kRejectedLedgerPreparation;
    return result;
  }
  result.cache_revision = state->data.focus_cache.revision;

  if (decision == Evaluator::Decision::kAcceptedGain ||
      decision == Evaluator::Decision::kAcceptedReplay) {
    if (previous != state) {
      if (previous != nullptr) {
        domain.SetFocused(state);
        domain.ClearCapture(previous);
        routing_internal::AdvanceFocusGenerationLocked(&previous->data, true);
        routing_internal::AdvanceFocusGenerationLocked(&state->data, false);
        state->data.focus_order = domain.NextFocusOrder();
      } else {
        domain.SetFocused(state);
        routing_internal::AdvanceFocusGenerationLocked(&state->data, false);
        state->data.focus_order = domain.NextFocusOrder();
      }
    }
  } else if (previous == state) {
    domain.ClearFocus(state);
    routing_internal::AdvanceFocusGenerationLocked(&state->data, true);
  }

  result.notification_present = evaluation.ShouldNotify();
  return result;
}
}  // namespace

InputRoutingFocusResult ApplyInputRoutingFocusControl(
    const InputRoutingRecipientHandle& expected, FocusControl control) {
  return ApplyLocked(expected, control, false);
}

InputRoutingFocusResult ReplayCachedInputRoutingFocus(
    const InputRoutingRecipientHandle& expected) {
  return ApplyLocked(expected, FocusControl{}, true);
}

namespace {
bool CommitNotificationLocked(
    const InputRoutingRecipientHandle& expected, FocusControl control,
    bool invoked, bool fenced, uint64_t cache_revision) {
  if (!invoked || expected == nullptr || expected->id == 0 || control.epoch == 0)
    return false;
  const auto state = expected->Routing();
  if (state == nullptr) return false;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> state_lock(state->data.mutex);
  if (!SameRecipient(state->data.recipient, expected) ||
      state->data.consumer_id != expected->id ||
      state->data.endpoint != expected->originalendpoint)
    return false;
  if (fenced && state->data.focus_cache.revision != cache_revision)
    return false;

  Evaluator::Notification notification;
  notification.present = true;
  notification.channel = state;
  notification.recipient = expected;
  notification.epoch = control.epoch;
  notification.focused = control.focused;
  return Evaluator::CommitNotification(state->data.focus_cache, notification);
}
}  // namespace

bool CommitInputRoutingFocusNotification(
    const InputRoutingFocusResult& result, bool invoked) {
  return CommitNotificationLocked(result.recipient, result.control, invoked,
                                  true, result.cache_revision);
}

bool CommitInputRoutingFocusReadiness(const InputRoutingFocusResult& result,
                                      bool delivered) {
  if (!result.notification_present || result.recipient == nullptr ||
      result.recipient->id == 0 || result.control.epoch == 0)
    return false;
  const auto state = result.recipient->Routing();
  if (state == nullptr) return false;
  bool committed = false;
  uint64_t generation = 0;
  {
    auto domain = LockInputRoutingDomain();
    std::lock_guard<std::mutex> state_lock(state->data.mutex);
    if (!SameRecipient(state->data.recipient, result.recipient) ||
        state->data.consumer_id != result.recipient->id ||
        state->data.endpoint != result.recipient->originalendpoint)
      return false;
    Evaluator::Notification notification;
    notification.present = true;
    notification.channel = state;
    notification.recipient = result.recipient;
    notification.epoch = result.control.epoch;
    notification.focused = result.control.focused;
    committed = Evaluator::CommitFocusReadiness(
        domain.FocusEpochDomain(), state->data.focus_cache, notification,
        delivered, result.cache_revision);
    generation = state->data.generation;
  }
  if (committed && delivered)
    routing_internal::NotifyInputRoutingState(state,
        InputRoutingNotificationKind::kFocusReadiness, result.recipient->id,
        generation, result.recipient->originalendpoint);
  return committed;
}

bool RevokeInputRoutingFocus(const InputRoutingRecipientHandle& expected,
                             uint64_t revocation_epoch) {
  if (expected == nullptr || expected->id == 0) return false;
  const auto state = expected->Routing();
  if (state == nullptr) return false;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> state_lock(state->data.mutex);
  if (!SameRecipient(state->data.recipient, expected) ||
      state->data.consumer_id != expected->id ||
      state->data.endpoint != expected->originalendpoint)
    return false;
  if (!state->data.focus_cache.present) return false;
  return Evaluator::Revoke(state->data.focus_cache, revocation_epoch);
}

namespace routing_internal {
bool RevokeFocusCacheLocked(InputRoutingStateData* data,
                            uint64_t revocation_epoch) noexcept {
  if (data == nullptr || !data->focus_cache.present) return false;
  return Evaluator::Revoke(data->focus_cache, revocation_epoch);
}
}  // namespace routing_internal

using routing_internal::PrepareFocusGenerationLocked;
using routing_internal::AdvanceFocusGenerationLocked;

// Focus execution owns domain/channel linearization. Packet history and
// cancellation mutation remain behind the ledger owner's prepared interface.
bool SetInputRoutingFocus(const InputRoutingHandle& state, ReceiverId expected_id) {
  if (state == nullptr) return false;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> state_lock(state->data.mutex);
  if (expected_id != 0 && state->data.consumer_id != expected_id) return false;
  if (!state->data.window.Eligible() || state->data.consumer_id == 0 ||
      routing_internal::FocusEndpointTerminatedLocked(state->data)) return false;
  auto previous = domain.Focused();
  if (previous != state) {
    PrepareFocusGenerationLocked(&state->data);
    if (previous != nullptr) {
      std::lock_guard<std::mutex> previous_lock(previous->data.mutex);
      PrepareFocusGenerationLocked(&previous->data);
    }
  }
  domain.SetFocused(state);
  if (previous != nullptr && previous != state) {
    domain.ClearCapture(previous);
    std::lock_guard<std::mutex> previous_lock(previous->data.mutex);
    AdvanceFocusGenerationLocked(&previous->data, true);
  }
  if (previous != state) AdvanceFocusGenerationLocked(&state->data, false);
  state->data.focus_order = domain.NextFocusOrder();
  return true;
}

bool ClearInputRoutingFocus(const InputRoutingHandle& state, ReceiverId expected_id) {
  if (state == nullptr) return false;
  auto domain = LockInputRoutingDomain();
  std::lock_guard<std::mutex> state_lock(state->data.mutex);
  if (expected_id != 0 && state->data.consumer_id != expected_id) return false;
  if (domain.Focused() == state) {
    PrepareFocusGenerationLocked(&state->data);
    domain.ClearFocus(state);
    AdvanceFocusGenerationLocked(&state->data, true);
  }
  domain.ClearCapture(state);
  return true;
}
}  // namespace darwin_art::input
