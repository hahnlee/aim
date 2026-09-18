#include "root_key_ingress.h"

#include "root_key_routing.h"
#include "root_key_ingress_lifetime.h"
#include "root_key_ingress_registry.h"
#include "../../../compat/darwin_android_time.h"
#include "../../../compat/looper/android_looper_owner.h"
#include "../../../compat/looper/reusable_task.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <deque>
#include <limits>
#include <mutex>
#include <new>
#include <utility>
#include <vector>

namespace darwin_art::input {
namespace {

using Result = darwin_art::DarwinArtInputEnqueueResult;
constexpr size_t kCapacity = 32;
constexpr uint64_t kExpiryNanos = 5'000'000'000ULL;

uint64_t MonotonicNow() noexcept {
  const int64_t nanos = darwin_art::AndroidUptimeNanos();
  return nanos <= 0 ? 1 : static_cast<uint64_t>(nanos);
}

bool AddNanos(uint64_t value, uint64_t amount, uint64_t* result) noexcept {
  if (result == nullptr || value > std::numeric_limits<uint64_t>::max() - amount)
    return false;
  *result = value + amount;
  return true;
}

}  // namespace

namespace {

struct NotificationContext final {
  std::shared_ptr<RootKeyIngress::State> state;
  ~NotificationContext();
};

struct TaskContext final {
  std::weak_ptr<RootKeyIngress::State> state;
};

struct TimerContext final {
  // Pins completion bookkeeping, not a root, JNI receiver or window. A
  // detached provider callback must remain discoverable during shutdown.
  std::shared_ptr<RootKeyIngress::State> state;
  uint64_t generation = 0;
};

}  // namespace

struct RootKeyIngress::State final
    : public std::enable_shared_from_this<RootKeyIngress::State> {
  struct Key final {
    DarwinArtKeyEventV1 packet{};
    DecisionRecordHandle assigned;
    // Once routing has proposed an admission, keep that exact fence across
    // a channel-owner backpressure result.  It is never recomputed for the
    // same physical key.
    std::unique_ptr<InputRoutingAdmission> admission;
    uint64_t incarnation = 0;
    uint64_t state_revision = 0;
    uint64_t expiry = 0;
  };

  struct Subscription final {
    InputRoutingHandle route;
    InputRoutingNotificationSubscriptionHandle handle;
    // Routing's callback token is weak. Own its context for this subscription;
    // Close breaks this deliberate inert-State cycle outside ingress locks.
    std::shared_ptr<NotificationContext> context;
  };

  State(const RootKeyAuthorityHandle& owner, void* looper, SubmitPort port)
      : authority(owner), owner_looper(looper), submit_port(port) {}

  mutable std::mutex mutex;
  std::weak_ptr<RootKeyAuthority> authority;
  void* const owner_looper;
  const SubmitPort submit_port;
  std::deque<Key> keys;
  std::vector<Subscription> subscriptions;
  std::weak_ptr<NotificationContext> notification_context;
  std::shared_ptr<looper::ReusableLooperTask> task;
  bool closed = false;
  bool timer_scheduled = false;
  bool timer_arming = false;
  uint64_t timer_deadline = 0;
  uint64_t timer_token = 0;
  uint64_t timer_generation = 0;
  size_t outstanding_timers = 0;
  size_t callbacks = 0;
  size_t claimed_keys = 0;
  size_t operations = 0;
  size_t notification_contexts = 0;
  bool start_admitted = false;
};

class RootKeyIngressLifetime final {
 public:
  ~RootKeyIngressLifetime() = default;

 private:
  friend PreparedRootKeyIngress PrepareRootKeyIngress(
      const RootKeyAuthorityHandle&, void*, RootKeyIngress::SubmitPort) noexcept;
  friend bool StartRootKeyIngress(const RootKeyIngressLifetimeHandle&,
                                  const RootKeyAuthorityHandle&) noexcept;
  friend void CloseRootKeyIngressLifetime(
      const RootKeyIngressLifetimeHandle&) noexcept;
  friend bool IsRootKeyIngressLifetimeClosed(
      const RootKeyIngressLifetimeHandle&) noexcept;
  friend bool IsRootKeyIngressLifetimeQuiescent(
      const RootKeyIngressLifetimeHandle&) noexcept;

