#import <AppKit/AppKit.h>

#include "compat/window/desktop_root_events.h"

#include <atomic>
#include <condition_variable>
#include <chrono>
#include <cstdlib>
#include <cstdio>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <thread>
#include <vector>

namespace { thread_local bool fail_next_allocation = false; }

void* operator new(std::size_t size) {
  if (fail_next_allocation) {
    fail_next_allocation = false;
    throw std::bad_alloc();
  }
  if (void* value = std::malloc(size == 0 ? 1 : size)) return value;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* value) noexcept { std::free(value); }
void operator delete[](void* value) noexcept { std::free(value); }

namespace darwin_art::window {
struct DesktopRootEventsTestPeer {
  static void ExhaustSerial(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.next_serial_ = std::numeric_limits<uint64_t>::max();
  }
  static void ExhaustStampRevision(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.stamp_revision_ = std::numeric_limits<uint64_t>::max();
  }
};
}  // namespace darwin_art::window

@interface StampSubscriptionTestWindow : NSWindow
@property(nonatomic, assign) BOOL reportedKeyWindow;
@end

@implementation StampSubscriptionTestWindow
- (BOOL)isKeyWindow { return self.reportedKeyWindow; }
@end

namespace {

using darwin_art::window::DesktopRootEvents;
using darwin_art::window::DesktopRootEventsTestPeer;

struct StampObserver final : DesktopRootEvents::StampObserver {
  explicit StampObserver(DesktopRootEvents* source = nullptr) : source(source) {}
  ~StampObserver() override {
    // Final callback-capable destruction must also occur outside root locks.
    if (source != nullptr) (void)source->Snapshot();
  }
  DesktopRootEvents* source;
  mutable std::mutex mutex;
  std::condition_variable cv;
  bool block_first = false;
  bool first_started = false;
  bool release_first = false;
  uint64_t last_revision = 0;
  bool saw_closed = false;
  size_t accepted = 0;
  size_t suppressed = 0;
  std::vector<DesktopRootEvents::LocalStamp> received;

