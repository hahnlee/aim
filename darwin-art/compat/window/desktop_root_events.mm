#import <AppKit/AppKit.h>

#include "desktop_root_events.h"

#include <atomic>
#include <exception>
#include <limits>
#include <new>

namespace darwin_art::window {
namespace {

std::atomic<uint64_t> g_next_incarnation{1};

bool AllocateIncarnation(uint64_t* value) noexcept {
  uint64_t candidate = g_next_incarnation.load(std::memory_order_relaxed);
  for (;;) {
    if (candidate == 0 || candidate == std::numeric_limits<uint64_t>::max()) return false;
    if (g_next_incarnation.compare_exchange_weak(
            candidate, candidate + 1, std::memory_order_relaxed,
            std::memory_order_relaxed)) {
      *value = candidate;
      return true;
    }
  }
}

bool IsMainThread() noexcept {
  return [NSThread isMainThread];
}

void DeliverStamp(std::shared_ptr<DesktopRootEvents::StampObserver>* observer,
                  const DesktopRootEvents::LocalStamp& stamp) noexcept {
  if (observer == nullptr || *observer == nullptr) return;
  (*observer)->OnLocalStamp(stamp);
  observer->reset();
}

}  // namespace

struct DesktopRootEvents::Binding {
  __strong NSWindow* window;
  DarwinArtDesktopRootEventCallback callback;
  DarwinArtDesktopRootEventContext context;

  Binding(NSWindow* source, DarwinArtDesktopRootEventCallback target,
          DarwinArtDesktopRootEventContext retained_context) noexcept
      : window(source), callback(target), context(retained_context) {}

  ~Binding() {
    if (context.value != nullptr && context.release != nullptr) {
      context.release(context.value);
    }
  }
};

struct DesktopRootEvents::WindowIdentity {
  __weak NSWindow* window = nil;