  explicit RootKeyIngressLifetime(
      const std::shared_ptr<RootKeyIngress::State>& state) noexcept
      : state_(state) {}
  std::shared_ptr<RootKeyIngress::State> state_;
};

namespace {

NotificationContext::~NotificationContext() {
  if (state == nullptr) return;
  std::lock_guard<std::mutex> lock(state->mutex);
  --state->notification_contexts;
}

void RequestTask(const std::shared_ptr<RootKeyIngress::State>& state) noexcept;
void DrainTask(void* opaque) noexcept;

void Notification(void* opaque, const InputRoutingNotification& note) {
  auto* context = static_cast<NotificationContext*>(opaque);
  if (context == nullptr) return;
  (void)note;
  // All of these notifications can make the head admission runnable.  In
  // particular terminal cleanup/action-unclaimed events release capacity;
  // readiness is only advisory and DrainTask revalidates the exact fence.
  const auto state = context->state;
  if (state != nullptr) RequestTask(state);
}

void ReleaseTimer(void* opaque) {
  std::unique_ptr<TimerContext> context(static_cast<TimerContext*>(opaque));
  if (context == nullptr) return;
  const auto state = context->state;
  const auto generation = context->generation;
  context.reset();
  std::lock_guard<std::mutex> lock(state->mutex);
  if (state->timer_generation == generation) {
    state->timer_scheduled = false;
    state->timer_token = 0;
    state->timer_deadline = 0;
  }
  if (state->outstanding_timers != 0) --state->outstanding_timers;
}

void FireTimer(void* opaque) noexcept {
  auto* context = static_cast<TimerContext*>(opaque);
  if (context == nullptr) return;
  const auto state = context->state;
  if (state == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->timer_generation == context->generation) {
      state->timer_scheduled = false;
      state->timer_token = 0;
      state->timer_deadline = 0;
    }
  }
  RequestTask(state);
}

bool ValidIntent(const RootKeyIntent& intent) noexcept {
  return !intent.closed && intent.server_live && !intent.stamp.closed &&
         intent.stamp.key && intent.stamp.incarnation != 0 &&
         intent.decision != nullptr && intent.decision->selection_present &&
         intent.decision->incarnation == intent.stamp.incarnation &&
         intent.decision->fact_serial == intent.stamp.latest_emitted_serial;
}

bool SameRecord(const DecisionRecordHandle& left,
                const DecisionRecordHandle& right) noexcept {
  return left.get() == right.get();
}

RootKeyIntent SnapshotIntent(const std::shared_ptr<RootKeyAuthority>& authority) noexcept {
  RootKeyIntent intent;
  if (authority == nullptr) return intent;
  auto domain = LockInputRoutingDomain();
  auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
  return guard.SnapshotKeyIntent();
}

void CloseState(const std::shared_ptr<RootKeyIngress::State>& state) noexcept;

bool ScheduleExpiry(const std::shared_ptr<RootKeyIngress::State>& state,
                    uint64_t expiry) noexcept {
  if (state == nullptr || expiry == 0) return true;
  TimerContext* context = nullptr;
  uint64_t generation = 0;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed || state->timer_scheduled) return true;
    if (state->timer_generation == std::numeric_limits<uint64_t>::max())
      return false;
    state->timer_scheduled = true;
    state->timer_deadline = expiry;
    try {
      context = new TimerContext;
      context->state = state;
      generation = ++state->timer_generation;
      context->generation = generation;
      ++state->outstanding_timers;
    } catch (const std::bad_alloc&) {
      state->timer_scheduled = false;
      state->timer_deadline = 0;
      return false;
    }
  }
  const int64_t deadline = expiry > static_cast<uint64_t>(std::numeric_limits<int64_t>::max())
      ? std::numeric_limits<int64_t>::max() : static_cast<int64_t>(expiry);
  int scheduled = 0;
  uint64_t token = 0;
  scheduled = looper::ScheduleTimedTaskAtOwned(
      state->owner_looper, deadline, &FireTimer, context, &ReleaseTimer, &token);
  bool cancel = false;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    cancel = state->closed;
    // Dispatch/release may have won before publication returns.
    if (scheduled == 1 && state->timer_generation == generation &&
        state->timer_scheduled)
      state->timer_token = token;
  }
  if (scheduled == 1 && cancel)
    (void)looper::CancelTimedTaskIfOwned(state->owner_looper, token);
  return scheduled == 1;
}

