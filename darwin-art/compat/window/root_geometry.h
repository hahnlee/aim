#pragma once

#include <condition_variable>
#include <cstdint>
#include <mutex>

#include "../darwin_surface_bridge.h"

namespace darwin_art::window {

// Host fact: the desktop root's content extent in macOS points after a user or
// window-manager resize, and the backing scale of the display it is on (a
// window moved between 1x and 2x displays). Android pixels and density are
// never derived here.
struct RootGeometryReport {
  uint64_t serial = 0;
  uint32_t points_width = 0;
  uint32_t points_height = 0;
  uint32_t backing_scale = 0;
  // CGDirectDisplayID of the screen the root is on (0 unknown).
  uint32_t display_id = 0;
};

// Process-wide latest-wins slot between AppKit (producer) and the Android
// reporter thread (consumer). AppKit never calls into Java: it only stores the
// newest extent and signals. Extents equal to the last known root extent
// (creation size, applied task revisions, their AppKit echoes) are not facts.
class RootGeometryReports final {
 public:
  static RootGeometryReports& Process();

  // AppKit main thread. Returns true when a new report was published. A zero
  // backing scale leaves the last known scale unchanged.
  bool Publish(uint32_t points_width, uint32_t points_height,
               uint32_t backing_scale = 0, uint32_t display_id = 0);
  // AppKit main thread. Records an extent the root now has because Android
  // geometry (or window creation) set it, not because the user resized.
  void NoteKnownExtent(uint32_t points_width, uint32_t points_height,
                       uint32_t backing_scale = 0, uint32_t display_id = 0);
  // Blocks until a report newer than `after_serial` exists. False once closed.
  bool Await(uint64_t after_serial, RootGeometryReport* report);
  // Root closed or process shutdown; wakes the consumer permanently.
  void Close();
  uint64_t latest_serial() const;
  // The root's current backing scale: the latest report's, else creation's.
  uint32_t backing_scale() const;
  // The root's current display: the latest report's, else creation's.
  uint32_t display_id() const;

  RootGeometryReports() = default;
  RootGeometryReports(const RootGeometryReports&) = delete;
  RootGeometryReports& operator=(const RootGeometryReports&) = delete;

 private:
  mutable std::mutex mutex_;
  std::condition_variable changed_;
  RootGeometryReport latest_{};
  uint32_t known_width_ = 0;
  uint32_t known_height_ = 0;
  uint32_t known_scale_ = 0;
  uint32_t known_display_ = 0;
  bool moved_from_known_ = false;
  bool closed_ = false;
};

// One Android task revision addressed to this process's desktop root.
struct RootGeometryPublication {
  uint64_t revision = 0;
  // Newest host report serial the geometry owner had seen.
  uint64_t host_serial = 0;
  uint32_t android_width = 0;
  uint32_t android_height = 0;
  uint32_t points_width = 0;
  uint32_t points_height = 0;
};

enum class RootGeometryStatus : int32_t {
  kApplied = 0,
  kStale = 1,
  kClosed = 2,
  kAllocationFailed = 3,
  kInvalid = 4,
};

// Resizes the backing/scanout and optionally the host window; see surface
// internals. Injected so the policy below is testable without AppKit.
struct RootGeometryHost {
  virtual ~RootGeometryHost() = default;
  virtual bool closed() const = 0;
  virtual void content_points(uint32_t* width, uint32_t* height) const = 0;
  virtual void set_content_points(uint32_t width, uint32_t height) = 0;
  virtual DarwinArtSurfaceResult resize_backing(uint32_t android_width,
                                                uint32_t android_height) = 0;
};

// Per-root application state; revisions only ever move forward, and a failed
// allocation leaves the root at its previous revision so a later one retries.
class RootGeometryApplier final {
 public:
  RootGeometryStatus Apply(const RootGeometryPublication& publication,
                           uint64_t latest_host_serial, RootGeometryHost* host,
                           RootGeometryReports* reports);
  bool owned() const { return owned_; }
  uint64_t revision() const { return revision_; }

 private:
  bool owned_ = false;
  uint64_t revision_ = 0;
};

// AppKit main thread: applies `publication` to the process's visible root.
RootGeometryStatus ApplyProcessRootGeometry(
    const RootGeometryPublication& publication);
// Any thread: whether this process published a visible desktop root.
bool ProcessHasVisibleRoot();
// The visible root's Android raster scale, or 0 without a visible root.
uint32_t ProcessRootRasterScale();
// The visible root's CGDirectDisplayID, or 0.
uint32_t ProcessRootDisplayId();
// AppKit main thread: whether Android task geometry owns this root's extent.
bool RootGeometryOwned(DarwinArtSurface* surface);

}  // namespace darwin_art::window
