#include "root_geometry.h"

#include "../darwin_surface_internal.h"

#include <algorithm>
#include <cmath>
#include <iostream>
#include <unistd.h>

namespace darwin_art::window {

namespace {

class SurfaceRootGeometryHost final : public RootGeometryHost {
 public:
  explicit SurfaceRootGeometryHost(DarwinArtSurface* surface) : surface_(surface) {}

  bool closed() const override {
    return surface_->window == nil ||
        surface_->window_closed.load(std::memory_order_acquire);
  }

  void content_points(uint32_t* width, uint32_t* height) const override {
    const NSRect bounds = surface_->view.bounds;
    *width = static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(bounds.size.width)));
    *height = static_cast<uint32_t>(std::max<CGFloat>(1.0, std::ceil(bounds.size.height)));
  }

  void set_content_points(uint32_t width, uint32_t height) override {
    // Keep the top-left placement; setContentSize: anchors the lower-left.
    const NSPoint top_left = NSMakePoint(NSMinX(surface_->window.frame),
                                         NSMaxY(surface_->window.frame));
    [surface_->window setContentSize:NSMakeSize(width, height)];
    [surface_->window setFrameTopLeftPoint:top_left];
  }

  DarwinArtSurfaceResult resize_backing(uint32_t android_width,
                                        uint32_t android_height) override {
    // Scanout backing follows the root's real content pixels; the Android
    // extent is the task raster SurfaceFlinger composes into it.
    [surface_->view updateDrawableSize];
    const CGFloat scale = surface_->view.metalLayer.contentsScale > 0.0
        ? surface_->view.metalLayer.contentsScale : 1.0;
    const NSRect bounds = surface_->view.bounds;
    const uint32_t width = static_cast<uint32_t>(
        std::max<CGFloat>(1.0, std::ceil(bounds.size.width * scale)));
    const uint32_t height = static_cast<uint32_t>(
        std::max<CGFloat>(1.0, std::ceil(bounds.size.height * scale)));
    return ResizeSurfaceBackingOnMain(surface_, width, height, false, android_width,
                                      android_height);
  }

 private:
  DarwinArtSurface* surface_;
};

RootGeometryApplier& ProcessApplier() {
  static RootGeometryApplier applier;
  return applier;
}

}  // namespace

RootGeometryStatus ApplyProcessRootGeometry(
    const RootGeometryPublication& publication) {
  if (![NSThread isMainThread]) return RootGeometryStatus::kInvalid;
  DarwinArtSurface* surface = g_active_gpu_surface.load(std::memory_order_acquire);
  if (surface == nullptr || !surface->visible || surface->view == nil) {
    return RootGeometryStatus::kClosed;
  }
  auto& reports = RootGeometryReports::Process();
  SurfaceRootGeometryHost host(surface);
  const auto status = ProcessApplier().Apply(publication, reports.latest_serial(),
                                             &host, &reports);
  const auto backing = surface->backing_owner.Acquire();
  std::cerr << "DARWIN_ART root geometry pid=" << getpid()
            << " revision=" << publication.revision
            << " status=" << static_cast<int32_t>(status)
            << " android=" << publication.android_width << "x"
            << publication.android_height << " points="
            << publication.points_width << "x" << publication.points_height
            << " backing="
            << (backing ? backing->physical_width() : 0) << "x"
            << (backing ? backing->physical_height() : 0) << "\n";
  return status;
}

bool HideProcessRoot() {
  if (![NSThread isMainThread]) return false;
  DarwinArtSurface* surface = g_active_gpu_surface.load(std::memory_order_acquire);
  if (surface == nullptr || !surface->visible || surface->window == nil ||
      !surface->window.visible) {
    return false;
  }
  // windowShouldClose: orders the window out and publishes the hidden root.
  [surface->window performClose:nil];
  return true;
}

uint32_t ProcessRootRasterScale() {
  return ProcessHasVisibleRoot() ? RootGeometryReports::Process().backing_scale() : 0;
}

uint32_t ProcessRootDisplayId() {
  return ProcessHasVisibleRoot() ? RootGeometryReports::Process().display_id() : 0;
}

bool ProcessHasVisibleRoot() {
  DarwinArtSurface* surface = g_active_gpu_surface.load(std::memory_order_acquire);
  return surface != nullptr && surface->visible;
}

bool RootGeometryOwned(DarwinArtSurface* surface) {
  return surface != nullptr &&
      surface == g_active_gpu_surface.load(std::memory_order_acquire) &&
      ProcessApplier().owned();
}

}  // namespace darwin_art::window