void RequestTask(const std::shared_ptr<RootKeyIngress::State>& state) noexcept {
  if (state == nullptr) return;
  std::shared_ptr<looper::ReusableLooperTask> task;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return;
    task = state->task;
  }
  if (task != nullptr) (void)task->Request();
}

bool EnsureSubscription(const std::shared_ptr<RootKeyIngress::State>& state,
                        const InputRoutingHandle& route) noexcept {
  if (state == nullptr || route == nullptr) return false;
  std::shared_ptr<NotificationContext> context;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    for (const auto& existing : state->subscriptions)
      if (existing.route.get() == route.get()) return true;
    context = state->notification_context.lock();
    if (context == nullptr) {
      try {
        context = std::make_shared<NotificationContext>();
        context->state = state;
        ++state->notification_contexts;
        state->notification_context = context;
      } catch (const std::bad_alloc&) {
        return false;
      }
    }
  }
  auto subscription = SubscribeInputRoutingNotifications(
      route, InputRoutingNotificationCallbacks{&Notification, context.get(), context});
  if (subscription == nullptr) return false;
  bool retain = false;
  std::vector<RootKeyIngress::State::Subscription> replacement;
  std::vector<RootKeyIngress::State::Subscription> retired;
  try {
    replacement.push_back({route, subscription, context});
    std::lock_guard<std::mutex> lock(state->mutex);
    if (!state->closed) {
      // Recheck under the mutex: another owner-thread wake may have
      // installed this route while SubscribeInputRoutingNotifications ran.
      bool duplicate = false;
      for (const auto& existing : state->subscriptions)
        duplicate = duplicate || existing.route.get() == route.get();
      if (!duplicate) {
        // Only the FIFO head can make progress. Retain its one exact route,
        // not a history of channels encountered by earlier keys.
        retired.swap(state->subscriptions);
        replacement.swap(state->subscriptions);
        retain = true;
      } else {
        retain = true;
      }
    }
  } catch (const std::bad_alloc&) {
    retain = false;
  }
  if (!retain) subscription.reset();
  return retain;
}

bool StillAssigned(const std::shared_ptr<RootKeyAuthority>& authority,
                   const RootKeyIngress::State::Key& key) noexcept {
  if (authority == nullptr || key.assigned == nullptr) return false;
  const RootKeyIntent intent = SnapshotIntent(authority);
  return ValidIntent(intent) &&
         intent.stamp.incarnation == key.incarnation &&
         intent.stamp.state_revision == key.state_revision &&
         SameRecord(intent.decision, key.assigned);
}

std::unique_ptr<InputRoutingAdmission> CloneAdmission(
    const InputRoutingAdmission& source) noexcept {
  try {
    auto copy = std::make_unique<InputRoutingAdmission>();
    copy->state = source.state;
    copy->packet = source.packet;
    copy->generation = source.generation;
    copy->consumer_id = source.consumer_id;
    copy->endpoint = source.endpoint;
    copy->offset_x = source.offset_x;
    copy->offset_y = source.offset_y;
    copy->pointer_down = source.pointer_down;
    copy->pointer_end = source.pointer_end;
    copy->local_transport_ready = source.local_transport_ready;
    copy->focus_ready = source.focus_ready;
    copy->legacy_key_focus = source.legacy_key_focus;
    copy->focus_epoch = source.focus_epoch;
    copy->focus_cache_revision = source.focus_cache_revision;
    copy->focus_recipient = source.focus_recipient;
    copy->key_fence = source.key_fence;
    return copy;
  } catch (...) {
    return {};
  }
}

bool RestoreHead(const std::shared_ptr<RootKeyIngress::State>& state,
                 RootKeyIngress::State::Key&& key) noexcept {
  try {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (!state->closed) {
      state->keys.push_front(std::move(key));
      return true;
    }
  } catch (...) {
    // Allocation failure is terminal for this already-owned key. Its frozen
    // admission is destroyed after the mutex has been released.
  }
  return false;
}

void ScheduleNextExpiry(const std::shared_ptr<RootKeyIngress::State>& state) noexcept {
  uint64_t earliest = 0;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed || state->timer_scheduled || state->timer_arming ||
        state->keys.empty()) return;
    for (const auto& key : state->keys) {
      if (key.expiry != 0 && (earliest == 0 || key.expiry < earliest))
        earliest = key.expiry;
    }
  }
  if (earliest != 0 && !ScheduleExpiry(state, earliest)) CloseState(state);
}

