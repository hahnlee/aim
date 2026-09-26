#include "compat/window/root_geometry.h"

#include <cassert>
#include <cstdio>
#include <thread>

using darwin_art::window::RootGeometryApplier;
using darwin_art::window::RootGeometryHost;
using darwin_art::window::RootGeometryPublication;
using darwin_art::window::RootGeometryReport;
using darwin_art::window::RootGeometryReports;
using darwin_art::window::RootGeometryStatus;

namespace {
struct FakeHost final : RootGeometryHost {
  uint32_t width = 360;
  uint32_t height = 640;
  bool is_closed = false;
  int set_calls = 0;
  int resize_calls = 0;
  DarwinArtSurfaceResult next = DARWIN_ART_SURFACE_OK;
  uint32_t android_width = 0;
  uint32_t android_height = 0;
  RootGeometryReports* echo = nullptr;

  bool closed() const override { return is_closed; }
  void content_points(uint32_t* w, uint32_t* h) const override { *w = width; *h = height; }
  void set_content_points(uint32_t w, uint32_t h) override {
    ++set_calls;
    width = w;
    height = h;
    // AppKit delivers windowDidResize synchronously for setContentSize:.
    if (echo != nullptr) (void)echo->Publish(w, h);
  }
  DarwinArtSurfaceResult resize_backing(uint32_t w, uint32_t h) override {
    ++resize_calls;
    if (next == DARWIN_ART_SURFACE_OK) {
      android_width = w;
      android_height = h;
    }
    return next;
  }
};

RootGeometryPublication Publication(uint64_t revision, uint64_t serial, uint32_t w,
                                    uint32_t h, uint32_t scale = 2) {
  return {revision, serial, w, h, w / scale, h / scale};
}

void CreationAndEchoesAreNotFacts() {
  RootGeometryReports reports;
  reports.NoteKnownExtent(360, 640);
  assert(!reports.Publish(360, 640));  // Creation-time resize callback.
  FakeHost host;
  host.echo = &reports;
  RootGeometryApplier applier;
  // Launch orientation: the owner has seen every report (serial 0).
  assert(applier.Apply(Publication(2, 0, 1280, 720), reports.latest_serial(), &host,
                       &reports) == RootGeometryStatus::kApplied);
  assert(host.set_calls == 1 && host.width == 640 && host.height == 360);
  assert(host.android_width == 1280 && host.android_height == 720);
  assert(reports.latest_serial() == 0);  // The AppKit echo was suppressed.
  assert(applier.owned() && applier.revision() == 2);
}

void UserResizeAndStaleRevisions() {
  RootGeometryReports reports;
  reports.NoteKnownExtent(640, 360);
  assert(reports.Publish(800, 450));
  assert(!reports.Publish(800, 450));  // Duplicate of the latest fact.
  assert(reports.Publish(900, 500));
  RootGeometryReport report;
  assert(reports.Await(0, &report) && report.serial == 2 && report.points_width == 900);
  FakeHost host;
  host.width = 900;
  host.height = 500;
  RootGeometryApplier applier;
  // A revision answering report 1 must not fight the user's newer extent.
  assert(applier.Apply(Publication(3, 1, 1600, 900), reports.latest_serial(), &host,
                       &reports) == RootGeometryStatus::kApplied);
  assert(host.set_calls == 0 && host.android_width == 1600);
  assert(applier.Apply(Publication(4, 2, 1800, 1000), reports.latest_serial(), &host,
                       &reports) == RootGeometryStatus::kApplied);
  assert(host.set_calls == 0 && host.android_width == 1800);
  // Older or repeated revisions are stale and never touch the backing.
  const int resizes = host.resize_calls;
  assert(applier.Apply(Publication(3, 2, 1600, 900), reports.latest_serial(), &host,
                       &reports) == RootGeometryStatus::kStale);
  assert(applier.Apply(Publication(4, 2, 1800, 1000), reports.latest_serial(), &host,
                       &reports) == RootGeometryStatus::kStale);
  assert(host.resize_calls == resizes);
  // The user returning to the Android-set extent is a fact again.
  reports.NoteKnownExtent(900, 500);
  assert(reports.Publish(700, 700));
  assert(reports.Publish(900, 500));
}

void BackingScaleIsAHostFact() {
  RootGeometryReports reports;
  reports.NoteKnownExtent(640, 360, 2);
  assert(reports.backing_scale() == 2);
  assert(!reports.Publish(640, 360, 2));  // Creation extent and scale.
  assert(!reports.Publish(640, 360));     // Unknown scale keeps the known one.
  // Moving to a 1x display at the same point extent is a new fact.
  assert(reports.Publish(640, 360, 1));
  RootGeometryReport report;
  assert(reports.Await(0, &report) && report.backing_scale == 1 &&
         report.points_width == 640);
  assert(reports.backing_scale() == 1);
  assert(!reports.Publish(640, 360, 1));  // Duplicate.
  // A later resize without a scale keeps the display's scale.
  assert(reports.Publish(700, 400));
  assert(reports.Await(1, &report) && report.backing_scale == 1);
  // Back on the 2x display.
  assert(reports.Publish(700, 400, 2));
  assert(reports.backing_scale() == 2);
}

void AllocationFailureRetriesAndCloseIsTerminal() {
  RootGeometryReports reports;
  FakeHost host;
  host.width = 640;
  host.height = 360;
  RootGeometryApplier applier;
  host.next = DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  assert(applier.Apply(Publication(5, 0, 1280, 720), 0, &host, &reports) ==
         RootGeometryStatus::kAllocationFailed);
  assert(!applier.owned() && applier.revision() == 0);
  host.next = DARWIN_ART_SURFACE_OK;
  assert(applier.Apply(Publication(6, 0, 1280, 720), 0, &host, &reports) ==
         RootGeometryStatus::kApplied);
  host.next = DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  assert(applier.Apply(Publication(7, 0, 1400, 720), 0, &host, &reports) ==
         RootGeometryStatus::kAllocationFailed);
  assert(applier.revision() == 6 && host.android_width == 1280);
  host.next = DARWIN_ART_SURFACE_OK;
  host.is_closed = true;
  assert(applier.Apply(Publication(8, 0, 1400, 720), 0, &host, &reports) ==
         RootGeometryStatus::kClosed);
  assert(applier.revision() == 6);
  // A successor root has its own applier: nothing of this root carries over.
  RootGeometryApplier successor;
  FakeHost fresh;
  assert(successor.Apply(Publication(1, 0, 720, 1280), 0, &fresh, &reports) ==
         RootGeometryStatus::kApplied);

  std::thread waiter([&] {
    RootGeometryReport ignored;
    assert(!reports.Await(reports.latest_serial(), &ignored));
  });
  reports.Close();
  waiter.join();
  assert(!reports.Publish(10, 10));
}
}  // namespace

int main() {
  CreationAndEchoesAreNotFacts();
  UserResizeAndStaleRevisions();
  BackingScaleIsAHostFact();
  AllocationFailureRetriesAndCloseIsTerminal();
  std::puts("root geometry provider PASS");
  return 0;
}
