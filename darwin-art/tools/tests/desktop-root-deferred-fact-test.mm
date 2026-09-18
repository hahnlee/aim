#import <AppKit/AppKit.h>

#include "compat/window/desktop_root_events.h"

#include <cstdio>
#include <limits>
#include <memory>
#include <thread>

namespace darwin_art::window {
struct DesktopRootEventsTestPeer {
  static void ExhaustSerial(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.next_serial_ = std::numeric_limits<uint64_t>::max();
  }
  static void ExhaustRevision(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.state_revision_ = std::numeric_limits<uint64_t>::max();
  }
};
}  // namespace darwin_art::window

@interface DeferredFactTestWindow : NSWindow
@property(nonatomic, assign) BOOL reportedKeyWindow;
@end

@implementation DeferredFactTestWindow
- (BOOL)isKeyWindow {
  return self.reportedKeyWindow;
}
@end

namespace {

using darwin_art::window::DesktopRootEvents;

struct CallbackState {
  int retains = 0;
  int releases = 0;
  int callbacks = 0;
  bool saw_committed_stamp = false;
  uint64_t last_serial = 0;
};

void Retain(void* value) noexcept {
  ++static_cast<CallbackState*>(value)->retains;
}

void Release(void* value) noexcept {
  ++static_cast<CallbackState*>(value)->releases;
}

void Record(void* value, DarwinArtDesktopRootEvent event) noexcept {
  auto* state = static_cast<CallbackState*>(value);
  ++state->callbacks;
  state->last_serial = event.serial;
}

struct StampCheckingCallback {
  DesktopRootEvents* source = nullptr;
  CallbackState* state = nullptr;
};

void RecordCommitted(void* value, DarwinArtDesktopRootEvent event) noexcept {
  auto* callback = static_cast<StampCheckingCallback*>(value);
  ++callback->state->callbacks;
  callback->state->last_serial = event.serial;
  const auto stamp = callback->source->Snapshot();
  callback->state->saw_committed_stamp =
      stamp.incarnation == event.incarnation &&
      stamp.latest_emitted_serial == event.serial &&
      stamp.key == event.key_window_snapshot && !stamp.closed;
}

void RetainCommitted(void* value) noexcept {
  ++static_cast<StampCheckingCallback*>(value)->state->retains;
}

void ReleaseCommitted(void* value) noexcept {
  ++static_cast<StampCheckingCallback*>(value)->state->releases;
}

struct ReentrantPrepare {
  DesktopRootEvents::DeferredFact* pending = nullptr;
  DesktopRootEvents* source = nullptr;
  uint64_t old_revision = 0;
  bool saw_changed_revision = false;
  bool stale_delivery = true;
  int retains = 0;
  int releases = 0;
  int callbacks = 0;
};

void RetainAndDeliverOld(void* value) noexcept {
  auto* reentrant = static_cast<ReentrantPrepare*>(value);
  ++reentrant->retains;
  const auto current = reentrant->source->Snapshot();
  reentrant->saw_changed_revision =
      current.latest_emitted_serial == 0 &&
      current.state_revision != reentrant->old_revision;
  reentrant->stale_delivery = reentrant->pending->Deliver();
}

void ReleaseReentrant(void* value) noexcept {
  ++static_cast<ReentrantPrepare*>(value)->releases;
}

void RecordReentrant(void* value, DarwinArtDesktopRootEvent event) noexcept {
  auto* reentrant = static_cast<ReentrantPrepare*>(value);
  ++reentrant->callbacks;
  (void)event;
}

bool Check(bool value, const char* message) {
  if (value) return true;
  std::fprintf(stderr, "desktop root deferred fact failure: %s\n", message);
  return false;
}

}  // namespace

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    auto* window = [[DeferredFactTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    if (!Check(window != nil, "real NSWindow allocation")) return 1;
    window.releasedWhenClosed = NO;
    window.reportedKeyWindow = NO;

    auto source = DesktopRootEvents::Create(window);
    if (!Check(source != nullptr, "source allocation")) return 1;

    // A valid deferred fact commits its serial/stamp before the callback and
    // can be explicitly delivered exactly once.
    CallbackState committed_state;
    StampCheckingCallback committed_callback{source.get(), &committed_state};
    DarwinArtDesktopRootEventContext committed_context{
        &committed_callback, RetainCommitted, ReleaseCommitted};
    if (!Check(source->Bind(window, RecordCommitted, committed_context),
               "initial bind")) return 1;
    if (!Check(committed_state.callbacks == 1, "bind baseline callback")) return 1;
    auto fact = source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    const auto prepared = source->Snapshot();
    if (!Check(prepared.latest_emitted_serial != 0 && !prepared.closed,
               "prepare commits serial")) return 1;
    if (!Check(fact.Deliver() && committed_state.callbacks == 2 &&
                   committed_state.saw_committed_stamp,
               "explicit delivery sees committed stamp")) return 1;
    if (!Check(!fact.Deliver(), "explicit delivery is single-use")) return 1;

    // Move transfers the one delivery right; the moved-from capability is
    // inert, and scope exit on the main thread settles a pending fact.
    auto moved_source = DesktopRootEvents::Create(window);
    CallbackState moved_state;
    if (!Check(moved_source->Bind(window, Record, {&moved_state, Retain, Release}),
               "move source bind")) return 1;
    auto pending_move = moved_source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    auto moved_fact = std::move(pending_move);
    if (!Check(!pending_move.Deliver() && moved_fact.Deliver() &&
                   !moved_fact.Deliver() && moved_state.callbacks == 2,
               "move transfers single delivery")) return 1;
    auto scope_source = DesktopRootEvents::Create(window);
    CallbackState scope_state;
    if (!Check(scope_source->Bind(window, Record, {&scope_state, Retain, Release}),
               "scope source bind")) return 1;
    {
      auto scope_fact = scope_source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
      (void)scope_fact;
    }
    if (!Check(scope_state.callbacks == 2, "scope exit auto-delivers on main")) return 1;

    // A newer same-binding fact supersedes an older serial.
    auto superseded_source = DesktopRootEvents::Create(window);
    CallbackState superseded_state;
    if (!Check(superseded_source->Bind(window, Record,
                                      {&superseded_state, Retain, Release}),
               "supersede source bind")) return 1;
    auto superseded = superseded_source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    auto successor_fact = superseded_source->PrepareNotify(
        DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    if (!Check(!superseded.Deliver() && successor_fact.Deliver() &&
                   superseded_state.callbacks == 2,
               "superseded serial is rejected")) return 1;

    // Rebinding, unbinding, and closing all invalidate a prepared fact before
    // its callback tail gets a chance to run.
    auto rebound_source = DesktopRootEvents::Create(window);
    CallbackState rebound_old;
    CallbackState rebound_new;
    if (!Check(rebound_source->Bind(window, Record,
                                    {&rebound_old, Retain, Release}),
               "rebind predecessor")) return 1;
    auto rebound_fact = rebound_source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    if (!Check(rebound_source->Bind(window, Record,
                                    {&rebound_new, Retain, Release}) &&
                   !rebound_fact.Deliver() && rebound_old.callbacks == 1 &&
                   rebound_new.callbacks == 1,
               "rebound binding rejects predecessor fact")) return 1;
    if (!Check(rebound_source->Unbind(), "rebound source unbind")) return 1;

    auto unbound_source = DesktopRootEvents::Create(window);
    CallbackState unbound_state;
    if (!Check(unbound_source->Bind(window, Record,
                                    {&unbound_state, Retain, Release}),
               "unbind predecessor")) return 1;
    auto unbound_fact = unbound_source->PrepareNotify(
        DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    if (!Check(unbound_source->Unbind() && !unbound_fact.Deliver() &&
                   unbound_state.callbacks == 1,
               "unbound source rejects pending fact")) return 1;

    auto closed_source = DesktopRootEvents::Create(window);
    CallbackState closed_state;
    if (!Check(closed_source->Bind(window, Record,
                                   {&closed_state, Retain, Release}),
               "close predecessor")) return 1;
    auto closed_fact = closed_source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    auto terminal = closed_source->PrepareClose();
    if (!Check(!closed_fact.Deliver() && terminal.Deliver() &&
                   closed_state.callbacks == 2,
               "closed source rejects pending fact")) return 1;

    // Valid facts are main-thread only. A wrong-thread attempt preserves the
    // capability so the owner thread can still settle it.
    auto thread_source = DesktopRootEvents::Create(window);
    CallbackState thread_state;
    if (!Check(thread_source->Bind(window, Record,
                                   {&thread_state, Retain, Release}),
               "thread source bind")) return 1;
    auto thread_fact = thread_source->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    bool wrong_thread = true;
    std::thread worker([&] { wrong_thread = thread_fact.Deliver(); });
    worker.join();
    if (!Check(!wrong_thread && thread_state.callbacks == 1 && thread_fact.Deliver() &&
                   thread_state.callbacks == 2,
               "wrong-thread delivery has no effect")) return 1;

    const auto before_invalid = thread_source->Snapshot();
    auto invalid = thread_source->PrepareNotify(
        static_cast<DarwinArtDesktopRootEventKind>(99));
    if (!Check(!invalid.Deliver(), "invalid kind has no effect")) return 1;
    const auto after_invalid = thread_source->Snapshot();
    if (!Check(before_invalid.state_revision == after_invalid.state_revision &&
                   before_invalid.latest_emitted_serial ==
                       after_invalid.latest_emitted_serial &&
                   before_invalid.key == after_invalid.key &&
                   thread_state.callbacks == 2,
               "invalid kind preserves stamp and callback state")) return 1;

    // A validated fact while unbound remains authoritative in the cached
    // stamp, but has no delivery target.
    auto valid_unbound = DesktopRootEvents::Create(window);
    window.reportedKeyWindow = YES;
    auto unbound_fact_stamp = valid_unbound->PrepareNotify(
        DARWIN_ART_DESKTOP_ROOT_ACTIVATED);
    const auto unbound_stamp = valid_unbound->Snapshot();
    if (!Check(unbound_stamp.key && unbound_stamp.latest_emitted_serial == 0 &&
                   !unbound_stamp.closed && !unbound_fact_stamp.Deliver(),
               "valid unbound fact updates stamp without callback")) return 1;
    window.reportedKeyWindow = NO;

    // Bind's first phase may update the interval before callback-capable
    // context retention. An old prepared fact must fail closed in that tail.
    auto reentry_source = DesktopRootEvents::Create(window);
    CallbackState reentry_old;
    if (!Check(reentry_source->Bind(window, Record,
                                    {&reentry_old, Retain, Release}),
               "reentry predecessor bind")) return 1;
    auto old_fact = reentry_source->PrepareNotify(
        DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    const auto old_stamp = reentry_source->Snapshot();
    ReentrantPrepare reentrant;
    reentrant.pending = &old_fact;
    reentrant.source = reentry_source.get();
    reentrant.old_revision = old_stamp.state_revision;
    window.reportedKeyWindow = YES;
    DarwinArtDesktopRootEventContext reentrant_context{
        &reentrant, RetainAndDeliverOld, ReleaseReentrant};
    // The replacement context's retain invokes old_fact while Bind's phase-1
    // key transition is committed, before the replacement is published.
    if (!Check(reentry_source->Bind(window, RecordReentrant, reentrant_context),
               "reentrant Bind publishes after its retain tail")) return 1;
    if (!Check(!reentrant.stale_delivery && reentrant.saw_changed_revision &&
                   reentry_old.callbacks == 1 && reentrant.callbacks == 1,
               "new interval has no prior serial and suppresses old prepared fact")) return 1;
    window.reportedKeyWindow = NO;
    if (!Check(reentry_source->Unbind() && reentrant.releases == 1,
               "reentry source settles")) return 1;

    using Peer = darwin_art::window::DesktopRootEventsTestPeer;
    auto serial_exhausted = DesktopRootEvents::Create(window);
    CallbackState serial_state;
    if (!Check(serial_exhausted->Bind(window, Record,
                                      {&serial_state, Retain, Release}),
               "serial boundary bind")) return 1;
    Peer::ExhaustSerial(*serial_exhausted);
    auto serial_fact = serial_exhausted->PrepareNotify(
        DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    if (!Check(!serial_fact.Deliver() && serial_exhausted->closed() &&
                   serial_exhausted->Snapshot().latest_emitted_serial == 0 &&
                   serial_state.callbacks == 1,
               "serial exhaustion fail-closes")) return 1;

    auto revision_exhausted = DesktopRootEvents::Create(window);
    CallbackState revision_state;
    if (!Check(revision_exhausted->Bind(window, Record,
                                        {&revision_state, Retain, Release}),
               "revision boundary bind")) return 1;
    Peer::ExhaustRevision(*revision_exhausted);
    window.reportedKeyWindow = YES;
    auto revision_fact = revision_exhausted->PrepareNotify(
        DARWIN_ART_DESKTOP_ROOT_ACTIVATED);
    if (!Check(!revision_fact.Deliver() && revision_exhausted->closed() &&
                   revision_exhausted->Snapshot().latest_emitted_serial == 0 &&
                   revision_state.callbacks == 1,
               "revision exhaustion fail-closes")) return 1;
    window.reportedKeyWindow = NO;

    // Settle every live binding while its callback context is still in scope.
    // This also makes the test's lifetime contract explicit instead of
    // relying on destruction order at the end of main.
    (void)source->Unbind();
    (void)moved_source->Unbind();
    (void)scope_source->Unbind();
    (void)superseded_source->Unbind();
    (void)rebound_source->Unbind();
    (void)unbound_source->Unbind();
    (void)closed_source->Unbind();
    (void)thread_source->Unbind();
    (void)valid_unbound->Unbind();
    (void)reentry_source->Unbind();
    (void)serial_exhausted->Unbind();
    (void)revision_exhausted->Unbind();

    [window close];
    std::puts("desktop root deferred fact checks passed");
    return 0;
  }
}