void CloseState(const std::shared_ptr<RootKeyIngress::State>& state) noexcept {
  if (state == nullptr) return;
  std::shared_ptr<looper::ReusableLooperTask> task;
  std::vector<RootKeyIngress::State::Subscription> subscriptions;
  std::deque<RootKeyIngress::State::Key> retired;
  uint64_t timer_token = 0;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return;
    state->closed = true;
    retired.swap(state->keys);
    subscriptions.swap(state->subscriptions);
    task = state->task;
    timer_token = state->timer_token;
  }
  // Resource-bearing queue rows and subscription handles are released only
  // after the ingress mutex is dropped; callbacks may re-enter routing.
  retired.clear();
  subscriptions.clear();
  if (task != nullptr) (void)task->Cancel();
  if (timer_token != 0)
    (void)looper::CancelTimedTaskIfOwned(state->owner_looper, timer_token);
}

bool StateQuiescent(const std::shared_ptr<RootKeyIngress::State>& state) noexcept {
  if (state == nullptr) return true;
  std::shared_ptr<looper::ReusableLooperTask> task;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (!state->closed || state->callbacks != 0 || state->operations != 0 ||
        state->notification_contexts != 0 || state->timer_arming ||
        state->outstanding_timers != 0) return false;
    task = state->task;
  }
  return task == nullptr || task->IsQuiescent();
}

void DrainTask(void* opaque) noexcept {
  auto* context = static_cast<TaskContext*>(opaque);
  if (context == nullptr) return;
  auto state = context->state.lock();
  if (state == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed || state->timer_arming) return;
    ++state->callbacks;
  }
  struct CallbackGuard {
    std::shared_ptr<RootKeyIngress::State> state;
    ~CallbackGuard() {
      {
        std::lock_guard<std::mutex> lock(state->mutex);
        if (state->callbacks != 0) --state->callbacks;
      }
      ScheduleNextExpiry(state);
    }
  } callback_guard{state};

  const auto authority = state->authority.lock();
  if (authority == nullptr) {
    CloseState(state);
    return;
  }
  for (size_t count = 0; count < kCapacity; ++count) {
    RootKeyIngress::State::Key key;
    {
      std::lock_guard<std::mutex> lock(state->mutex);
      if (state->closed || state->keys.empty()) return;
      key = std::move(state->keys.front());
      state->keys.pop_front();
      ++state->claimed_keys;
    }
    struct ClaimedKeyGuard {
      std::shared_ptr<RootKeyIngress::State> state;
      ~ClaimedKeyGuard() {
        std::lock_guard<std::mutex> lock(state->mutex);
        --state->claimed_keys;
      }
    } claimed_guard{state};
    const uint64_t now = MonotonicNow();
    if (key.expiry != 0 && now >= key.expiry) continue;

    if (key.assigned == nullptr) {
      const RootKeyIntent intent = SnapshotIntent(authority);
      if (intent.closed || intent.stamp.incarnation != key.incarnation ||
          intent.stamp.state_revision != key.state_revision || !intent.stamp.key) {
        continue;  // Never retarget an interval to a successor root/state.
      }
      if (!ValidIntent(intent)) {
        if (RestoreHead(state, std::move(key))) return;
        continue;
      }
      key.assigned = intent.decision;
    }
    if (!StillAssigned(authority, key)) continue;

    if (key.admission == nullptr) {
      // Promote and subscribe to the exact route before querying readiness.
      // The notification is only a wake hint; RouteRootFrameworkKeyPacket
      // takes the final readiness/root fence under its own transaction.
      RootKeyAuthorityTicket ticket;
      {
        auto domain = LockInputRoutingDomain();
        auto guard = LockRootKeyAuthorityForRouting(domain, *authority);
        ticket = guard.TrySnapshot();
      }
      if (!ticket || ticket.Decision() != key.assigned) {
        if (StillAssigned(authority, key)) {
          if (RestoreHead(state, std::move(key))) return;
        }
        continue;
      }
      const auto route = ticket.Routing();
      if (route == nullptr || !EnsureSubscription(state, route)) {
        if (RestoreHead(state, std::move(key))) return;
        continue;
      }
      InputRoutingAdmission admission;
      const auto proposed = RouteRootFrameworkKeyPacket(
          authority, key.packet, &admission, key.assigned);
      if (proposed != Result::kQueued) {
        if ((proposed == Result::kNoFocusedChannel ||
             proposed == Result::kBackpressured) &&
            StillAssigned(authority, key)) {
          if (RestoreHead(state, std::move(key))) return;
        }
        continue;
      }
      try {
        key.admission = std::make_unique<InputRoutingAdmission>(std::move(admission));
      } catch (const std::bad_alloc&) {
        if (RestoreHead(state, std::move(key))) return;
        continue;
      }
    }

    if (!ValidateRootKeyAdmissionForRetry(*key.admission)) continue;
    const auto attempt = CloneAdmission(*key.admission);
    if (attempt == nullptr) {
      if (RestoreHead(state, std::move(key))) return;
      continue;
    }
    Result submitted;
    try {
      submitted = state->submit_port(std::move(*attempt));
    } catch (...) {
      // The channel port may already own an action. Never duplicate the key
      // by retrying after an indeterminate transfer; its ledger owns cleanup.
      std::fputs("ART InputChannel: root key submit threw; ingress closed\n", stderr);
      CloseState(state);
      return;
    }
    if (submitted == Result::kBackpressured) {
      if (RestoreHead(state, std::move(key))) return;
      continue;
    }
    // A queued/terminal result consumes the attempt; the retained original
    // is intentionally released here. Backpressure above keeps it frozen.
    key.admission.reset();
  }
}

}  // namespace

