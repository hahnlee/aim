#include "../../compat/process/service_endpoint.h"
#include "../../compat/surfaceflinger/composition_queue.h"
#include "../../compat/surfaceflinger/iosurface_backing.h"

#import <CoreFoundation/CoreFoundation.h>
#import <IOSurface/IOSurface.h>

#include <assert.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <mutex>
#include <span>

namespace {

std::atomic<uint64_t> next_endpoint{1};
std::atomic<int> endpoint_closes{0};

struct Surface {
  IOSurfaceRef value = nullptr;
  uint32_t id = 0;
};

Surface MakeSurface(uint32_t width, uint32_t height) {
  CFMutableDictionaryRef properties = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 4, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  assert(properties != nullptr);
  const int32_t bytes_per_element = 4;
  const int32_t width_value = static_cast<int32_t>(width);
  const int32_t height_value = static_cast<int32_t>(height);
  const int32_t bytes_per_row = width_value * bytes_per_element;
  const int32_t allocation_size =
      height_value * bytes_per_row;
  CFNumberRef width_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &width_value);
  CFNumberRef height_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &height_value);
  CFNumberRef bytes_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_element);
  CFNumberRef allocation_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &allocation_size);
  assert(width_number != nullptr && height_number != nullptr &&
         bytes_number != nullptr && allocation_number != nullptr);
  CFDictionarySetValue(properties, kIOSurfaceWidth, width_number);
  CFDictionarySetValue(properties, kIOSurfaceHeight, height_number);
  CFDictionarySetValue(properties, kIOSurfaceBytesPerElement, bytes_number);
  CFNumberRef row_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_row);
  assert(row_number != nullptr);
  CFDictionarySetValue(properties, kIOSurfaceBytesPerRow, row_number);
  CFDictionarySetValue(properties, kIOSurfaceAllocSize, allocation_number);
  IOSurfaceRef surface = IOSurfaceCreate(properties);
  CFRelease(allocation_number);
  CFRelease(row_number);
  CFRelease(bytes_number);
  CFRelease(height_number);
  CFRelease(width_number);
  CFRelease(properties);
  assert(surface != nullptr);
  return {surface, IOSurfaceGetID(surface)};
}

struct ComposeState {
  std::mutex mutex;
  std::condition_variable changed;
  bool entered = false;
  bool release = false;
  void* observed_surface = nullptr;
  size_t observed_width = 0;
  size_t observed_height = 0;
};

ComposeState* compose_state = nullptr;

void Compose(darwin_art::surfaceflinger::CompositionJob& job) {
  assert(compose_state != nullptr && job.backings != nullptr);
  auto snapshot = job.backings;
  std::unique_lock<std::mutex> lock(compose_state->mutex);
  compose_state->entered = true;
  compose_state->changed.notify_all();
  compose_state->changed.wait(lock, [] { return compose_state->release; });
  lock.unlock();

  // Resolve only the retained job snapshot. Do not call IOSurfaceLookup(id):
  // native resource lifetime must come from ownership, not ID re-lookup.
  auto backing = snapshot->Find(job.header.target_iosurface_id);
  assert(backing != nullptr && backing->native_surface() != nullptr);
  auto* surface = static_cast<IOSurfaceRef>(backing->native_surface());
  lock.lock();
  compose_state->observed_surface = backing->native_surface();
  compose_state->observed_width = IOSurfaceGetWidth(surface);
  compose_state->observed_height = IOSurfaceGetHeight(surface);
  compose_state->changed.notify_all();
}

bool ConsumeFence(int descriptor) noexcept {
  assert(descriptor >= 0);
  char marker = 0;
  assert(read(descriptor, &marker, sizeof(marker)) == 1);
  close(descriptor);
  return true;
}

std::shared_ptr<darwin_art::process::ServiceEndpoint> Endpoint() {
  return std::make_shared<darwin_art::process::ServiceEndpoint>(
      "/iosurface-backing-lifetime-fixture");
}

template <typename Predicate>
bool WaitFor(ComposeState& state, Predicate predicate) {
  std::unique_lock<std::mutex> lock(state.mutex);
  return state.changed.wait_for(lock, std::chrono::seconds(2), predicate);
}

void TestImportValidationAndSnapshotReplacement() {
  Surface first = MakeSurface(17, 9);
  Surface replacement = MakeSurface(31, 7);
  std::array<uint32_t, 1> zero{0};
  auto empty =
      darwin_art::surfaceflinger::ImportedCompositionBackings::Import(zero);
  assert(empty != nullptr && empty->Find(0) == nullptr);
  std::array<uint32_t, 1> missing{UINT32_MAX};
  assert(!darwin_art::surfaceflinger::ImportedCompositionBackings::Import(missing));

  std::array<uint32_t, 2> duplicate{first.id, first.id};
  auto imported =
      darwin_art::surfaceflinger::ImportedCompositionBackings::Import(duplicate);
  assert(imported != nullptr);
  auto duplicate_a = imported->Find(first.id);
  auto duplicate_b = imported->Find(first.id);
  assert(duplicate_a != nullptr && duplicate_a == duplicate_b);

  auto old_snapshot =
      darwin_art::surfaceflinger::ImportedCompositionBackings::Import(
          std::span<const uint32_t>(&first.id, 1));
  auto new_snapshot =
      darwin_art::surfaceflinger::ImportedCompositionBackings::Import(
          std::span<const uint32_t>(&replacement.id, 1));
  std::weak_ptr<const darwin_art::surfaceflinger::IosurfaceBacking> old_weak =
      old_snapshot->Find(first.id);
  auto retained_old_snapshot = old_snapshot;
  old_snapshot = std::move(new_snapshot);
  assert(!old_weak.expired());
  retained_old_snapshot.reset();
  old_snapshot.reset();
  assert(old_weak.expired());
  imported.reset();
  CFRelease(replacement.value);
  CFRelease(first.value);
}

