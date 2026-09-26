#include "appkit_window_delegate.h"
#include "../darwin_surface_internal.h"
#include "display_output.h"
#include "root_geometry.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iostream>

namespace {
// The Android raster scale for this root: DARWIN_ART_WINDOW_SCALE pins it (as
// at window creation), otherwise it is the display's backing scale, so a 2x
// scanout stays a 2x Android raster on every display.
uint32_t RasterScale(NSWindow* window) {
  const char* pinned = std::getenv("DARWIN_ART_WINDOW_SCALE");
  if (pinned != nullptr && std::strcmp(pinned, "2") == 0) return 2;
  if (pinned != nullptr && std::strcmp(pinned, "1") == 0) return 1;
  const CGFloat scale = window == nil ? 1.0 : window.backingScaleFactor;
  return static_cast<uint32_t>(std::clamp<CGFloat>(std::round(scale), 1.0, 4.0));
}

// CGDirectDisplayID of the screen showing the window, 0 when off screen.
uint32_t DisplayId(NSWindow* window) {
  NSNumber* number = window.screen.deviceDescription[@"NSScreenNumber"];
  return number == nil ? 0 : number.unsignedIntValue;
}

void PublishRootFact(DarwinArtMetalView* view, NSWindow* window, const char* what) {
  const NSRect bounds = view.bounds;
  if (darwin_art::window::RootGeometryReports::Process().Publish(
          static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(bounds.size.width))),
          static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(bounds.size.height))),
          RasterScale(window), DisplayId(window))) {
    std::cerr << "DARWIN_ART window " << what << " reported scale=" << RasterScale(window)
              << " display=" << DisplayId(window) << "\n";
  }
}
}  // namespace

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
    // Delayed task revisions must not resize a closed root; stop reports.
    if (self.surface->visible) darwin_art::window::RootGeometryReports::Process().Close();
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
  if (surface->visible) {
    // Report the host fact; Android ActivityTask owns the resulting task
    // extent, and its revision resizes the backing (root_geometry.mm).
    const bool reported = darwin_art::window::RootGeometryReports::Process().Publish(
        static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(bounds.size.width))),
        static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(bounds.size.height))),
        RasterScale(surface->window), DisplayId(surface->window));
    if (darwin_art::window::RootGeometryOwned(surface)) {
      if (reported) {
        std::cerr << "DARWIN_ART window resize reported points="
                  << bounds.size.width << "x" << bounds.size.height << "\n";
      }
      return;
    }
  }
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
- (void)windowDidChangeBackingProperties:(NSNotification*)notification {
  (void)notification;
  DarwinArtMetalView* view = self.view;
  DarwinArtSurface* surface = self.surface;
  if (surface == nullptr || view == nil || [view ownerSurface] != surface ||
      !surface->visible || !darwin_art::window::RootGeometryOwned(surface)) {
    return;
  }
  // The window moved to a display of another backing scale. The layer takes
  // the new scale; Android answers the host fact with a revision that changes
  // density and the raster together, and that revision resizes the backing.
  view.metalLayer.contentsScale = RasterScale(surface->window);
  [view updateDrawableSize];
  PublishRootFact(view, surface->window, "backing scale");
}
- (void)windowDidChangeScreen:(NSNotification*)notification {
  (void)notification;
  DarwinArtMetalView* view = self.view;
  DarwinArtSurface* surface = self.surface;
  if (surface == nullptr || view == nil || [view ownerSurface] != surface ||
      !surface->visible || !darwin_art::window::RootGeometryOwned(surface)) {
    return;
  }
  // The display Android reports (DisplayInfo) follows the root's screen.
  PublishRootFact(view, surface->window, "screen");
}
@end

namespace darwin_art::window {
uint32_t SurfaceRasterScale(NSWindow* window) { return RasterScale(window); }
uint32_t SurfaceDisplayId(NSWindow* window) { return DisplayId(window); }

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
