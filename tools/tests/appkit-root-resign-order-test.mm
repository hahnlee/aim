#include "compat/darwin_surface_internal.h"
#include "compat/window/appkit_window_delegate.h"
#include "compat/window/desktop_root_surface.h"

#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <vector>

using aim::window::DesktopRootEvents;

@interface ResignTestWindow : NSWindow
@property(nonatomic, assign) BOOL reportedKey;
@end
@implementation ResignTestWindow
- (BOOL)isKeyWindow { return self.reportedKey; }
@end

namespace {
enum class Reentry { None, Close, Rekey, Rebind, Destroy };
struct State {
  std::vector<AimDesktopRootEvent> events;
  bool cancelling = false;
  bool cancelled = false;
  uint64_t original_revision = 0;
  int pins = 0;
};
void Retain(void* opaque) noexcept { ++static_cast<State*>(opaque)->pins; }
void Release(void* opaque) noexcept { --static_cast<State*>(opaque)->pins; }
void Record(void* opaque, AimDesktopRootEvent event) noexcept {
  auto& state = *static_cast<State*>(opaque);
  if (event.kind == AIM_DESKTOP_ROOT_RESIGNED && !state.cancelling)
    assert(state.cancelled);
  state.events.push_back(event);
}
AimSurfaceResult UnusedResize(AimSurface*, uint32_t, uint32_t,
                                   bool, uint32_t, uint32_t) {
  std::abort();
}
}

// Only the host cancellation boundary is deterministic. The root provider,
// NSWindowDelegate implementation, surface setter/destructor are production.
@interface CancellationWitnessView : NSView {
 @public
  std::shared_ptr<DesktopRootEvents> root;
  State* state;
  Reentry reentry;
  ResignTestWindow* window;
  std::unique_ptr<AimSurface>* surface;
}
- (void)cancelPointerStream;
- (void)setOwnerSurface:(AimSurface*)owner;
@end
@implementation CancellationWitnessView
- (void)setOwnerSurface:(AimSurface*)owner { assert(owner == nullptr); }
- (void)cancelPointerStream {
  const auto stamp = root->Snapshot();
  assert(!stamp.key && !stamp.closed);
  assert(stamp.state_revision > state->original_revision);
  assert(state->events.empty());
  state->cancelling = true;
  switch (reentry) {
    case Reentry::None: break;
    case Reentry::Close: assert(root->Close()); break;
    case Reentry::Rekey:
      window.reportedKey = YES;
      assert(root->Notify(AIM_DESKTOP_ROOT_ACTIVATED));
      break;
    case Reentry::Rebind:
      assert(root->Bind(window, Record, {state, Retain, Release}));
      break;
    case Reentry::Destroy:
      assert((*surface)->desktop_root_target->Close());
      surface->reset();
      break;
  }
  state->cancelling = false;
  state->cancelled = true;
}
@end

int main() {
  @autoreleasepool {
    [NSApplication sharedApplication];
    for (Reentry reentry : {Reentry::None, Reentry::Close, Reentry::Rekey,
                           Reentry::Rebind, Reentry::Destroy}) {
      State state;
      auto surface = std::make_unique<AimSurface>();
      ResignTestWindow* window = [[ResignTestWindow alloc]
          initWithContentRect:NSMakeRect(0, 0, 100, 100)
          styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered
          defer:YES];
      window.releasedWhenClosed = NO;
      window.reportedKey = YES;
      surface->window = window;
      assert(aim::window::InitializeDesktopRoot(surface.get()) == AIM_SURFACE_OK);
      auto root = surface->desktop_root_events;
      assert(root->Bind(window, Record, {&state, Retain, Release}));
      state.events.clear();
      state.original_revision = root->Snapshot().state_revision;
      CancellationWitnessView* witness = [[CancellationWitnessView alloc] init];
      witness->root = root;
      witness->state = &state;
      witness->reentry = reentry;
      witness->window = window;
      witness->surface = &surface;
      surface->view = (AimMetalView*)witness;
      id<NSWindowDelegate> delegate = aim::window::CreateSurfaceWindowDelegate(
          surface.get(), UnusedResize);
      assert(delegate != nil);
      window.reportedKey = NO;
      [delegate windowDidResignKey:[NSNotification notificationWithName:
          NSWindowDidResignKeyNotification object:window]];
      assert(state.cancelled && state.events.size() == 1);
      const auto expected = reentry == Reentry::Rekey ? AIM_DESKTOP_ROOT_ACTIVATED :
          (reentry == Reentry::Close || reentry == Reentry::Destroy) ?
          AIM_DESKTOP_ROOT_CLOSED : AIM_DESKTOP_ROOT_RESIGNED;
      assert(state.events[0].kind == expected);
      assert(state.events[0].serial == root->Snapshot().latest_emitted_serial);
      if (surface != nullptr) (void)surface->desktop_root_target->Close();
      assert(state.pins == 0);
      [window close];
    }
  }
  std::puts("AppKit root resign preparation/cancellation/reentry: PASS");
}
