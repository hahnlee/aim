#import <AppKit/AppKit.h>

#include "compat/window/desktop_root_events.h"

#include <cstdio>
#include <memory>
#include <limits>
#include <thread>
#include <vector>

namespace darwin_art::window {
struct DesktopRootEventsTestPeer {
  static void ExhaustAttempts(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.next_binding_attempt_ = std::numeric_limits<uint64_t>::max();
  }
  static void ExhaustSerial(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.next_serial_ = std::numeric_limits<uint64_t>::max();
  }
  static void ExhaustRevision(DesktopRootEvents& source) {
    std::lock_guard<std::mutex> lock(source.mutex_);
    source.state_revision_ = std::numeric_limits<uint64_t>::max();
  }
};
}

@interface StampTestWindow : NSWindow
@property(nonatomic, assign) BOOL reportedKeyWindow;
@end

@implementation StampTestWindow
- (BOOL)isKeyWindow {
  // Deterministic host query for the production provider; no provider state
  // is fabricated or cached by DesktopRootEvents itself.
  return self.reportedKeyWindow;
}
@end

namespace {

using darwin_art::window::DesktopRootEvents;

struct CallbackState {
  std::vector<DarwinArtDesktopRootEvent> events;
  int retains = 0;
  int releases = 0;
  DesktopRootEvents* source = nullptr;
  bool close_reentered = false;
  bool callback_saw_committed_stamp = false;
  StampTestWindow* activate_on_retain = nil;
};

void Retain(void* value) noexcept {
  auto* state = static_cast<CallbackState*>(value);
  ++state->retains;
  if (state->activate_on_retain != nil) state->activate_on_retain.reportedKeyWindow = YES;
}
void Release(void* value) noexcept {
  auto* state = static_cast<CallbackState*>(value);
  ++state->releases;
  // Resource release is callback-capable: it must occur outside provider locks.
  if (state->source != nullptr) (void)state->source->Snapshot();
}

void Record(void* value, DarwinArtDesktopRootEvent event) noexcept {
  auto* state = static_cast<CallbackState*>(value);
  state->events.push_back(event);
  if (state->source != nullptr) {
    const auto stamp = state->source->Snapshot();
    state->callback_saw_committed_stamp =
        stamp.latest_emitted_serial == event.serial && stamp.key == event.key_window_snapshot;
  }
  if (event.kind == DARWIN_ART_DESKTOP_ROOT_CLOSED && state->source != nullptr) {
    state->close_reentered = !state->source->Close();
  }
}

struct RebindContext {
  DesktopRootEvents* source = nullptr;
  NSWindow* window = nil;
  CallbackState* replacement = nullptr;
  bool done = false;
  int retains = 0;
  int releases = 0;
};

void RetainReplacement(void* value) noexcept {
  auto* context = static_cast<RebindContext*>(value);
  ++context->retains;
  if (!context->done) {
    context->done = true;
    DarwinArtDesktopRootEventContext replacement_context{
        context->replacement, Retain, Release};
    (void)context->source->Bind(context->window, Record, replacement_context);
  }
}

void ReleaseReplacement(void* value) noexcept {
  ++static_cast<RebindContext*>(value)->releases;
}

struct UnbindContext {
  DesktopRootEvents* source = nullptr;
  bool done = false;
  int retains = 0;
  int releases = 0;
};

void RetainAndUnbind(void* value) noexcept {
  auto* context = static_cast<UnbindContext*>(value);
  ++context->retains;
  if (!context->done) {
    context->done = true;
    (void)context->source->Unbind();
  }
}

void ReleaseUnbind(void* value) noexcept {
  ++static_cast<UnbindContext*>(value)->releases;
}

bool Check(bool value, const char* message) {
  if (value) return true;
  std::fprintf(stderr, "desktop root events failure: %s\n", message);
  return false;
}

}  // namespace

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    StampTestWindow* window = [[StampTestWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    if (!Check(window != nil, "real NSWindow allocation")) return 1;
    window.releasedWhenClosed = NO;
    window.reportedKeyWindow = NO;

    auto source = DesktopRootEvents::Create(window);
    auto successor = DesktopRootEvents::Create(window);
    if (!Check(source != nullptr && successor != nullptr, "source allocation")) return 1;
    if (!Check(source->incarnation() != 0 && source->incarnation() != successor->incarnation(),
               "distinct monotonic incarnations")) return 1;

    const auto initial_stamp = source->Snapshot();
    if (!Check(initial_stamp.state_revision != 0 && !initial_stamp.key &&
                   initial_stamp.latest_emitted_serial == 0 &&
                   !initial_stamp.closed,
               "initial local host stamp")) return 1;

    // Production owner tracking remains live before any observer is present.
    auto unbound_tracking = DesktopRootEvents::Create(window);
    window.reportedKeyWindow = YES;
    if (!Check(!unbound_tracking->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED),
               "unbound validated transition has no observer result")) return 1;
    const auto transitioned_stamp = unbound_tracking->Snapshot();
    if (!Check(transitioned_stamp.key &&
                   transitioned_stamp.state_revision ==
                       initial_stamp.state_revision + 1 &&
                   transitioned_stamp.latest_emitted_serial == 0,
               "unbound transition updates local stamp")) return 1;
    window.reportedKeyWindow = NO;
    if (!Check(!unbound_tracking->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED),
               "unbound validated resignation has no observer result")) return 1;
    window.reportedKeyWindow = YES;
    if (!Check(!unbound_tracking->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED),
               "pre-observer reactivation has no observer")) return 1;
    const auto reactivated = unbound_tracking->Snapshot();
    if (!Check(reactivated.key && reactivated.latest_emitted_serial == 0 &&
                   reactivated.state_revision == transitioned_stamp.state_revision + 2,
               "pre-observer resign/rekey replaces continuous key interval")) return 1;
    CallbackState active_state;
    active_state.source = unbound_tracking.get();
    if (!Check(unbound_tracking->Bind(window, Record, {&active_state, Retain, Release}) &&
                   unbound_tracking->Snapshot().state_revision == reactivated.state_revision &&
                   unbound_tracking->Snapshot().latest_emitted_serial != 0,
               "active initial bind assigns serial without replacing interval")) return 1;
    if (!Check(unbound_tracking->Unbind() &&
                   unbound_tracking->Snapshot().latest_emitted_serial == 0,
               "observer removal clears emitted authority serial")) return 1;
    window.reportedKeyWindow = NO;
    (void)unbound_tracking->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);

    // A separate real owner verifies exact-window admission and same-interval
    // Bind serial allocation without perturbing the main callback assertions.
    auto interval_source = DesktopRootEvents::Create(window);
    CallbackState interval_state;
    if (!Check(interval_source->Bind(window, Record,
                                    {&interval_state, Retain, Release}),
               "same-interval binding")) return 1;
    const auto interval_before = interval_source->Snapshot();
    if (!Check(interval_source->Bind(window, Record,
                                    {&interval_state, Retain, Release}),
               "same-interval replacement binding")) return 1;
    const auto interval_after = interval_source->Snapshot();
    if (!Check(interval_after.state_revision == interval_before.state_revision &&
                   interval_after.latest_emitted_serial >
                       interval_before.latest_emitted_serial,
               "same-state bind keeps revision and emits serial")) return 1;
    NSWindow* wrong_window = [[NSWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 320, 240)
                  styleMask:NSWindowStyleMaskBorderless
                    backing:NSBackingStoreBuffered
                      defer:YES];
    if (!Check(wrong_window != nil &&
                   !interval_source->Bind(wrong_window, Record,
                                          {&interval_state, Retain, Release}),
               "wrong original window rejected")) return 1;
    wrong_window.releasedWhenClosed = NO;
    if (!Check(interval_source->Unbind(), "settle interval callback before fixture teardown")) return 1;

    using Peer = darwin_art::window::DesktopRootEventsTestPeer;
    for (bool initially_bound : {false, true}) {
      auto exhausted = DesktopRootEvents::Create(window);
      CallbackState counted;
      DarwinArtDesktopRootEventContext counted_context{&counted, Retain, Release};
      if (initially_bound && !Check(exhausted->Bind(window, Record, counted_context),
                                   "initial exhaustion binding")) return 1;
      Peer::ExhaustAttempts(*exhausted);
      if (!Check(!exhausted->Bind(window, Record, counted_context) && exhausted->closed() &&
                     counted.retains == (initially_bound ? 1 : 0) &&
                     counted.releases == (initially_bound ? 1 : 0),
                 "attempt exhaustion seals without new retain and retires original")) return 1;
    }
    auto serial_exhausted = DesktopRootEvents::Create(window);
    CallbackState serial_state;
    if (!Check(serial_exhausted->Bind(window, Record, {&serial_state, Retain, Release}),
               "serial boundary initial binding")) return 1;
    Peer::ExhaustSerial(*serial_exhausted);
    if (!Check(!serial_exhausted->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED) &&
                   serial_exhausted->closed() && serial_state.releases == 1 &&
                   serial_state.events.size() == 1,
               "serial exhaustion retires original without wrapping record")) return 1;

    CallbackState state;
    state.source = source.get();
    DarwinArtDesktopRootEventContext context{&state, Retain, Release};
    if (!Check(source->Bind(window, Record, context), "bind real window")) return 1;
    if (!Check(state.retains == 1 && state.events.size() == 1,
               "bind retains context and emits baseline")) return 1;
    DesktopRootEvents::LocalStamp worker_stamp;
    std::thread snapshot_thread([&] { worker_stamp = source->Snapshot(); });
    snapshot_thread.join();
    const auto bound_stamp = source->Snapshot();
    if (!Check(worker_stamp.incarnation == bound_stamp.incarnation &&
                   worker_stamp.state_revision == bound_stamp.state_revision &&
                   worker_stamp.latest_emitted_serial == bound_stamp.latest_emitted_serial &&
                   worker_stamp.key == bound_stamp.key &&
                   worker_stamp.closed == bound_stamp.closed,
               "any-thread immutable stamp snapshot")) return 1;
    if (!Check(state.callback_saw_committed_stamp,
               "baseline callback observes committed stamp")) return 1;
    if (!Check(state.events[0].kind == DARWIN_ART_DESKTOP_ROOT_RESIGNED &&
                   state.events[0].key_window_snapshot == [window isKeyWindow],
               "baseline key-window snapshot")) return 1;
    if (!Check(!source->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED),
               "contradictory superseded activation rejected")) return 1;
    if (!Check(source->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED), "validated resign notify")) return 1;
    if (!Check(source->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED), "resigned notify")) return 1;
    if (!Check(state.events[1].serial < state.events[2].serial,
               "strict serial monotonicity")) return 1;

    std::shared_ptr<DesktopRootEvents> source_for_thread = source;
    bool wrong_thread_result = true;
    std::thread wrong_thread([&] {
      wrong_thread_result = source_for_thread->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED);
    });
    wrong_thread.join();
    if (!Check(!wrong_thread_result, "wrong-thread notify rejected")) return 1;

    if (!Check(source->Unbind(), "unbind")) return 1;
    if (!Check(state.releases == 1 && !source->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED),
               "unbind releases context and removes callback")) return 1;

    // A retain hook may reenter with a newer binding. The older Bind attempt
    // must not publish over that newer binding or lose its context release.
    auto reentrant_source = DesktopRootEvents::Create(window);
    CallbackState replacement;
    RebindContext rebind;
    rebind.source = reentrant_source.get();
    rebind.window = window;
    rebind.replacement = &replacement;
    DarwinArtDesktopRootEventContext reentrant_context{
        &rebind, RetainReplacement, ReleaseReplacement};
    if (!Check(!reentrant_source->Bind(window, Record, reentrant_context),
               "superseded bind is rejected")) return 1;
    if (!Check(rebind.retains == 1 && rebind.releases == 1 && replacement.events.size() == 1,
               "reentrant bind keeps newer callback")) return 1;
    if (!Check(reentrant_source->Unbind() && replacement.releases == 1,
               "replacement unbind releases context")) return 1;

    auto unbind_source = DesktopRootEvents::Create(window);
    UnbindContext unbind;
    unbind.source = unbind_source.get();
    DarwinArtDesktopRootEventContext unbind_context{
        &unbind, RetainAndUnbind, ReleaseUnbind};
    if (!Check(!unbind_source->Bind(window, Record, unbind_context),
               "reentrant unbind supersedes bind")) return 1;
    if (!Check(unbind.retains == 1 && unbind.releases == 1 &&
                   !unbind_source->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED),
               "reentrant unbind leaves no callback installed")) return 1;

    state.source = source.get();
    if (!Check(source->Bind(window, Record, context), "rebind after unbind")) return 1;
    const auto before_close = source->Snapshot();
    auto terminal = source->PrepareClose();
    const auto prepared_close = source->Snapshot();
    if (!Check(prepared_close.closed &&
                   prepared_close.state_revision == before_close.state_revision + 1 &&
                   prepared_close.latest_emitted_serial != 0,
               "close commits visible stamp before deferred callback")) return 1;
    if (!Check(source->closed() && !source->Close() &&
                   !source->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED),
               "prepared close seals before other callbacks")) return 1;
    auto moved_terminal = std::move(terminal);
    if (!Check(!terminal.Deliver() && moved_terminal.Deliver() && !moved_terminal.Deliver(),
               "terminal capability delivers once after move")) return 1;
    if (!Check(state.events.back().kind == DARWIN_ART_DESKTOP_ROOT_CLOSED &&
                   state.close_reentered && state.callback_saw_committed_stamp &&
                   source->closed() && state.releases == 2,
               "close callback is sealed, reentry-safe, and releases context")) return 1;
    if (!Check(!source->Close() && !source->Unbind() &&
                   !source->Notify(DARWIN_ART_DESKTOP_ROOT_RESIGNED),
               "closed source rejects all later lifecycle operations")) return 1;

    auto deferred_source = DesktopRootEvents::Create(window);
    CallbackState deferred_state;
    if (!Check(deferred_source->Bind(window, Record, {&deferred_state, Retain, Release}),
               "auto-settlement initial binding")) return 1;
    {
      auto pending = deferred_source->PrepareClose();
      bool worker_delivered = true;
      std::thread worker([&] { worker_delivered = pending.Deliver(); });
      worker.join();
      if (!Check(!worker_delivered && deferred_state.events.size() == 1,
                 "wrong-thread delivery preserves original terminal obligation")) return 1;
    }
    if (!Check(deferred_state.events.size() == 2 && deferred_state.releases == 1 &&
                   deferred_state.events.back().kind == DARWIN_ART_DESKTOP_ROOT_CLOSED,
               "main-thread destruction delivers unconsumed terminal once")) return 1;

    auto revision_exhausted = DesktopRootEvents::Create(window);
    CallbackState revision_state;
    revision_state.source = revision_exhausted.get();
    if (!Check(revision_exhausted->Bind(window, Record,
                                       {&revision_state, Retain, Release}),
               "revision exhaustion binding")) return 1;
    using Peer = darwin_art::window::DesktopRootEventsTestPeer;
    Peer::ExhaustRevision(*revision_exhausted);
    window.reportedKeyWindow = YES;
    if (!Check(!revision_exhausted->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED) &&
                   revision_exhausted->closed() && revision_state.releases == 1 &&
                   revision_exhausted->Snapshot().closed &&
                   revision_exhausted->Snapshot().latest_emitted_serial == 0,
               "revision exhaustion fail-closes with visible stamp")) return 1;
    window.reportedKeyWindow = NO;

    auto bind_revision_exhausted = DesktopRootEvents::Create(window);
    CallbackState original_state;
    original_state.source = bind_revision_exhausted.get();
    if (!Check(bind_revision_exhausted->Bind(window, Record,
                   {&original_state, Retain, Release}), "bind-overflow predecessor")) return 1;
    Peer::ExhaustRevision(*bind_revision_exhausted);
    CallbackState candidate_state;
    candidate_state.source = bind_revision_exhausted.get();
    candidate_state.activate_on_retain = window;
    if (!Check(!bind_revision_exhausted->Bind(window, Record,
                   {&candidate_state, Retain, Release}) &&
                   bind_revision_exhausted->Snapshot().closed &&
                   original_state.releases == 1 && candidate_state.releases == 1,
               "post-retain revision overflow releases both contexts outside lock")) return 1;
    window.reportedKeyWindow = NO;

    [window close];
    [wrong_window close];
    std::puts("desktop root events checks passed");
    return 0;
  }
}
