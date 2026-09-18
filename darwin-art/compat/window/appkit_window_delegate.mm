#include "appkit_window_delegate.h"
#include "../darwin_surface_internal.h"
#include "display_output.h"

#include <algorithm>
#include <cmath>
#include <iostream>

@interface DarwinArtSurfaceWindowDelegate : NSObject <NSWindowDelegate>
@property(nonatomic, weak) DarwinArtMetalView* view;
@property(nonatomic, assign) DarwinArtSurface* surface;
@property(nonatomic, assign) darwin_art::window::SurfaceResize resize;
@end

@implementation DarwinArtSurfaceWindowDelegate
- (void)windowDidBecomeKey:(NSNotification*)notification {
  (void)notification;
  auto root = self.surface == nullptr ? nullptr : self.surface->desktop_root_events;
  if (root != nullptr) (void)root->Notify(DARWIN_ART_DESKTOP_ROOT_ACTIVATED);
}
- (void)windowDidResignKey:(NSNotification*)notification {
  (void)notification;
  auto root = self.surface == nullptr ? nullptr : self.surface->desktop_root_events;
  __strong DarwinArtMetalView* view = self.view;
  // Commit the validated host interval before cancellation can reenter input.
  // Java observation stays behind the cancellation tail, as on terminal close.
  auto fact = root == nullptr ? darwin_art::window::DesktopRootEvents::DeferredFact()
                             : root->PrepareNotify(DARWIN_ART_DESKTOP_ROOT_RESIGNED);
  [view cancelPointerStream];
  (void)fact.Deliver();
}
- (void)windowWillClose:(NSNotification*)notification {
  (void)notification;
  auto root = self.surface == nullptr ? nullptr : self.surface->desktop_root_events;
  auto target = self.surface == nullptr ? nullptr : self.surface->desktop_root_target;
  DarwinArtMetalView* view = self.view;
  if (self.surface != nullptr) {
    RetireDisplayOutput(self.surface);
    self.surface->window_closed.store(true, std::memory_order_release);
  }
  auto terminal = root == nullptr ? darwin_art::window::DesktopRootEvents::DeferredClose()
                                  : root->PrepareClose();
  [view cancelPointerStream];
  (void)terminal.Deliver();
  // PrepareClose/Deliver above preserves the established seal -> pointer
  // cancellation -> terminal callback ordering.  Retire the exact target
  // publication only after that callback tail has settled.
  if (target != nullptr) (void)target->Close();
}
- (void)windowDidResize:(NSNotification*)notification {
  (void)notification;
  DarwinArtMetalView* view = self.view;
  DarwinArtSurface* surface = self.surface;
  const auto resize = self.resize;
  [view cancelPointerStream];
  if (surface == nullptr || view == nil || [view ownerSurface] != surface) return;
  [view updateDrawableSize];
  const CGFloat scale = view.metalLayer.contentsScale > 0.0
                            ? view.metalLayer.contentsScale : 1.0;
  const NSRect bounds = view.bounds;
  const uint32_t width = static_cast<uint32_t>(std::max<CGFloat>(
      1.0, std::ceil(bounds.size.width * scale)));
  const uint32_t height = static_cast<uint32_t>(std::max<CGFloat>(
      1.0, std::ceil(bounds.size.height * scale)));
  uint32_t logical_width = width;
  uint32_t logical_height = height;
  if (surface->scale_to_display) {
    logical_width = static_cast<uint32_t>(std::max<CGFloat>(
        1.0, std::ceil(bounds.size.width)));
    logical_height = static_cast<uint32_t>(std::max<CGFloat>(
        1.0, std::ceil(bounds.size.height)));
  }
  // Physical backing and host points are separate. Android projection is
  // configured by its own owner, not reconstructed in this delegate.
  const DarwinArtSurfaceResult result = resize(surface, width,
      height, false, logical_width, logical_height);
  if (result != DARWIN_ART_SURFACE_OK) {
    std::cerr << "DARWIN_ART window resize failed status=" << result
              << " width=" << width << " height=" << height << "\n";
  } else {
    std::cerr << "DARWIN_ART window resize pixels=" << width << "x"
              << height << "\n";
  }
}
@end

namespace darwin_art::window {
id<NSWindowDelegate> CreateSurfaceWindowDelegate(DarwinArtSurface* surface,
                                               SurfaceResize resize) {
  if (![NSThread isMainThread] || surface == nullptr || resize == nullptr)
    return nil;
  DarwinArtSurfaceWindowDelegate* delegate =
      [[DarwinArtSurfaceWindowDelegate alloc] init];
  delegate.view = surface->view;
  delegate.surface = surface;
  delegate.resize = resize;
  return delegate;
}
}  // namespace darwin_art::window