std::shared_ptr<RootKeyIngress> RootKeyIngress::Create(
    const RootKeyAuthorityHandle& authority, void* owner_looper,
    SubmitPort submit_port) noexcept {
  return AcquireCanonicalRootKeyIngress(authority, owner_looper, submit_port);
}

RootKeyIngress::RootKeyIngress(std::shared_ptr<State> state) noexcept
    : state_(std::move(state)) {}

RootKeyIngress::~RootKeyIngress() { Close(); }

Result RootKeyIngress::Submit(const DarwinArtKeyEventV1& key) noexcept {
  const auto state = state_;
  if (state == nullptr) return Result::kNoFocusedChannel;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return Result::kNoFocusedChannel;
    ++state->operations;
  }
  struct OperationGuard {
    std::shared_ptr<State> state;
    ~OperationGuard() {
      std::lock_guard<std::mutex> lock(state->mutex);
      --state->operations;
    }
  } operation_guard{state};
  if (key.version != 1 || key.size < sizeof(DarwinArtKeyEventV1) ||
      (key.action != 0 && key.action != 1))
    return Result::kNoFocusedChannel;
  const auto authority = state->authority.lock();
  if (authority == nullptr) return Result::kNoFocusedChannel;
  const RootKeyIntent intent = SnapshotIntent(authority);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr,
                 "ART RootKey ingress sequence=%llu action=%u closed=%u "
                 "key=%u incarnation=%llu state_revision=%llu serial=%llu "
                 "server_live=%u decision_valid=%u\n",
                 static_cast<unsigned long long>(key.sequence), key.action,
                 static_cast<unsigned>(intent.closed || intent.stamp.closed),
                 static_cast<unsigned>(intent.stamp.key),
                 static_cast<unsigned long long>(intent.stamp.incarnation),
                 static_cast<unsigned long long>(intent.stamp.state_revision),
                 static_cast<unsigned long long>(intent.stamp.latest_emitted_serial),
                 static_cast<unsigned>(intent.server_live),
                 static_cast<unsigned>(ValidIntent(intent)));
  }
  if (intent.closed || intent.stamp.closed || !intent.stamp.key ||
      intent.stamp.incarnation == 0)
    return Result::kNoFocusedChannel;
  RootKeyIngress::State::Key queued;
  queued.packet = key;
  queued.incarnation = intent.stamp.incarnation;
  queued.state_revision = intent.stamp.state_revision;
  const uint64_t now = MonotonicNow();
  if (!AddNanos(now, kExpiryNanos, &queued.expiry)) return Result::kNoFocusedChannel;
  if (ValidIntent(intent)) queued.assigned = intent.decision;
  uint64_t earliest = queued.expiry;
  bool arm_timer = false;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return Result::kNoFocusedChannel;
    if (state->keys.size() + state->claimed_keys >= kCapacity)
      return Result::kBackpressured;
    if (state->timer_arming) return Result::kBackpressured;
    try {
      state->keys.push_back(std::move(queued));
    } catch (const std::bad_alloc&) {
      return Result::kBackpressured;
    }
    if (!state->timer_scheduled && !state->timer_arming) {
      state->timer_arming = true;
      arm_timer = true;
    }
    for (const auto& pending : state->keys)
      earliest = std::min(earliest, pending.expiry);
  }
  if (arm_timer) {
    const bool armed = ScheduleExpiry(state, earliest);
    if (!armed) {
      CloseState(state);
      std::lock_guard<std::mutex> lock(state->mutex);
      state->timer_arming = false;
      return Result::kNoFocusedChannel;
    }
    std::lock_guard<std::mutex> lock(state->mutex);
    state->timer_arming = false;
  }
  RequestTask(state);
  return Result::kQueued;
}

