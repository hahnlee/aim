#include "root_geometry.h"

namespace darwin_art::window {

RootGeometryReports& RootGeometryReports::Process() {
  static RootGeometryReports reports;
  return reports;
}

bool RootGeometryReports::Publish(uint32_t points_width, uint32_t points_height,
                                  uint32_t backing_scale, uint32_t display_id) {
  if (points_width == 0 || points_height == 0) return false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_) return false;
    const uint32_t current_scale = latest_.serial != 0 ? latest_.backing_scale : known_scale_;
    const uint32_t scale = backing_scale != 0 ? backing_scale : current_scale;
    const uint32_t current_display = latest_.serial != 0 ? latest_.display_id : known_display_;
    const uint32_t display = display_id != 0 ? display_id : current_display;
    const bool same_as_latest = latest_.serial != 0 &&
        latest_.points_width == points_width && latest_.points_height == points_height &&
        latest_.backing_scale == scale && latest_.display_id == display;
    const bool known = known_width_ == points_width && known_height_ == points_height &&
        known_scale_ == scale && known_display_ == display;
    // AppKit echoes of an extent Android (or window creation) set are not
    // facts; a user returning to that extent after resizing away still is.
    if (same_as_latest || (known && !moved_from_known_)) return false;
    if (!known) moved_from_known_ = true;
    ++latest_.serial;
    latest_.points_width = points_width;
    latest_.points_height = points_height;
    latest_.backing_scale = scale;
    latest_.display_id = display;
  }
  changed_.notify_all();
  return true;
}

bool RootGeometryReports::PublishHidden(bool hidden) {
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_ || latest_.quit || latest_.hidden == hidden) return false;
    if (latest_.serial == 0) {
      latest_.points_width = known_width_;
      latest_.points_height = known_height_;
      latest_.backing_scale = known_scale_;
      latest_.display_id = known_display_;
    }
    ++latest_.serial;
    latest_.hidden = hidden;
  }
  changed_.notify_all();
  return true;
}

bool RootGeometryReports::PublishQuit() {
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closed_ || latest_.quit) return false;
    if (latest_.serial == 0) {
      latest_.points_width = known_width_;
      latest_.points_height = known_height_;
      latest_.backing_scale = known_scale_;
      latest_.display_id = known_display_;
    }
    ++latest_.serial;
    latest_.quit = true;
  }
  changed_.notify_all();
  return true;
}

void RootGeometryReports::NoteKnownExtent(uint32_t points_width,
                                          uint32_t points_height,
                                          uint32_t backing_scale,
                                          uint32_t display_id) {
  std::lock_guard<std::mutex> lock(mutex_);
  known_width_ = points_width;
  known_height_ = points_height;
  if (backing_scale != 0) known_scale_ = backing_scale;
  if (display_id != 0) known_display_ = display_id;
  moved_from_known_ = false;
}

bool RootGeometryReports::Await(uint64_t after_serial, RootGeometryReport* report) {
  if (report == nullptr) return false;
  std::unique_lock<std::mutex> lock(mutex_);
  changed_.wait(lock, [&] { return closed_ || latest_.serial > after_serial; });
  if (closed_) return false;
  *report = latest_;
  return true;
}

void RootGeometryReports::Close() {
  {
    std::lock_guard<std::mutex> lock(mutex_);
    closed_ = true;
  }
  changed_.notify_all();
}

uint32_t RootGeometryReports::backing_scale() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return latest_.serial != 0 && latest_.backing_scale != 0 ? latest_.backing_scale
                                                             : known_scale_;
}

uint32_t RootGeometryReports::display_id() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return latest_.serial != 0 && latest_.display_id != 0 ? latest_.display_id
                                                          : known_display_;
}

uint64_t RootGeometryReports::latest_serial() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return latest_.serial;
}

RootGeometryStatus RootGeometryApplier::Apply(
    const RootGeometryPublication& publication, uint64_t latest_host_serial,
    RootGeometryHost* host, RootGeometryReports* reports) {
  if (host == nullptr || publication.android_width == 0 ||
      publication.android_height == 0 || publication.points_width == 0 ||
      publication.points_height == 0) {
    return RootGeometryStatus::kInvalid;
  }
  if (host->closed()) return RootGeometryStatus::kClosed;
  if (owned_ && publication.revision <= revision_) return RootGeometryStatus::kStale;
  // Android-initiated extents (orientation, launch) resize the host window only
  // when the owner has seen every host report; otherwise the user is still
  // resizing and a newer revision answering that report will follow.
  if (publication.host_serial >= latest_host_serial) {
    uint32_t width = 0;
    uint32_t height = 0;
    host->content_points(&width, &height);
    if (reports != nullptr) {
      reports->NoteKnownExtent(publication.points_width, publication.points_height);
    }
    if (width != publication.points_width || height != publication.points_height) {
      host->set_content_points(publication.points_width, publication.points_height);
    }
  }
  const DarwinArtSurfaceResult result =
      host->resize_backing(publication.android_width, publication.android_height);
  if (result == DARWIN_ART_SURFACE_ALLOCATION_FAILED) {
    // Keep the previous revision: the next publication retries allocation.
    return RootGeometryStatus::kAllocationFailed;
  }
  if (result != DARWIN_ART_SURFACE_OK) {
    return host->closed() ? RootGeometryStatus::kClosed : RootGeometryStatus::kInvalid;
  }
  owned_ = true;
  revision_ = publication.revision;
  return RootGeometryStatus::kApplied;
}

}  // namespace darwin_art::window
