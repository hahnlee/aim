#include "compat/window/desktop_root_target.h"
#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <cassert>
#include <cstdio>
#include <thread>
#include <vector>

namespace darwin_art::window {
struct DesktopRootTargetTestPeer {
  static bool Publish(const std::shared_ptr<DesktopRootTarget>& target) {
    return target->Publish();
  }
};
}
using darwin_art::window::DesktopRootEvents;
using darwin_art::window::DesktopRootTarget;
using darwin_art::window::AcquireProcessDesktopRootTarget;
using darwin_art::window::DesktopRootTargetTestPeer;

std::atomic<int> window_deallocations{0};
std::atomic<bool> wrong_deallocation_thread{false};
@interface TargetTestWindow : NSWindow
@end
@implementation TargetTestWindow
- (void)dealloc {
  if (![NSThread isMainThread]) wrong_deallocation_thread.store(true);
  ++window_deallocations;
}
@end

namespace {
struct Observations {
  int pins = 0;
  bool retired_acquisition = false;
  std::vector<DarwinArtDesktopRootEvent> events;
  static void Retain(void* value) noexcept { ++static_cast<Observations*>(value)->pins; }
  static void Release(void* value) noexcept {
    auto& state = *static_cast<Observations*>(value);
    assert(state.pins > 0);
    --state.pins;
  }
  static void Event(void* value, DarwinArtDesktopRootEvent event) noexcept {
    auto& state = *static_cast<Observations*>(value);
    state.events.push_back(event);
    if (event.kind == DARWIN_ART_DESKTOP_ROOT_CLOSED) {
      // Retirement must unpublish before external callback and unlock first.
      state.retired_acquisition = AcquireProcessDesktopRootTarget() == nullptr;
    }
  }
  DarwinArtDesktopRootObserver Observer() {
    return {Event, {this, Retain, Release}};
  }
};
TargetTestWindow* Window() {
  TargetTestWindow* window = [[TargetTestWindow alloc]
      initWithContentRect:NSMakeRect(0, 0, 100, 100)
      styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
  assert(window != nil);
  window.releasedWhenClosed = NO;
  return window;
}
std::shared_ptr<DesktopRootTarget> Target(NSWindow* window) {
  auto root = DesktopRootEvents::Create(window);
  assert(root != nullptr);
  auto target = DesktopRootTarget::Create(std::move(root), window);
  assert(target != nullptr && target->incarnation() != 0);
  assert(DesktopRootTargetTestPeer::Publish(target));
  return target;
}
}

int main() {
  assert([NSThread isMainThread]);
  std::shared_ptr<DesktopRootTarget> worker_pin;
  std::shared_ptr<DesktopRootTarget> active_tail;
  Observations tail_observed;
  @autoreleasepool {
    assert(AcquireProcessDesktopRootTarget() == nullptr);
    TargetTestWindow* window = Window();
    auto original = Target(window);
    const uint64_t incarnation = original->incarnation();
    auto retained_events = original->RetainEvents();
    assert(retained_events != nullptr && retained_events->incarnation() == incarnation);
    assert(AcquireProcessDesktopRootTarget() == original);
    std::thread acquire([&] {
      worker_pin = AcquireProcessDesktopRootTarget();
      assert(worker_pin != nullptr && worker_pin->RetainEvents() == retained_events);
    });
    acquire.join();
    assert(worker_pin == original);
    Observations observed;
    assert(original->Bind(observed.Observer()));
    assert(observed.events.size() == 1 && observed.pins == 1);
    assert(observed.events[0].incarnation == incarnation);
    assert(observed.events[0].kind == DARWIN_ART_DESKTOP_ROOT_RESIGNED);
    Observations replacement;
    assert(original->Bind(replacement.Observer()));
    assert(observed.pins == 0 && replacement.pins == 1);
    assert(!original->UnbindExpected(&observed));
    assert(replacement.pins == 1); // old cleanup cannot remove successor
    assert(original->UnbindExpected(&replacement));
    assert(replacement.pins == 0);
    observed.events.clear();
    assert(original->Bind(observed.Observer()));

    TargetTestWindow* other_window = Window();
    auto other = Target(other_window);
    assert(other->incarnation() != incarnation);
    assert(AcquireProcessDesktopRootTarget() == nullptr); // never guess active
    // Exact retained capabilities remain unambiguous with multiple roots.
    assert(original->RetainEvents() == retained_events);
    assert(other->RetainEvents() != retained_events);
    assert(other->Retire());
    assert(AcquireProcessDesktopRootTarget() == original);
    assert(original->Retire());
    assert(original->RetainEvents() == nullptr);
    assert(worker_pin->RetainEvents() == nullptr);
    assert(retained_events->Snapshot().closed);
    assert(observed.events.size() == 2 && observed.pins == 0);
    assert(observed.events.back().kind == DARWIN_ART_DESKTOP_ROOT_CLOSED);
    assert(observed.retired_acquisition);
    assert(AcquireProcessDesktopRootTarget() == nullptr);
    // A delayed exact handle cannot bind to this or any successor root.
    assert(worker_pin->incarnation() == incarnation);
    assert(!worker_pin->Bind(observed.Observer()));
    original.reset();
    [window close];
    window = nil;
    other.reset();
    [other_window close];
    other_window = nil;
    TargetTestWindow* tail_window = Window();
    active_tail = Target(tail_window);
    assert(active_tail->Bind(tail_observed.Observer()));
    [tail_window close];
    tail_window = nil;
  }
  // Drain AppKit construction autoreleases before the worker drops the last
  // exact target, so a background final release cannot be masked by the pool.
  std::thread release([held = std::move(worker_pin)]() mutable { held.reset(); });
  release.join();
  // Unlike explicit Retire (which releases the window on main), this drops a
  // still-live target's last reference on the worker and tests its actual tail.
  std::thread release_active([held = std::move(active_tail)]() mutable { held.reset(); });
  release_active.join();
  for (int turn = 0; turn < 30 && window_deallocations.load() < 3; ++turn) {
    @autoreleasepool {
      CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, true);
    }
  }
  assert(window_deallocations.load() == 3);
  assert(!wrong_deallocation_thread.load());
  assert(tail_observed.events.size() == 2 && tail_observed.pins == 0);
  assert(tail_observed.retired_acquisition);
  std::puts("Desktop root target: exact retention/ambiguity/retirement/main-thread release PASS");
}