  explicit WindowIdentity(NSWindow* source) noexcept : window(source) {}
};

std::shared_ptr<DesktopRootEvents> DesktopRootEvents::Create(
    NSWindow* window) noexcept {
  if (!IsMainThread() || window == nil) return nullptr;
  uint64_t incarnation = 0;
  if (!AllocateIncarnation(&incarnation)) return nullptr;
  __strong NSWindow* strong_window = window;
  const bool key_window = [strong_window isKeyWindow];
  try {
    auto identity = std::make_shared<WindowIdentity>(strong_window);
    return std::shared_ptr<DesktopRootEvents>(
        new DesktopRootEvents(incarnation, std::move(identity), key_window));
  } catch (const std::bad_alloc&) {
    return nullptr;
  }
}

DesktopRootEvents::DesktopRootEvents(
    uint64_t incarnation, std::shared_ptr<WindowIdentity> window_identity,
    bool key_window) noexcept
    : incarnation_(incarnation), original_window_(std::move(window_identity)),
      state_revision_(1),
      key_window_(key_window) {}

bool DesktopRootEvents::MainThread() const noexcept {
  return IsMainThread();
}

bool DesktopRootEvents::ReserveSerialLocked(uint64_t* serial) noexcept {
  if (next_serial_ == std::numeric_limits<uint64_t>::max()) {
    FailClosedLocked();
    return false;
  }
  *serial = next_serial_++;
  return true;
}

bool DesktopRootEvents::ReserveBindingAttemptLocked(uint64_t* attempt) noexcept {
  if (next_binding_attempt_ == std::numeric_limits<uint64_t>::max()) {
    FailClosedLocked();
    return false;
  }
  *attempt = next_binding_attempt_++;
  latest_binding_attempt_ = *attempt;
  return true;
}

bool DesktopRootEvents::AdvanceRevisionLocked() noexcept {
  if (state_revision_ == std::numeric_limits<uint64_t>::max()) {
    FailClosedLocked();
    return false;
  }
  ++state_revision_;
  return true;
}

bool DesktopRootEvents::UpdateKeyLocked(bool key_window) noexcept {
  if (key_window_ == key_window) return true;
  key_window_ = key_window;
  // A serial from the previous host key interval must never authorize this
  // newly entered interval.
  latest_emitted_serial_ = 0;
  return AdvanceRevisionLocked();
}

bool DesktopRootEvents::AdvanceStampRevisionLocked() noexcept {
  if (stamp_revision_ == std::numeric_limits<uint64_t>::max()) {
    FailClosedLocked();
    return false;
  }
  ++stamp_revision_;
  return true;
}

DesktopRootEvents::LocalStamp DesktopRootEvents::CurrentStampLocked() const noexcept {
  return {incarnation_, state_revision_, latest_emitted_serial_, key_window_,
          closed_, stamp_revision_};
}

void DesktopRootEvents::FinishStampMutationLocked(const LocalStamp& before,
    std::shared_ptr<StampObserver>* observer, LocalStamp* stamp) noexcept {
  if (observer == nullptr || stamp == nullptr) return;
  const LocalStamp after = CurrentStampLocked();
  if (before.state_revision != after.state_revision ||
      before.latest_emitted_serial != after.latest_emitted_serial ||
      before.key != after.key || before.closed != after.closed) {
    (void)AdvanceStampRevisionLocked();
    *observer = stamp_observer_.lock();
  }
  *stamp = CurrentStampLocked();
}

void DesktopRootEvents::FailClosedLocked() noexcept {
  latest_emitted_serial_ = 0;
  if (!closed_) {
    closed_ = true;
    if (state_revision_ != std::numeric_limits<uint64_t>::max()) ++state_revision_;
  }
}

bool DesktopRootEvents::Bind(
    NSWindow* window, DarwinArtDesktopRootEventCallback callback,
    DarwinArtDesktopRootEventContext context) noexcept {
  if (!MainThread() || window == nil || callback == nullptr) return false;
  __strong NSWindow* original_window =
      original_window_ == nullptr ? nil : original_window_->window;
  if (original_window == nil || window != original_window) return false;
  std::shared_ptr<DesktopRootEvents> keep_alive;
  try {
    keep_alive = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return false;
  }
  (void)keep_alive;
  if (context.value == nullptr &&
      (context.retain != nullptr || context.release != nullptr)) return false;
  if (context.value != nullptr &&
      (context.retain == nullptr || context.release == nullptr)) return false;

  __strong NSWindow* strong_window = original_window;
  bool key_window = [strong_window isKeyWindow];
  uint64_t attempt = 0;
  std::shared_ptr<Binding> exhausted_binding;
  std::shared_ptr<StampObserver> phase_stamp_observer;
  LocalStamp phase_stamp;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    const LocalStamp before = CurrentStampLocked();
    if (closed_) return false;
    const bool reserved_attempt = ReserveBindingAttemptLocked(&attempt);
    if (reserved_attempt) {
      if (!UpdateKeyLocked(key_window))
        exhausted_binding = std::move(binding_);
    } else {
      exhausted_binding = std::move(binding_);
    }
    FinishStampMutationLocked(before, &phase_stamp_observer, &phase_stamp);
    if (closed_ && binding_ != nullptr) exhausted_binding = std::move(binding_);
  }
  DeliverStamp(&phase_stamp_observer, phase_stamp);
  exhausted_binding.reset();
  if (attempt == 0) return false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_ || latest_binding_attempt_ != attempt) return false;
  }

  if (context.value != nullptr) context.retain(context.value);
  std::shared_ptr<Binding> candidate;
  try {
    candidate = std::make_shared<Binding>(strong_window, callback, context);
  } catch (const std::bad_alloc&) {
    std::shared_ptr<StampObserver> allocation_stamp_observer;
    std::shared_ptr<Binding> allocation_retired_binding;
    LocalStamp allocation_stamp;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!closed_ && latest_binding_attempt_ == attempt) {
        const LocalStamp before = CurrentStampLocked();
        latest_emitted_serial_ = 0;
        FinishStampMutationLocked(before, &allocation_stamp_observer,
                                  &allocation_stamp);
        if (closed_) allocation_retired_binding = std::move(binding_);
      }
    }
    DeliverStamp(&allocation_stamp_observer, allocation_stamp);
    allocation_retired_binding.reset();
    if (context.value != nullptr) context.release(context.value);
    return false;
  }

  std::shared_ptr<Binding> old_binding;
  std::shared_ptr<StampObserver> publish_stamp_observer;
  LocalStamp publish_stamp;
  uint64_t serial = 0;
  bool published = false;
  key_window = [strong_window isKeyWindow];
  {
    std::lock_guard<std::mutex> lock(mutex_);
    const LocalStamp before = CurrentStampLocked();
    if (closed_ || latest_binding_attempt_ != attempt) {
      // candidate's retained context is released by its destructor.
    } else if (!UpdateKeyLocked(key_window)) {
      old_binding = std::move(binding_);
    } else if (!ReserveSerialLocked(&serial)) {
      old_binding = std::move(binding_);
    } else {
      old_binding = std::move(binding_);
      binding_ = candidate;
      latest_emitted_serial_ = serial;
      published = true;
    }
    FinishStampMutationLocked(before, &publish_stamp_observer, &publish_stamp);
    if (closed_ && binding_.get() == candidate.get()) {
      // candidate pins this new binding; keep the predecessor alive as well
      // until the synchronous stamp observer has completed outside the lock.
      binding_.reset();
      published = false;
    }
  }
  DeliverStamp(&publish_stamp_observer, publish_stamp);
  if (!published) {
    old_binding.reset();
    return false;
  }

  DarwinArtDesktopRootEvent event{
      key_window ? DARWIN_ART_DESKTOP_ROOT_ACTIVATED
                 : DARWIN_ART_DESKTOP_ROOT_RESIGNED,
      incarnation_, serial, key_window};
  bool still_current = false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    const LocalStamp current = CurrentStampLocked();
    still_current = current.incarnation == publish_stamp.incarnation &&
                    current.state_revision == publish_stamp.state_revision &&
                    current.latest_emitted_serial ==
                        publish_stamp.latest_emitted_serial &&
                    current.key == publish_stamp.key &&
                    current.closed == publish_stamp.closed &&
                    current.stamp_revision == publish_stamp.stamp_revision &&
                    !current.closed && binding_.get() == candidate.get() &&
                    latest_emitted_serial_ == serial;
  }
  if (!still_current) {
    old_binding.reset();
    return false;
  }
  Dispatch(candidate, event);
  old_binding.reset();
  return true;
}

