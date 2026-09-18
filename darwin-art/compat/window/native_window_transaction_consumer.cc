#include "native_window_transaction_consumer.h"

#include <algorithm>
#include <atomic>
#include <deque>
#include <limits>
#include <memory>
#include <mutex>
#include <optional>
#include <unordered_map>
#include <utility>
#include <vector>

namespace darwin_art::window {
namespace {

constexpr int kQuarantineFence = -2;
std::atomic<uint64_t> next_submission_cookie{1};

bool AllocateSubmissionCookie(uint64_t* cookie) {
  if (cookie == nullptr) return false;
  uint64_t current = next_submission_cookie.load(std::memory_order_relaxed);
  while (current != 0 && current != std::numeric_limits<uint64_t>::max()) {
    if (next_submission_cookie.compare_exchange_weak(
            current, current + 1, std::memory_order_relaxed,
            std::memory_order_relaxed)) {
      *cookie = current;
      return true;
    }
  }
  return false;
}

struct LayerGroup;
struct RetainedControlGroup;

struct PendingFrame {
  enum class Disposition : uint8_t { kPreparing, kCallbackArmed, kSubmitted };
  NativeWindowTransactionFrame frame;
  std::shared_ptr<LayerGroup> group;
  std::shared_ptr<RetainedControlGroup> control;
  uint64_t submission_cookie = 0;
  Disposition disposition = Disposition::kPreparing;
  bool quarantined = false;
};

struct LayerIdentity {
  bool stable = false;
  uint32_t owner_process = 0;
  uint32_t layer = 0;
  ASurfaceControl* fallback = nullptr;

  bool operator==(const LayerIdentity& other) const {
    if (stable != other.stable) return false;
    return stable ? owner_process == other.owner_process &&
                         layer == other.layer
                  : fallback == other.fallback;
  }
};

struct LayerIdentityHash {
  size_t operator()(const LayerIdentity& value) const noexcept {
    if (value.stable) {
      return (static_cast<size_t>(value.owner_process) << 32) ^
             static_cast<size_t>(value.layer);
    }
    return std::hash<ASurfaceControl*>{}(value.fallback);
  }
};

struct LayerGroup {
  explicit LayerGroup(LayerIdentity value) : identity(value) {}
  LayerIdentity identity;
};

struct RetainedControlGroup {
  NativeWindowTransactionConsumerHooks hooks;
  ASurfaceControl* control = nullptr;

  ~RetainedControlGroup() {
    if (control != nullptr && hooks.release_control != nullptr)
      hooks.release_control(hooks.context, control);
  }
};

struct OperationPin {
  NativeWindowTransactionConsumerHooks hooks;
  bool retained = false;

  explicit OperationPin(NativeWindowTransactionConsumerHooks value)
      : hooks(value) {
    if (hooks.retain_owner != nullptr) {
      hooks.retain_owner(hooks.context);
      retained = true;
    }
  }

  ~OperationPin() { Release(); }

  void Release() {
    if (!retained) return;
    retained = false;
    if (hooks.release_owner != nullptr) hooks.release_owner(hooks.context);
  }
};

struct Completion {
  NativeWindowTransactionConsumerHooks hooks;
  std::shared_ptr<NativeWindowTransactionConsumer::State> state;
  NativeWindowTransactionFrame frame;
  std::shared_ptr<LayerGroup> group;
  std::shared_ptr<RetainedControlGroup> control;
  std::shared_ptr<void> observer_lifetime;
  uint64_t submission_cookie = 0;
  bool owner_retained = false;
};

void ReleaseCompletion(Completion* completion) {
  if (completion == nullptr) return;
  const NativeWindowTransactionConsumerHooks hooks = completion->hooks;
  const bool owner_retained = completion->owner_retained;
  // Destroy every callback-owned resource, including the shared state and
  // control lease, before dropping the producer owner pin. A release hook may
  // synchronously destroy the producer, so no completion member may remain
  // live across that call.
  delete completion;
  if (owner_retained && hooks.release_owner != nullptr)
    hooks.release_owner(hooks.context);
}

}  // namespace

struct NativeWindowTransactionConsumer::State {
  explicit State(NativeWindowTransactionConsumerHooks value) noexcept
      : hooks(value) {}