void TestDelayedQueueRetainsNativeBacking() {
  Surface surface = MakeSurface(23, 11);
  auto imported =
      darwin_art::surfaceflinger::ImportedCompositionBackings::Import(
          std::span<const uint32_t>(&surface.id, 1));
  assert(imported != nullptr);
  std::weak_ptr<const darwin_art::surfaceflinger::IosurfaceBacking> weak =
      imported->Find(surface.id);

  ComposeState state;
  compose_state = &state;
  int producer[2] = {-1, -1};
  assert(pipe(producer) == 0);
  {
    darwin_art::surfaceflinger::CompositionQueue queue;
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence));
    darwin_art::surfaceflinger::CompositionJob job;
    job.header.target_iosurface_id = surface.id;
    job.producer_descriptor = producer[0];
    job.backings = imported;
    assert(queue.Enqueue(std::move(job)));
    producer[0] = -1;
    imported.reset();
    // Remove the creator's native reference before the delayed worker runs;
    // the queued import must now be the sole storage lifetime authority.
    CFRelease(surface.value);
    surface.value = nullptr;
    assert(!weak.expired());

    const char marker = 1;
    assert(write(producer[1], &marker, sizeof(marker)) == 1);
    assert(WaitFor(state, [&state] { return state.entered; }));
    assert(!weak.expired());
    {
      std::lock_guard<std::mutex> lock(state.mutex);
      state.release = true;
    }
    state.changed.notify_all();
    assert(WaitFor(state, [&state] {
      return state.observed_surface != nullptr;
    }));
  }
  compose_state = nullptr;
  assert(weak.expired());
  assert(state.observed_width == 23 && state.observed_height == 11);
}

void TestEnqueueFailureAndAbortReleaseBacking() {
  Surface surface = MakeSurface(13, 5);
  auto imported =
      darwin_art::surfaceflinger::ImportedCompositionBackings::Import(
          std::span<const uint32_t>(&surface.id, 1));
  assert(imported != nullptr);
  std::weak_ptr<const darwin_art::surfaceflinger::IosurfaceBacking> weak =
      imported->Find(surface.id);
  int producer[2] = {-1, -1};
  assert(pipe(producer) == 0);
  {
    darwin_art::surfaceflinger::CompositionQueue queue;
    darwin_art::surfaceflinger::CompositionJob job;
    job.producer_descriptor = producer[0];
    job.backings = imported;
    assert(!queue.Enqueue(std::move(job)));
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence));
    job = {};
    job.producer_descriptor = producer[0];
    job.backings = imported;
    assert(queue.Enqueue(std::move(job)));
    producer[0] = -1;
    imported.reset();
    queue.Abort();
    assert(!queue.Enqueue({}));
  }
  close(producer[1]);
  producer[1] = -1;
  assert(weak.expired());
  CFRelease(surface.value);
}

void TestTryEnqueueFullPreservesCallerDescriptors() {
  int producer[2]{-1, -1};
  assert(pipe(producer) == 0);
  {
    darwin_art::surfaceflinger::CompositionQueue queue;
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence));
    // Every fence stays unreadable, so no job can leave the bounded queue.
    for (size_t i = 0; i < queue.kCapacity; ++i) {
      darwin_art::surfaceflinger::CompositionJob job;
      job.producer_descriptor = dup(producer[0]);
      assert(job.producer_descriptor >= 0);
      assert(queue.TryEnqueue(std::move(job)));
    }
    darwin_art::surfaceflinger::CompositionJob rejected;
    rejected.producer_descriptor = dup(producer[0]);
    const int caller_descriptor = rejected.producer_descriptor;
    assert(caller_descriptor >= 0);
    const auto start = std::chrono::steady_clock::now();
    assert(!queue.TryEnqueue(std::move(rejected)));
    assert(std::chrono::steady_clock::now() - start < std::chrono::seconds(1));
    assert(fcntl(caller_descriptor, F_GETFD) >= 0);
    close(caller_descriptor);
    queue.Abort();
    assert(!queue.TryEnqueue({}));
  }
  close(producer[0]);
  close(producer[1]);
}

}  // namespace

extern "C" int32_t darwin_art_service_endpoint_open(
    const uint8_t*, size_t, uint64_t* handle, int32_t* descriptor) {
  assert(handle != nullptr && descriptor != nullptr);
  *handle = next_endpoint.fetch_add(1, std::memory_order_relaxed);
  *descriptor = -1;
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_close(uint64_t handle) {
  assert(handle != 0);
  endpoint_closes.fetch_add(1, std::memory_order_relaxed);
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_ready(uint64_t handle,
                                                       uint32_t bits) {
  assert(handle != 0 && bits != 0);
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_lost(uint64_t handle,
                                                      uint32_t bits) {
  assert(handle != 0 && bits != 0);
  return 0;
}

int main() {
  TestImportValidationAndSnapshotReplacement();
  TestDelayedQueueRetainsNativeBacking();
  TestEnqueueFailureAndAbortReleaseBacking();
  TestTryEnqueueFullPreservesCallerDescriptors();
  std::printf("iosurface backing lifetime: real import, delayed queue retention, "
              "snapshot replacement, failure/abort cleanup, nonblocking queue-full "
              "rejection PASS (endpoint closes=%d)\n",
              endpoint_closes.load(std::memory_order_relaxed));
}