bool DesktopRootEvents::Unbind() noexcept {
  return UnbindExpected(nullptr);
}

bool DesktopRootEvents::UnbindExpected(void* context) noexcept {
  if (!MainThread()) return false;
  std::shared_ptr<DesktopRootEvents> keep_alive;
  try {
    keep_alive = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return false;
  }
  (void)keep_alive;
  std::shared_ptr<Binding> old_binding;
  std::shared_ptr<StampObserver> stamp_observer;
  LocalStamp stamp;
  bool unbound = true;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_) return false;
    if (context != nullptr &&
        (binding_ == nullptr || binding_->context.value != context)) return false;
    const LocalStamp before = CurrentStampLocked();
    uint64_t ignored_attempt = 0;
    if (!ReserveBindingAttemptLocked(&ignored_attempt)) unbound = false;
    old_binding = std::move(binding_);
    latest_emitted_serial_ = 0;
    FinishStampMutationLocked(before, &stamp_observer, &stamp);
  }
  DeliverStamp(&stamp_observer, stamp);
  old_binding.reset();
  return unbound;
}

DesktopRootEvents::DeferredFact DesktopRootEvents::PrepareNotify(
    DarwinArtDesktopRootEventKind kind) noexcept {
  if (!MainThread() || (kind != DARWIN_ART_DESKTOP_ROOT_ACTIVATED &&
                        kind != DARWIN_ART_DESKTOP_ROOT_RESIGNED)) return {};
  std::shared_ptr<DesktopRootEvents> keep_alive;
  try {
    keep_alive = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return {};
  }
  (void)keep_alive;
  __strong NSWindow* window =
      original_window_ == nullptr ? nil : original_window_->window;
  if (window == nil) {
    std::shared_ptr<Binding> retired_binding;
    std::shared_ptr<StampObserver> stamp_observer;
    LocalStamp stamp;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (!closed_) {
        const LocalStamp before = CurrentStampLocked();
        FailClosedLocked();
        retired_binding = std::move(binding_);
        FinishStampMutationLocked(before, &stamp_observer, &stamp);
      }
    }
    DeliverStamp(&stamp_observer, stamp);
    retired_binding.reset();
    return {};
  }
  const bool key_window = [window isKeyWindow];
  if ((kind == DARWIN_ART_DESKTOP_ROOT_ACTIVATED) != key_window) return {};

  std::shared_ptr<Binding> binding;
  std::shared_ptr<Binding> retired_binding;
  std::shared_ptr<StampObserver> stamp_observer;
  LocalStamp stamp;
  uint64_t serial = 0;
  LocalStamp committed_stamp;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    const LocalStamp before = CurrentStampLocked();
    if (closed_) return {};
    if (!UpdateKeyLocked(key_window)) {
      retired_binding = std::move(binding_);
      binding.reset();
    } else {
      binding = binding_;
    }
    if (binding == nullptr && !closed_) latest_emitted_serial_ = 0;
    if (!closed_ && binding != nullptr) {
      if (!ReserveSerialLocked(&serial)) {
        retired_binding = std::move(binding_);
        binding.reset();
      } else {
        latest_emitted_serial_ = serial;
      }
    }
    FinishStampMutationLocked(before, &stamp_observer, &stamp);
    if (closed_ && binding_ != nullptr) {
      retired_binding = std::move(binding_);
      // retired_binding pins the observer until after stamp notification.
      binding.reset();
    }
    committed_stamp = CurrentStampLocked();
  }
  DeliverStamp(&stamp_observer, stamp);
  retired_binding.reset();
  DarwinArtDesktopRootEvent event{kind, incarnation_, serial, key_window};
  return DeferredFact(std::move(keep_alive), std::move(binding), event,
                      committed_stamp);
}