  NativeWindowTransactionConsumerHooks hooks;
  std::mutex mutex;
  std::unordered_map<LayerIdentity, std::shared_ptr<LayerGroup>,
                     LayerIdentityHash>
      groups;
  std::unordered_map<LayerIdentity, std::deque<PendingFrame>,
                     LayerIdentityHash>
      pending;
};

namespace {

LayerIdentity IdentifyLayer(const NativeWindowTransactionConsumerHooks& hooks,
                            ASurfaceControl* control) {
  uint32_t owner_process = 0;
  uint32_t layer = 0;
  if (hooks.get_layer_identity != nullptr &&
      hooks.get_layer_identity(hooks.context, control, &owner_process, &layer)) {
    return {.stable = true,
            .owner_process = owner_process,
            .layer = layer,
            .fallback = nullptr};
  }
  return {.stable = false,
          .owner_process = 0,
          .layer = 0,
          .fallback = control};
}

std::shared_ptr<RetainedControlGroup> RetainControl(
    const NativeWindowTransactionConsumerHooks& hooks,
    const NativeWindowTransactionFrame& frame) {
  bool acquired = frame.control_retained;
  if (!acquired && hooks.acquire_control != nullptr) {
    hooks.acquire_control(hooks.context, frame.control);
    acquired = true;
  }
  try {
    auto result = std::make_shared<RetainedControlGroup>();
    result->hooks = hooks;
    result->control = frame.control;
    return result;
  } catch (...) {
    if (acquired && hooks.release_control != nullptr)
      hooks.release_control(hooks.context, frame.control);
    return nullptr;
  }
}

void FinishOperation(std::shared_ptr<void>* lifetime) {
  if (lifetime != nullptr) lifetime->reset();
}

void ErasePending(const std::shared_ptr<NativeWindowTransactionConsumer::State>&
                     state,
                 const LayerIdentity& identity,
                 const std::shared_ptr<LayerGroup>& group,
                 const NativeWindowTransactionFrame& frame) {
  if (state == nullptr) return;
  std::lock_guard<std::mutex> lock(state->mutex);
  auto found = state->pending.find(identity);
  if (found == state->pending.end()) return;
  auto& frames = found->second;
  for (auto it = frames.begin(); it != frames.end(); ++it) {
    if (it->group == group && it->frame.slot == frame.slot &&
        it->frame.generation == frame.generation &&
        it->frame.frame == frame.frame) {
      frames.erase(it);
      break;
    }
  }
  if (frames.empty()) {
    state->pending.erase(found);
    state->groups.erase(identity);
  }
}

bool SetDisposition(
    const std::shared_ptr<NativeWindowTransactionConsumer::State>& state,
    const LayerIdentity& identity, const std::shared_ptr<LayerGroup>& group,
    const NativeWindowTransactionFrame& frame,
    PendingFrame::Disposition disposition) {
  if (state == nullptr) return false;
  std::lock_guard<std::mutex> lock(state->mutex);
  auto found = state->pending.find(identity);
  if (found == state->pending.end()) return false;
  for (auto& pending : found->second) {
    if (pending.group == group && pending.frame.slot == frame.slot &&
        pending.frame.generation == frame.generation &&
        pending.frame.frame == frame.frame) {
      pending.disposition = disposition;
      return true;
    }
  }
  return false;
}

void ReturnFrame(Completion* completion, int fence, bool quarantine) {
  if (completion == nullptr) return;
  if (completion->hooks.return_frame != nullptr)
    completion->hooks.return_frame(completion->hooks.context,
                                   completion->frame, fence, quarantine);
}

void ReturnUnownedFrame(NativeWindowTransactionConsumer::State* state,
                        const NativeWindowTransactionFrame& frame, int fence,
                        bool quarantine) {
  if (state == nullptr) return;
  if (state->hooks.return_frame != nullptr) {
    state->hooks.return_frame(state->hooks.context, frame, fence, quarantine);
  } else if (!quarantine && state->hooks.close_fence != nullptr) {
    state->hooks.close_fence(state->hooks.context, fence);
  }
  if (frame.control_retained && state->hooks.release_control != nullptr)
    state->hooks.release_control(state->hooks.context, frame.control);
}

void OnDiscard(void* opaque, int fence) {
  auto* completion = static_cast<Completion*>(opaque);
  if (completion == nullptr) return;
  bool found_current = false;
  const LayerIdentity* identity =
      completion->group == nullptr ? nullptr : &completion->group->identity;
  if (completion->state != nullptr && identity != nullptr) {
    std::lock_guard<std::mutex> lock(completion->state->mutex);
    auto found = completion->state->pending.find(*identity);
    if (found != completion->state->pending.end()) {
      auto& frames = found->second;
      for (auto it = frames.begin(); it != frames.end(); ++it) {
        if (it->group == completion->group &&
            it->frame.slot == completion->frame.slot &&
            it->frame.generation == completion->frame.generation &&
            it->frame.frame == completion->frame.frame) {
          found_current = true;
          if (fence == kQuarantineFence) {
            it->quarantined = true;
          } else {
            frames.erase(it);
          }
          break;
        }
      }
      if (frames.empty()) {
        completion->state->pending.erase(found);
        completion->state->groups.erase(*identity);
      }
    }
  }
  if (found_current) {
    ReturnFrame(completion, fence, fence == kQuarantineFence);
  } else if (completion->hooks.close_fence != nullptr) {
    // A late/duplicate callback no longer owns a producer frame. It still
    // owns its callback fence and must consume that descriptor exactly once.
    completion->hooks.close_fence(completion->hooks.context, fence);
  }
  ReleaseCompletion(completion);
}

void OnComplete(void* opaque, ASurfaceTransactionStats* stats) {
  auto* completion = static_cast<Completion*>(opaque);
  if (completion == nullptr) return;
  std::optional<PendingFrame> retired;
  const LayerIdentity* identity =
      completion->group == nullptr ? nullptr : &completion->group->identity;
  auto state = completion->state;
  AHardwareBuffer* previous_buffer = nullptr;
  uint64_t previous_cookie = 0;
  const bool has_previous =
      completion->hooks.previous_buffer_metadata != nullptr &&
      completion->hooks.previous_buffer_metadata(
          completion->hooks.context, stats, completion->frame.control,
          &previous_buffer, &previous_cookie);
  if (state != nullptr && identity != nullptr) {
    std::lock_guard<std::mutex> lock(state->mutex);
    auto found = state->pending.find(*identity);
    if (found != state->pending.end()) {
      auto& frames = found->second;
      if (has_previous && previous_buffer != nullptr && previous_cookie != 0) {
        for (auto it = frames.begin(); it != frames.end(); ++it) {
          if (it->group == completion->group &&
              it->frame.buffer == previous_buffer &&
              it->submission_cookie == previous_cookie &&
              it->disposition == PendingFrame::Disposition::kSubmitted &&
              !it->quarantined) {
            // PendingFrame is movable without allocation. The exact cookie
            // identifies one authority, so retain at most this one record.
            retired.emplace(std::move(*it));
            frames.erase(it);
            break;
          }
        }
      }
      if (frames.empty()) {
        state->pending.erase(found);
        state->groups.erase(*identity);
      }
    }
  }
  if (retired.has_value()) {
    const int fence = completion->hooks.previous_release_fence == nullptr
                          ? -1
                          : completion->hooks.previous_release_fence(
                                completion->hooks.context, stats,
                                completion->frame.control);
    if (completion->hooks.return_frame != nullptr)
      completion->hooks.return_frame(completion->hooks.context,
                                     retired->frame, fence, false);
  }
  retired.reset();
  state.reset();
  ReleaseCompletion(completion);
}

}  // namespace

NativeWindowTransactionConsumer::NativeWindowTransactionConsumer(
    NativeWindowTransactionConsumerHooks hooks)
    : state_(std::make_shared<State>(hooks)) {}

NativeWindowTransactionConsumer::~NativeWindowTransactionConsumer() = default;

bool NativeWindowTransactionConsumer::Submit(
    const NativeWindowTransactionFrame& frame, int acquire_fence,
    NativeWindowTransactionObserver observer, void* observer_context,
    std::shared_ptr<void> observer_lifetime) {
  if (state_ == nullptr) return false;
  // Declare the producer pin before every resource-bearing local. C++ then
  // destroys state/control/group/lifetime locals before OperationPin drops
  // the final owner reference on every return path.
  OperationPin operation(state_->hooks);
  const auto state = state_;
  if (frame.control == nullptr ||
      state->hooks.create_transaction == nullptr ||
      state->hooks.delete_transaction == nullptr ||
      state->hooks.set_buffer_checked == nullptr ||
      state->hooks.set_callbacks_checked == nullptr ||
      state->hooks.return_frame == nullptr) {
    ReturnUnownedFrame(state.get(), frame, acquire_fence, false);
    FinishOperation(&observer_lifetime);
    return false;
  }
  const LayerIdentity identity = IdentifyLayer(state->hooks, frame.control);
  auto control = RetainControl(state->hooks, frame);
  if (control == nullptr) {
    // RetainControl already consumed and released the captured/acquired
    // control reference on allocation failure. Return only slot/fence ownership.
    state->hooks.return_frame(state->hooks.context, frame, acquire_fence, false);
    FinishOperation(&observer_lifetime);
    return false;
  }
  ASurfaceTransaction* transaction =
      state->hooks.create_transaction(state->hooks.context);
  if (transaction == nullptr) {
    state->hooks.return_frame(state->hooks.context, frame, acquire_fence,
                              false);
    control.reset();
    FinishOperation(&observer_lifetime);
    return false;
  }
  uint64_t submission_cookie = 0;
  if (!AllocateSubmissionCookie(&submission_cookie)) {
    state->hooks.delete_transaction(state->hooks.context, transaction);
    state->hooks.return_frame(state->hooks.context, frame, acquire_fence,
                              false);
    control.reset();
    FinishOperation(&observer_lifetime);
    return false;
  }
  auto* completion = new (std::nothrow) Completion{
      .hooks = state->hooks,
      .state = state,
      .frame = frame,
      .control = control,
      .observer_lifetime = std::move(observer_lifetime),
      .submission_cookie = submission_cookie,
      .owner_retained = false};
  if (completion == nullptr) {
    state->hooks.delete_transaction(state->hooks.context, transaction);
    state->hooks.return_frame(state->hooks.context, frame, acquire_fence,
                              false);
    control.reset();
    FinishOperation(&observer_lifetime);
    return false;
  }
  if (state->hooks.retain_owner != nullptr) {
    state->hooks.retain_owner(state->hooks.context);
    completion->owner_retained = true;
  }

  const int rollback_fence = acquire_fence >= 0 &&
                                     state->hooks.duplicate_fence != nullptr
                                 ? state->hooks.duplicate_fence(
                                       state->hooks.context, acquire_fence)
                                 : -1;
  if (!state->hooks.set_buffer_checked(state->hooks.context, transaction,
                                       frame.control, frame.buffer,
                                       acquire_fence,
                                       completion->submission_cookie)) {
    state->hooks.delete_transaction(state->hooks.context, transaction);
    if (rollback_fence >= 0)
      state->hooks.return_frame(state->hooks.context, frame, rollback_fence,
                                false);
    else if (acquire_fence >= 0)
      state->hooks.return_frame(state->hooks.context, frame, -1, true);
    else
      state->hooks.return_frame(state->hooks.context, frame, -1, false);
    ReleaseCompletion(completion);
    FinishOperation(&observer_lifetime);
    return false;
  }
  std::shared_ptr<LayerGroup> group;
  bool group_inserted = false;
  bool pending_inserted = false;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    try {
      auto found_group = state->groups.find(identity);
      if (found_group == state->groups.end()) {
        group = std::make_shared<LayerGroup>(identity);
        state->groups.emplace(identity, group);
        group_inserted = true;
      } else {
        group = found_group->second;
      }
      state->pending[identity].push_back(
          {frame, group, control, completion->submission_cookie,
           PendingFrame::Disposition::kPreparing, false});
      pending_inserted = true;
    } catch (...) {
      // Keep rollback under the same policy lock as publication. If this was
      // our newly inserted group, no competing submit can observe it until
      // the lock is released; remove only that exact empty group.
      if (group_inserted) {
        auto found_pending = state->pending.find(identity);
        if (found_pending != state->pending.end() &&
            found_pending->second.empty())
          state->pending.erase(found_pending);
        auto found_group = state->groups.find(identity);
        if (found_group != state->groups.end() &&
            found_group->second == group)
          state->groups.erase(found_group);
      }
    }
  }
  if (!pending_inserted) {
    state->hooks.delete_transaction(state->hooks.context, transaction);
    if (rollback_fence >= 0)
      state->hooks.return_frame(state->hooks.context, frame, rollback_fence,
                                false);
    else
      state->hooks.return_frame(state->hooks.context, frame, -1,
                                acquire_fence >= 0);
    ReleaseCompletion(completion);
    FinishOperation(&observer_lifetime);
    return false;
  }
  completion->group = group;
  // Publish the typed record before transferring callback ownership. A failed
  // checked registration can therefore remove the record and return the held
  // rollback duplicate without any callback racing an unarmed record.
  if (!state->hooks.set_callbacks_checked(
          state->hooks.context, transaction, frame.control, completion,
          &OnComplete, &OnDiscard)) {
    ErasePending(state, identity, group, frame);
    state->hooks.delete_transaction(state->hooks.context, transaction);
    if (rollback_fence >= 0)
      state->hooks.return_frame(state->hooks.context, frame, rollback_fence,
                                false);
    else
      state->hooks.return_frame(state->hooks.context, frame, -1,
                                acquire_fence >= 0);
    ReleaseCompletion(completion);
    FinishOperation(&observer_lifetime);
    return false;
  }
  (void)SetDisposition(state, identity, group, frame,
                       PendingFrame::Disposition::kCallbackArmed);
  if (rollback_fence >= 0 && state->hooks.close_fence != nullptr)
    state->hooks.close_fence(state->hooks.context, rollback_fence);
  (void)SetDisposition(state, identity, group, frame,
                       PendingFrame::Disposition::kSubmitted);
  const bool observer_accepted =
      observer != nullptr && observer(observer_context, transaction, frame.frame);
  if (observer_accepted) {
    FinishOperation(&observer_lifetime);
    return true;
  }
  if (state->hooks.apply_transaction != nullptr)
    state->hooks.apply_transaction(state->hooks.context, transaction);
  state->hooks.delete_transaction(state->hooks.context, transaction);
  FinishOperation(&observer_lifetime);
  return true;
}

}  // namespace darwin_art::window