  void OnLocalStamp(DesktopRootEvents::LocalStamp stamp) noexcept override {
    std::unique_lock<std::mutex> lock(mutex);
    received.push_back(stamp);
    if (block_first && !first_started) {
      first_started = true;
      cv.notify_all();
      if (!cv.wait_for(lock, std::chrono::seconds(5), [&] { return release_first; }))
        std::abort();
    }
    const auto current = source == nullptr ? stamp : source->Snapshot();
    const bool stale = stamp.stamp_revision <= last_revision ||
                       (current.stamp_revision > stamp.stamp_revision &&
                        !stamp.closed);
    // Terminal truth wins even when publication revision cannot advance.
    if (saw_closed || (!stamp.closed && stale)) {
      ++suppressed;
      return;
    }
    if (stamp.closed) saw_closed = true;
    last_revision = stamp.stamp_revision;
    ++accepted;
  }
};

struct EventState {
  DesktopRootEvents* source = nullptr;
  std::shared_ptr<StampObserver> stamps = {};
  int retains = 0;
  int releases = 0;
  std::vector<DarwinArtDesktopRootEvent> events = {};
  bool fail_candidate = false;
};

void RequireNotified(const EventState& state) noexcept {
  if (state.stamps == nullptr) return;
  const auto current = state.source->Snapshot();
  std::lock_guard lock(state.stamps->mutex);
  if (state.stamps->last_revision != current.stamp_revision ||
      state.stamps->saw_closed != current.closed) std::abort();
}
void Retain(void* value) noexcept {
  auto& state = *static_cast<EventState*>(value);
  RequireNotified(state);
  ++state.retains;
  if (state.fail_candidate) fail_next_allocation = true;
}
void Release(void* value) noexcept {
  auto& state = *static_cast<EventState*>(value);
  RequireNotified(state);
  ++state.releases;
}
void Record(void* value, DarwinArtDesktopRootEvent event) noexcept {
  auto* state = static_cast<EventState*>(value);
  RequireNotified(*state);
  state->events.push_back(event);
  if (state->source != nullptr) (void)state->source->Snapshot();
}

bool Check(bool value, const char* message) {
  if (value) return true;
  std::fprintf(stderr, "desktop root stamp subscription failure: %s\n", message);
  return false;
}

// Bindings must settle before their stack callback contexts on every exit,
// including a failed assertion; this guard is declared after each context.
struct BindingLifetime {
  std::shared_ptr<DesktopRootEvents> source;
  ~BindingLifetime() { (void)source->Unbind(); }
};

}  // namespace

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    auto* window = [[StampSubscriptionTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    if (!Check(window != nil, "real AppKit window")) return 1;
    window.releasedWhenClosed = NO;
    window.reportedKeyWindow = NO;

    auto source = DesktopRootEvents::Create(window);
    if (!Check(source != nullptr, "root allocation")) return 1;

    // One weak slot: baseline/retry is idempotent for the same observer and a
    // distinct live observer cannot replace it.
    auto observer = std::make_shared<StampObserver>(source.get());
    if (!Check(source->SubscribeStampObserver(observer), "initial subscription")) return 1;
    const size_t baseline = observer->accepted;
    if (!Check(source->SubscribeStampObserver(observer) && observer->accepted == baseline,
               "same observer retry suppresses old baseline")) return 1;
    auto distinct = std::make_shared<StampObserver>(source.get());
    if (!Check(!source->SubscribeStampObserver(distinct), "distinct observer rejected")) return 1;
    if (!Check(!source->UnsubscribeStampObserverExpected(distinct),
               "foreign observer cannot unsubscribe")) return 1;
    if (!Check(source->UnsubscribeStampObserverExpected(observer),
               "exact observer unsubscribe")) return 1;
    if (!Check(!source->UnsubscribeStampObserverExpected(observer),
               "duplicate unsubscribe rejected")) return 1;
    if (!Check(source->SubscribeStampObserver(distinct) &&
                   !source->UnsubscribeStampObserverExpected(observer),
               "delayed old cleanup cannot detach successor")) return 1;
    window.reportedKeyWindow = YES;
    if (!Check(!source->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED) &&
                   distinct->received.back().key,
               "successor remains subscribed")) return 1;
    window.reportedKeyWindow = NO;
    (void)source->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    std::weak_ptr<StampObserver> weak_observer = observer;
    observer.reset();
    if (!Check(weak_observer.expired(), "root keeps observer weak")) return 1;

    // A reentrant stamp hook can close during Bind phase one. No stale Bind
    // baseline may dispatch after that hook supersedes the attempt.
    auto reentrant = DesktopRootEvents::Create(window);
    bool close_once = false;
    struct CloseHook final : DesktopRootEvents::StampObserver {
      DesktopRootEvents* source;
      bool* once;
      bool armed = false;
      void OnLocalStamp(DesktopRootEvents::LocalStamp stamp) noexcept override {
        if (armed && !stamp.closed && !*once) {
          *once = true;
          (void)source->Close();
        }
      }
    } hook;
    hook.source = reentrant.get();
    hook.once = &close_once;
    // Use a non-owning shared wrapper for the stack hook only during this
    // bounded test; the provider stores it weakly and the call is synchronous.
    auto hook_owner = std::shared_ptr<DesktopRootEvents::StampObserver>(
        &hook, [](DesktopRootEvents::StampObserver*) {});
    if (!Check(reentrant->SubscribeStampObserver(hook_owner), "reentrant hook baseline")) return 1;
    hook.armed = true;
    window.reportedKeyWindow = YES;
    EventState reentrant_events{reentrant.get()};
    BindingLifetime reentrant_lifetime{reentrant};
    if (!Check(!reentrant->Bind(window, Record,
                                {&reentrant_events, Retain, Release}) &&
                   reentrant->closed() && reentrant_events.events.empty(),
               "Bind close hook suppresses stale fact")) return 1;
    window.reportedKeyWindow = NO;

    // Concurrent initial delivery can become stale after an exact Unbind;
    // the subscriber's monotonic/full-stamp guard suppresses that old value.
    auto concurrent = DesktopRootEvents::Create(window);
    EventState concurrent_events{concurrent.get()};
    BindingLifetime concurrent_lifetime{concurrent};
    if (!Check(concurrent->Bind(window, Record,
                                {&concurrent_events, Retain, Release}),
               "concurrent source bind")) return 1;
    auto concurrent_observer = std::make_shared<StampObserver>(concurrent.get());
    concurrent_observer->block_first = true;
    std::thread subscribe_thread([&] {
      (void)concurrent->SubscribeStampObserver(concurrent_observer);
    });
    {
      std::unique_lock<std::mutex> lock(concurrent_observer->mutex);
      if (!concurrent_observer->cv.wait_for(lock, std::chrono::seconds(5),
          [&] { return concurrent_observer->first_started; })) std::abort();
    }
    if (!Check(concurrent->Unbind(), "unbind while baseline callback is retained")) return 1;
    {
      std::lock_guard<std::mutex> lock(concurrent_observer->mutex);
      concurrent_observer->release_first = true;
    }
    concurrent_observer->cv.notify_all();
    subscribe_thread.join();
    if (!Check(concurrent_observer->suppressed == 1,
               "late baseline suppressed after newer unbind")) return 1;

    // Same-key serial publication advances stamp_revision but not the
    // activation state revision; entering a new key interval clears serial.
    auto serial_source = DesktopRootEvents::Create(window);
    auto serial_observer = std::make_shared<StampObserver>(serial_source.get());
    if (!Check(serial_source->SubscribeStampObserver(serial_observer), "serial observer")) return 1;
    EventState serial_events{serial_source.get(), serial_observer};
    BindingLifetime serial_lifetime{serial_source};
    if (!Check(serial_source->Bind(window, Record,
                                   {&serial_events, Retain, Release}), "serial bind")) return 1;
    const auto before_same = serial_source->Snapshot();
    if (!Check(serial_source->Bind(window, Record,
                                   {&serial_events, Retain, Release}), "same-key rebind")) return 1;
    const auto after_same = serial_source->Snapshot();
    if (!Check(after_same.state_revision == before_same.state_revision &&
                   after_same.stamp_revision > before_same.stamp_revision &&
                   after_same.latest_emitted_serial > before_same.latest_emitted_serial,
               "same-key serial stamp revision")) return 1;
    window.reportedKeyWindow = YES;
    if (!Check(serial_source->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED),
               "new key interval notify")) return 1;
    const auto entered = serial_source->Snapshot();
    if (!Check(entered.latest_emitted_serial != 0 &&
                   entered.state_revision > after_same.state_revision,
               "new interval gets new activation revision")) return 1;
    window.reportedKeyWindow = NO;
    if (!Check(serial_source->Unbind(), "serial unbind")) return 1;

    // Allocation fails at the real binding allocation, after context retain.
    // The incumbent remains bound, but its serial is revoked before either
    // candidate cleanup or eventual incumbent release can reenter the root.
    auto allocation_source = DesktopRootEvents::Create(window);
    auto allocation_observer = std::make_shared<StampObserver>(allocation_source.get());
    if (!Check(allocation_source->SubscribeStampObserver(allocation_observer),
               "allocation observer")) return 1;
    EventState incumbent{allocation_source.get(), allocation_observer};
    BindingLifetime incumbent_lifetime{allocation_source};
    if (!Check(allocation_source->Bind(window, Record, {&incumbent, Retain, Release}),
               "allocation incumbent bind")) return 1;
    const auto before_failure = allocation_source->Snapshot();
    EventState failing_candidate{allocation_source.get(), allocation_observer};
    failing_candidate.fail_candidate = true;
    BindingLifetime allocation_lifetime{allocation_source};
    if (!Check(!allocation_source->Bind(window, Record,
                                        {&failing_candidate, Retain, Release}) &&
                   failing_candidate.retains == 1 && failing_candidate.releases == 1 &&
                   incumbent.releases == 0 && !allocation_source->closed() &&
                   allocation_source->Snapshot().latest_emitted_serial == 0 &&
                   allocation_source->Snapshot().stamp_revision > before_failure.stamp_revision,
               "binding allocation failure publishes revoke before cleanup")) return 1;

    // Unbound validated facts still publish key interval changes with no
    // emitted serial, independent of the Java observer slot.
    auto unbound = DesktopRootEvents::Create(window);
    auto unbound_observer = std::make_shared<StampObserver>(unbound.get());
    if (!Check(unbound->SubscribeStampObserver(unbound_observer),
               "unbound stamp subscriber")) return 1;
    window.reportedKeyWindow = YES;
    if (!Check(!unbound->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED) &&
                   unbound_observer->received.back().key &&
                   unbound_observer->received.back().latest_emitted_serial == 0,
               "unbound key transition is published without Java fact")) return 1;
    window.reportedKeyWindow = NO;

    // Serial and stamp-revision exhaustion are sticky closed states and must
    // notify the observer with a closed full stamp before releasing context.
    auto serial_exhausted = DesktopRootEvents::Create(window);
    auto exhaustion_observer = std::make_shared<StampObserver>(serial_exhausted.get());
    if (!Check(serial_exhausted->SubscribeStampObserver(exhaustion_observer),
               "exhaustion observer")) return 1;
    EventState serial_exhausted_events{serial_exhausted.get(), exhaustion_observer};
    BindingLifetime serial_exhausted_lifetime{serial_exhausted};
    if (!Check(serial_exhausted->Bind(window, Record,
                                      {&serial_exhausted_events, Retain, Release}),
               "serial exhaustion bind")) return 1;
    DesktopRootEventsTestPeer::ExhaustSerial(*serial_exhausted);
    window.reportedKeyWindow = NO;
    if (!Check(!serial_exhausted->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED) &&
                   serial_exhausted->closed() && exhaustion_observer->saw_closed,
               "serial exhaustion sticky close")) return 1;
    auto revision_exhausted = DesktopRootEvents::Create(window);
    auto revision_observer = std::make_shared<StampObserver>(revision_exhausted.get());
    if (!Check(revision_exhausted->SubscribeStampObserver(revision_observer),
               "revision observer")) return 1;
    EventState revision_events{revision_exhausted.get(), revision_observer};
    BindingLifetime revision_lifetime{revision_exhausted};
    if (!Check(revision_exhausted->Bind(window, Record,
                                        {&revision_events, Retain, Release}),
               "revision exhaustion bind")) return 1;
    DesktopRootEventsTestPeer::ExhaustStampRevision(*revision_exhausted);
    window.reportedKeyWindow = YES;
    (void)revision_exhausted->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED);
    if (!Check(revision_exhausted->closed() && revision_observer->saw_closed,
               "stamp revision exhaustion sticky close")) return 1;

    // A stamp-revision overflow still reserves a distinct terminal event
    // serial for the retained Java-facing callback, while the native stamp
    // observer sees the fail-closed latest serial (zero) exactly once.
    auto terminal_source = DesktopRootEvents::Create(window);
    auto terminal_observer = std::make_shared<StampObserver>(terminal_source.get());
    EventState terminal_events{terminal_source.get()};
    BindingLifetime terminal_lifetime{terminal_source};
    if (!Check(terminal_source->Bind(window, Record,
                                    {&terminal_events, Retain, Release}),
               "terminal overflow bind")) return 1;
    DesktopRootEventsTestPeer::ExhaustStampRevision(*terminal_source);
    if (!Check(terminal_source->SubscribeStampObserver(terminal_observer),
               "terminal baseline at saturated revision")) return 1;
    terminal_events.stamps = terminal_observer;
    auto terminal = terminal_source->PrepareClose();
    if (!Check(terminal_source->closed() && terminal_observer->saw_closed &&
                   terminal_observer->accepted == 2 &&
                   terminal_observer->last_revision == std::numeric_limits<uint64_t>::max() &&
                   terminal_observer->received.back().latest_emitted_serial == 0,
               "terminal native stamp fail-closed")) return 1;
    if (!Check(terminal.Deliver() && terminal_events.events.back().serial != 0 &&
                   terminal_events.events.back().kind == DARWIN_ART_DESKTOP_ROOT_CLOSED,
               "terminal callback retains independent close serial")) return 1;
    if (!Check(!terminal.Deliver() && terminal_events.events.size() == 2,
               "retained close delivered once")) return 1;

    std::fprintf(stdout, "desktop root stamp subscription: PASS weak/exact/reentrant/concurrent/revision\n");
    return 0;
  }
}