bool DesktopRootEvents::Notify(DarwinArtDesktopRootEventKind kind) noexcept {
  auto notification = PrepareNotify(kind);
  return notification.Deliver();
}

DesktopRootEvents::DeferredClose DesktopRootEvents::PrepareClose() noexcept {
  if (!MainThread()) return {};
  std::shared_ptr<DesktopRootEvents> keep_alive;
  try {
    keep_alive = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return {};
  }
  (void)keep_alive;
  __strong NSWindow* window =
      original_window_ == nullptr ? nil : original_window_->window;
  const bool key_window = window != nil && [window isKeyWindow];
  std::shared_ptr<Binding> binding;
  std::shared_ptr<StampObserver> stamp_observer;
  LocalStamp stamp;
  uint64_t serial = 0;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_) return {};
    const LocalStamp before = CurrentStampLocked();
    key_window_ = key_window;
    closed_ = true;
    latest_emitted_serial_ = 0;
    if (state_revision_ == std::numeric_limits<uint64_t>::max()) {
      // Preserve the fail-closed terminal state without wrapping.
    } else {
      ++state_revision_;
    }
    binding = std::move(binding_);
    if (binding != nullptr && next_serial_ != std::numeric_limits<uint64_t>::max()) {
      serial = next_serial_++;
      latest_emitted_serial_ = serial;
    }
    FinishStampMutationLocked(before, &stamp_observer, &stamp);
  }
  DeliverStamp(&stamp_observer, stamp);
  // Stamp-counter exhaustion seals native admission and clears its emitted
  // serial, but a separately reserved terminal serial still conveys CLOSED
  // to WMS. Unlike a nonterminal fact, this capability must not be suppressed
  // by whole-stamp revalidation: terminal truth cannot reactivate this root.
  DarwinArtDesktopRootEvent event{
      DARWIN_ART_DESKTOP_ROOT_CLOSED, incarnation_, serial, key_window};
  return DeferredClose(std::move(keep_alive), std::move(binding), event);
}

DesktopRootEvents::DeferredClose::DeferredClose(
    std::shared_ptr<DesktopRootEvents> owner, std::shared_ptr<Binding> binding,
    DarwinArtDesktopRootEvent event) noexcept
    : owner_(std::move(owner)), binding_(std::move(binding)), event_(event) {}

DesktopRootEvents::DeferredClose::DeferredClose(DeferredClose&& other) noexcept
    : owner_(std::move(other.owner_)), binding_(std::move(other.binding_)),
      event_(other.event_) {}

DesktopRootEvents::DeferredClose::~DeferredClose() {
  if (owner_ != nullptr && !owner_->MainThread()) std::terminate();
  (void)Deliver();
}

bool DesktopRootEvents::DeferredClose::Deliver() noexcept {
  if (owner_ != nullptr && !owner_->MainThread()) return false;
  auto owner = std::move(owner_);
  auto binding = std::move(binding_);
  if (owner == nullptr) return false;
  if (binding == nullptr) return true;
  if (event_.serial == 0) return false;
  owner->Dispatch(binding, event_);
  return true;
}