void RootKeyIngress::Close() noexcept {
  CloseState(state_);
}

bool RootKeyIngress::IsQuiescent() const noexcept {
  return StateQuiescent(state_);
}


PreparedRootKeyIngress PrepareRootKeyIngress(
    const RootKeyAuthorityHandle& authority, void* owner_looper,
    RootKeyIngress::SubmitPort submit_port) noexcept {
  PreparedRootKeyIngress prepared;
  if (authority == nullptr || owner_looper == nullptr || submit_port == nullptr)
    return prepared;
  try {
    auto state = std::make_shared<RootKeyIngress::State>(
        authority, owner_looper, submit_port);
    auto facade = std::shared_ptr<RootKeyIngress>(new RootKeyIngress(state));
    auto lifetime = std::shared_ptr<RootKeyIngressLifetime>(
        new RootKeyIngressLifetime(state));
    prepared.facade = std::move(facade);
    prepared.lifetime = std::move(lifetime);
  } catch (...) {
    prepared = {};
  }
  return prepared;
}

bool StartRootKeyIngress(const RootKeyIngressLifetimeHandle& lifetime,
                         const RootKeyAuthorityHandle& authority) noexcept {
  if (lifetime == nullptr || authority == nullptr || lifetime->state_ == nullptr)
    return false;
  const auto state = lifetime->state_;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed || state->authority.owner_before(authority) ||
        authority.owner_before(state->authority) || state->start_admitted)
      return false;
    state->start_admitted = true;
    ++state->operations;
  }
  struct OperationGuard {
    std::shared_ptr<RootKeyIngress::State> state;
    ~OperationGuard() {
      std::lock_guard<std::mutex> lock(state->mutex);
      if (state->operations != 0) --state->operations;
    }
  } operation_guard{state};
  std::shared_ptr<TaskContext> context;
  std::shared_ptr<looper::ReusableLooperTask> task;
  try {
    context = std::make_shared<TaskContext>();
    context->state = state;
    task = looper::ReusableLooperTask::Prepare(
        state->owner_looper,
        looper::ReusableLooperTaskCallbacks{&DrainTask, context.get(), context});
    bool closed = false;
    {
      std::lock_guard<std::mutex> lock(state->mutex);
      closed = state->closed;
      // Retain completion bookkeeping before the authority may request this
      // task. Even a close racing initialization must poll detached tails.
      state->task = task;
    }
    if (closed) {
      if (task != nullptr) (void)task->Cancel();
      return false;
    }
    if (task == nullptr || !authority->SubscribeProgressTask(task)) {
      CloseState(state);
      return false;
    }
    return true;
  } catch (...) {
    if (task != nullptr) (void)task->Cancel();
    CloseState(state);
    return false;
  }
}

void CloseRootKeyIngressLifetime(
    const RootKeyIngressLifetimeHandle& lifetime) noexcept {
  if (lifetime != nullptr) CloseState(lifetime->state_);
}

bool IsRootKeyIngressLifetimeClosed(
    const RootKeyIngressLifetimeHandle& lifetime) noexcept {
  if (lifetime == nullptr || lifetime->state_ == nullptr) return true;
  std::lock_guard<std::mutex> lock(lifetime->state_->mutex);
  return lifetime->state_->closed;
}

bool IsRootKeyIngressLifetimeQuiescent(
    const RootKeyIngressLifetimeHandle& lifetime) noexcept {
  return lifetime == nullptr || StateQuiescent(lifetime->state_);
}

}  // namespace darwin_art::input