DesktopRootEvents::DeferredFact::DeferredFact(
    std::shared_ptr<DesktopRootEvents> owner, std::shared_ptr<Binding> binding,
    DarwinArtDesktopRootEvent event, LocalStamp stamp) noexcept
    : owner_(std::move(owner)), binding_(std::move(binding)), event_(event),
      stamp_(stamp) {}

DesktopRootEvents::DeferredFact::DeferredFact(DeferredFact&& other) noexcept
    : owner_(std::move(other.owner_)), binding_(std::move(other.binding_)),
      event_(other.event_), stamp_(other.stamp_) {}

DesktopRootEvents::DeferredFact::~DeferredFact() {
  if (owner_ != nullptr && !owner_->MainThread()) std::terminate();
  (void)Deliver();
}

bool DesktopRootEvents::DeferredFact::Deliver() noexcept {
  if (owner_ != nullptr && !owner_->MainThread()) return false;
  auto owner = std::move(owner_);
  auto binding = std::move(binding_);
  if (owner == nullptr || binding == nullptr || event_.serial == 0) return false;

  {
    std::lock_guard<std::mutex> lock(owner->mutex_);
    const LocalStamp current{owner->incarnation_, owner->state_revision_,
                             owner->latest_emitted_serial_, owner->key_window_,
                             owner->closed_, owner->stamp_revision_};
    // The whole committed stamp is checked, not only the serial. In
    // particular, Bind may have advanced the key interval before a
    // callback-capable context retain reenters and tries this capability.
    if (current.incarnation != stamp_.incarnation ||
        current.state_revision != stamp_.state_revision ||
        current.latest_emitted_serial != stamp_.latest_emitted_serial ||
        current.key != stamp_.key || current.closed != stamp_.closed ||
        current.stamp_revision != stamp_.stamp_revision ||
        current.closed || owner->binding_.get() != binding.get() ||
        owner->latest_emitted_serial_ != event_.serial) {
      return false;
    }
  }
  // No callback-capable operation occurs between the locked validation and
  // Dispatch. Dispatch itself invokes the observer only after unlocking.
  owner->Dispatch(binding, event_);
  return true;
}

bool DesktopRootEvents::Close() noexcept {
  auto notification = PrepareClose();
  return notification.Deliver();
}

void DesktopRootEvents::Dispatch(const std::shared_ptr<Binding>& binding,
                                 DarwinArtDesktopRootEvent event) noexcept {
  if (binding == nullptr || binding->callback == nullptr) return;
  std::shared_ptr<DesktopRootEvents> keep_alive;
  try {
    keep_alive = shared_from_this();
  } catch (const std::bad_weak_ptr&) {
    return;
  }
  (void)keep_alive;
  binding->callback(binding->context.value, event);
}

uint64_t DesktopRootEvents::incarnation() const noexcept { return incarnation_; }

bool DesktopRootEvents::closed() const noexcept {
  std::lock_guard<std::mutex> lock(mutex_);
  return closed_;
}

DesktopRootEvents::LocalStamp DesktopRootEvents::Snapshot() const noexcept {
  std::lock_guard<std::mutex> lock(mutex_);
  return CurrentStampLocked();
}

bool DesktopRootEvents::SubscribeStampObserver(
    const std::shared_ptr<StampObserver>& observer) noexcept {
  if (observer == nullptr) return false;
  std::shared_ptr<StampObserver> callback;
  std::shared_ptr<StampObserver> existing;
  LocalStamp stamp;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    existing = stamp_observer_.lock();
    if (existing != nullptr && existing.get() != observer.get()) return false;
    stamp_observer_ = observer;
    callback = observer;
    stamp = CurrentStampLocked();
  }
  // The strong callback capture intentionally outlives the mutex and may
  // reenter Close/Unbind/Subscribe without a provider lock cycle.
  callback->OnLocalStamp(stamp);
  callback.reset();
  return true;
}

bool DesktopRootEvents::UnsubscribeStampObserverExpected(
    const std::shared_ptr<StampObserver>& observer) noexcept {
  if (observer == nullptr) return false;
  std::shared_ptr<StampObserver> removed;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    removed = stamp_observer_.lock();
    if (removed == nullptr || removed.get() != observer.get()) return false;
    stamp_observer_.reset();
  }
  removed.reset();
  return true;
}

}  // namespace darwin_art::window
