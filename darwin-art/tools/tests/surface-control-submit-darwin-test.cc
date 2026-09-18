#include "compat/darwin_angle_egl.h"
#include "compat/darwin_surface_bridge.h"
#include "compat/graphics/hardware_buffer_owner.h"
#include "compat/surfaceflinger/service_darwin.h"
#include "compat/window/surface_control_submit_darwin.h"

#import <CoreFoundation/CoreFoundation.h>
#import <IOSurface/IOSurface.h>
#include <android/hardware_buffer.h>

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <vector>

namespace {

AHardwareBuffer* const kBuffer =
    reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(0x101));
DarwinArtSurface* const kHost =
    reinterpret_cast<DarwinArtSurface*>(static_cast<uintptr_t>(0x202));

IOSurfaceRef g_buffer_surface = nullptr;
bool g_begin_result = true;
bool g_begin_checked_result = true;
int g_end_result = -1;
bool g_end_checked_result = true;
bool g_throw_active = false;
bool g_throw_begin = false;
int g_service_commit_result = 31;
int g_service_present_result = 55;
int g_service_submit_result = 56;
int g_begin_calls = 0;
int g_end_calls = 0;
int g_state_calls = 0;
int g_buffer_calls = 0;
int g_active_calls = 0;
int g_commit_calls = 0;
int g_present_calls = 0;
int g_submit_calls = 0;
int g_monitor_dup_calls = 0;
int g_monitor_track_calls = 0;
int g_active_target_calls = 0;
int g_last_monitor_input = -1;
int g_last_monitor_duplicate = -1;
bool g_last_active = false;
std::uint64_t g_last_transaction = 0;
std::uint64_t g_last_service_transaction = 0;
std::size_t g_last_layer_count = 0;
std::uint32_t g_last_target_id = 0;
std::uint32_t g_last_target_width = 0;
std::uint32_t g_last_target_height = 0;
int g_ahb_acquire_calls = 0;
int g_ahb_release_calls = 0;

void ResetCalls() {
  g_throw_active = false;
  g_throw_begin = false;
  g_begin_calls = 0;
  g_end_calls = 0;
  g_state_calls = 0;
  g_buffer_calls = 0;
  g_active_calls = 0;
  g_commit_calls = 0;
  g_present_calls = 0;
  g_submit_calls = 0;
  g_monitor_dup_calls = 0;
  g_monitor_track_calls = 0;
  g_active_target_calls = 0;
  g_last_monitor_input = -1;
  g_last_monitor_duplicate = -1;
  g_last_active = false;
  g_last_transaction = 0;
  g_last_service_transaction = 0;
  g_last_layer_count = 0;
  g_last_target_id = 0;
  g_last_target_width = 0;
  g_last_target_height = 0;
  g_ahb_acquire_calls = 0;
  g_ahb_release_calls = 0;
}

IOSurfaceRef MakeSurface(std::uint32_t width, std::uint32_t height) {
  const void* keys[] = {kIOSurfaceWidth, kIOSurfaceHeight,
                        kIOSurfaceBytesPerElement};
  const int widths[] = {static_cast<int>(width), static_cast<int>(height), 4};
  CFNumberRef values[3]{};
  for (int index = 0; index < 3; ++index) {
    values[index] = CFNumberCreate(kCFAllocatorDefault, kCFNumberIntType,
                                   &widths[index]);
  }
  CFDictionaryRef properties = CFDictionaryCreate(
      kCFAllocatorDefault, keys, reinterpret_cast<const void**>(values), 3,
      &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
  IOSurfaceRef surface = IOSurfaceCreate(properties);
  CFRelease(properties);
  for (CFNumberRef value : values) CFRelease(value);
  assert(surface != nullptr);
  assert(IOSurfaceGetWidth(surface) == width && IOSurfaceGetHeight(surface) == height);
  return surface;
}

darwin_art::window::SurfaceControlSnapshot MakeSnapshot(bool state,
                                                         bool presentation) {
  darwin_art::window::SurfaceControlSnapshot snapshot;
  if (state) {
    snapshot.control_states.push_back({
        .owner_process_id = 7,
        .layer_id = 8,
        .parent_owner_process_id = 9,
        .parent_id = 10,
        .what = DARWIN_ART_SF_POSITION_CHANGED,
        .destination_left = 1,
        .destination_top = 2,
        .destination_right = 100,
        .destination_bottom = 80,
        .z = 3,
        .alpha = 1.0f,
    });
  }
  if (presentation) {
    AHardwareBuffer_acquire(kBuffer);
    snapshot.presentations.push_back({
        .opaque = reinterpret_cast<ASurfaceControl*>(0x303),
        .buffer = kBuffer,
        .owner_process_id = 11,
        .layer_id = 12,
        .parent_owner_process_id = 13,
        .parent_id = 14,
        .source = {0, 0, 64, 32},
        .destination = {4, 5, 68, 37},
        .alpha = 0.75f,
        .z_order = 6,
        .name = "fixture-buffer",
        .transform = 0,
    });
  }
  return snapshot;
}

}  // namespace

extern "C" {

void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_ahb_acquire_calls;
}

void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  ++g_ahb_release_calls;
}

void AHardwareBuffer_describe(const AHardwareBuffer* buffer,
                              AHardwareBuffer_Desc* description) {
  assert(buffer == kBuffer && description != nullptr);
  *description = {};
  description->width = 64;
  description->height = 32;
  description->layers = 1;
  description->format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
}

void* darwin_art_android_hardware_buffer_iosurface(AHardwareBuffer* buffer) {
  assert(buffer == kBuffer);
  return g_buffer_surface;
}

bool darwin_art_android_begin_hardware_buffer_composition(
    void* buffer, bool clear, std::uint64_t transaction_id) {
  assert(buffer == kBuffer && clear);
  ++g_begin_calls;
  g_last_transaction = transaction_id;
  return g_begin_result;
}

int darwin_art_android_end_hardware_buffer_composition() {
  ++g_end_calls;
  return g_end_result;
}
bool darwin_art_android_begin_hardware_buffer_composition_checked(
    void* buffer, bool clear, uint64_t transaction_id, bool* started) {
  assert(started != nullptr);
  if (g_throw_begin) throw std::bad_alloc();
  *started = darwin_art_android_begin_hardware_buffer_composition(
      buffer, clear, transaction_id);
  return g_begin_checked_result;
}

bool darwin_art_android_end_hardware_buffer_composition_checked(
    int* present_fence) {
  assert(present_fence != nullptr);
  ++g_end_calls;
  *present_fence = g_end_result;
  return g_end_checked_result;
}
bool darwin_art_android_end_hardware_buffer_composition_receipt(
    DarwinArtSurfaceFlingerReceipt* receipt) {
  assert(receipt != nullptr);
  ++g_end_calls;
  if (g_end_result >= 0)
    *receipt = {DARWIN_ART_SF_COMMIT_COMMITTED, 0, g_end_result};
  else
    *receipt = {DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
  return g_end_checked_result;
}

void darwin_art_android_set_hardware_buffer_composition_active(bool active) {
  if (g_throw_active) throw std::bad_alloc();
  ++g_active_calls;
  g_last_active = active;
}

void darwin_art_android_present_surface_control_state(
    std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::uint64_t what, std::uint32_t flags,
    std::uint32_t mask, std::uint32_t transform, std::int32_t destination_left,
    std::int32_t destination_top, std::int32_t destination_right,
    std::int32_t destination_bottom, std::int32_t position_x,
    std::int32_t position_y, float scale_x, float scale_y, bool has_crop,
    std::int32_t crop_left, std::int32_t crop_top, std::int32_t crop_right,
    std::int32_t crop_bottom, std::int32_t z, float alpha,
    const std::int32_t* transparent_region_rects,
    std::uint32_t transparent_region_count) {
  assert(owner_process_id != 0 && layer_id != 0);
  (void)parent_owner_process_id;
  (void)parent_id;
  (void)relative_parent_owner_process_id;
  (void)relative_parent_id;
  (void)what;
  (void)flags;
  (void)mask;
  (void)transform;
  (void)destination_left;
  (void)destination_top;
  (void)destination_right;
  (void)destination_bottom;
  (void)position_x;
  (void)position_y;
  (void)scale_x;
  (void)scale_y;
  (void)has_crop;
  (void)crop_left;
  (void)crop_top;
  (void)crop_right;
  (void)crop_bottom;
  (void)z;
  (void)alpha;
  assert((transparent_region_rects == nullptr) ==
         (transparent_region_count == 0));
  ++g_state_calls;
}

void darwin_art_android_present_hardware_buffer(
    void* queue, std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint64_t what, std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::int32_t z, void* buffer,
    std::uint32_t transform, std::int32_t source_left,
    std::int32_t source_top, std::int32_t source_right,
    std::int32_t source_bottom, std::int32_t destination_left,
    std::int32_t destination_top, std::int32_t destination_right,
    std::int32_t destination_bottom, bool has_damage,
    std::int32_t damage_left, std::int32_t damage_top,
    std::int32_t damage_right, std::int32_t damage_bottom, float alpha) {
  assert(queue == reinterpret_cast<void*>(0x303) && buffer == kBuffer &&
         owner_process_id != 0 && layer_id != 0);
  (void)parent_owner_process_id;
  (void)parent_id;
  (void)what;
  (void)relative_parent_owner_process_id;
  (void)relative_parent_id;
  (void)z;
  (void)transform;
  (void)source_left;
  (void)source_top;
  (void)source_right;
  (void)source_bottom;
  (void)destination_left;
  (void)destination_top;
  (void)destination_right;
  (void)destination_bottom;
  (void)has_damage;
  (void)damage_left;
  (void)damage_top;
  (void)damage_right;
  (void)damage_bottom;
  (void)alpha;
  ++g_buffer_calls;
}

int darwin_art_surfaceflinger_service_commit(
    std::uint64_t transaction_id,
    const DarwinArtMetalComposerLayer* layers, std::size_t layer_count) {
  assert(layers != nullptr && layer_count > 0);
  ++g_commit_calls;
  g_last_service_transaction = transaction_id;
  g_last_layer_count = layer_count;
  return g_service_commit_result;
}

int darwin_art_surfaceflinger_service_present(
    std::uint32_t target_iosurface_id, std::uint32_t target_width,
    std::uint32_t target_height, std::uint64_t transaction_id,
    const DarwinArtMetalComposerLayer* layers, std::size_t layer_count,
    void* producer_event, std::uint64_t producer_value) {
  assert(target_iosurface_id != 0 && target_width > 0 && target_height > 0);
  assert(layers != nullptr && layer_count > 0);
  assert(producer_event == nullptr && producer_value == 0);
  ++g_present_calls;
  g_last_target_id = target_iosurface_id;
  g_last_target_width = target_width;
  g_last_target_height = target_height;
  g_last_service_transaction = transaction_id;
  g_last_layer_count = layer_count;
  return g_service_present_result;
}

int darwin_art_surfaceflinger_service_submit(
    std::uint64_t transaction_id, const DarwinArtMetalComposerLayer* layers,
    std::size_t layer_count) {
  assert(layers != nullptr && layer_count > 0);
  ++g_submit_calls;
  g_last_service_transaction = transaction_id;
  g_last_layer_count = layer_count;
  return g_service_submit_result;
}
DarwinArtSurfaceFlingerReceipt darwin_art_surfaceflinger_service_commit_receipt(
    uint64_t id, const DarwinArtMetalComposerLayer* layers, size_t count) {
  const int fd = darwin_art_surfaceflinger_service_commit(id, layers, count);
  if (fd >= 0) return {DARWIN_ART_SF_COMMIT_COMMITTED, 0, fd};
  return {DARWIN_ART_SF_COMMIT_REJECTED, EBUSY, -1};
}
DarwinArtSurfaceFlingerReceipt darwin_art_surfaceflinger_service_submit_receipt(
    uint64_t id, const DarwinArtMetalComposerLayer* layers, size_t count) {
  const int fd = darwin_art_surfaceflinger_service_submit(id, layers, count);
  if (fd >= 0) return {DARWIN_ART_SF_COMMIT_COMMITTED, 0, fd};
  return {DARWIN_ART_SF_COMMIT_REJECTED, EBUSY, -1};
}
DarwinArtSurfaceFlingerReceipt darwin_art_surfaceflinger_service_present_receipt(
    uint32_t target, uint32_t width, uint32_t height, uint64_t id,
    const DarwinArtMetalComposerLayer* layers, size_t count, void* event, uint64_t value) {
  const int fd = darwin_art_surfaceflinger_service_present(
      target, width, height, id, layers, count, event, value);
  if (fd >= 0) return {DARWIN_ART_SF_COMMIT_COMMITTED, 0, fd};
  return {DARWIN_ART_SF_COMMIT_REJECTED, EBUSY, -1};
}

int darwin_art_bionic_socket_broker_dup(int fd) {
  assert(fd >= 0);
  ++g_monitor_dup_calls;
  g_last_monitor_input = fd;
  return fd + 100;
}

DarwinArtSurface* darwin_art_surface_active_gpu() { return kHost; }

bool darwin_art_surface_gpu_track_composition_fence(
    DarwinArtSurface* surface, int fence_fd) {
  assert(surface == kHost && fence_fd >= 0);
  ++g_monitor_track_calls;
  g_last_monitor_duplicate = fence_fd;
  return true;
}

void darwin_art_surface_gpu_set_iosurface_composition_active(void* iosurface,
                                                              bool active) {
  assert(iosurface != nullptr && active);
  ++g_active_target_calls;
}

}  // extern "C"

namespace darwin_art {

jint DarwinAngleHostSurfaceWidth() { return 320; }
jint DarwinAngleHostSurfaceHeight() { return 240; }

}  // namespace darwin_art

int main() {
  g_buffer_surface = MakeSurface(64, 32);
  const std::uint32_t target_id = IOSurfaceGetID(g_buffer_surface);
  char target_text[32];
  std::snprintf(target_text, sizeof(target_text), "%u", target_id);
  ResetCalls();

  // An empty snapshot is an honest no-op and never enters Android composition.
  {
    darwin_art::window::SurfaceControlSnapshot snapshot;
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 1, false);
    assert(result.success && result.present_fence == -1 && result.no_work);
    assert(result.receipt.disposition == DARWIN_ART_SF_COMMIT_UNKNOWN &&
           result.receipt.error == 0);
    assert(g_begin_calls == 0 && g_end_calls == 0 && g_active_calls == 1 &&
           !g_last_active);
  }

  // Structural-only state uses the central commit API without fabricating a
  // target or entering the buffer composition path.
  ResetCalls();
  {
    auto snapshot = MakeSnapshot(true, false);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 2, true);
    assert(result.success && result.present_fence == g_service_commit_result);
    assert(g_commit_calls == 1 && g_present_calls == 0 && g_submit_calls == 0);
    assert(g_begin_calls == 0 && g_active_calls == 1 && !g_last_active);
    assert(g_last_service_transaction == 2 && g_last_layer_count == 1);
  }

  // A successful Android begin/end publishes directly and does not invoke
  // central fallback.
  ResetCalls();
  g_begin_result = true;
  g_end_result = 47;
  {
    auto snapshot = MakeSnapshot(true, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 3, true);
    assert(result.success && result.present_fence == 47);
    assert(g_begin_calls == 1 && g_end_calls == 1 && g_state_calls == 1 &&
           g_buffer_calls == 1 && g_commit_calls == 0 && g_present_calls == 0);
    assert(g_active_calls == 1 && g_last_active);
    assert(g_last_transaction == 3);
  }
  assert(g_ahb_release_calls == 1);

  // A safe failed begin chooses the central target payload, preserving logical
  // dimensions and tracking a duplicate monitor fence separately from the
  // original transaction fence.
  ResetCalls();
  assert(setenv("DARWIN_ART_HOST_IOSURFACE_ID", target_text, 1) == 0);
  g_begin_result = false;
  g_end_result = -1;
  g_service_present_result = 55;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 4, true);
    assert(result.success && result.present_fence == 55);
    assert(g_begin_calls == 1 && g_end_calls == 0 && g_buffer_calls == 0);
    assert(g_present_calls == 1 && g_submit_calls == 0 &&
           g_last_service_transaction == 4 && g_last_layer_count == 1);
    assert(g_last_target_id == target_id && g_last_target_width == 320 &&
           g_last_target_height == 240);
    assert(g_monitor_dup_calls == 1 && g_last_monitor_input == 55 &&
           g_last_monitor_duplicate == 155 && g_monitor_track_calls == 1);
    assert(g_active_target_calls == 1 && g_active_calls == 0);
  }

  // Without a target, central submission remains targetless and uses the
  // target-independent submit API; it does not call IOSurfaceLookup or the
  // monitor duplication path.
  ResetCalls();
  ResetCalls();
  g_begin_checked_result = false;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 9, true);
    assert(!result.success && result.present_fence == -1);
    assert(g_begin_calls == 1 && g_end_calls == 0 && g_present_calls == 0 &&
           g_submit_calls == 0 && g_commit_calls == 0);
  }
  g_begin_checked_result = true;
  unsetenv("DARWIN_ART_HOST_IOSURFACE_ID");
  g_begin_result = false;
  g_end_result = -1;
  g_service_submit_result = 56;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 5, true);
    assert(result.success && result.present_fence == 56);
    assert(g_present_calls == 0 && g_submit_calls == 1 &&
           g_last_service_transaction == 5 && g_last_layer_count == 1);
    assert(g_monitor_dup_calls == 0 && g_monitor_track_calls == 0 &&
           g_active_target_calls == 0 && g_active_calls == 1 && g_last_active);
  }

  // A terminal context-restoration error is reported directly and does not
  // retry through central submission or claim composition was active.
  ResetCalls();
  assert(setenv("DARWIN_ART_HOST_IOSURFACE_ID", target_text, 1) == 0);
  g_begin_result = true;
  g_end_result = 47;
  g_end_checked_result = false;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 6, true);
    assert(!result.success && result.present_fence == 47 && !result.context_restored);
    assert(result.receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED);
    assert(g_begin_calls == 1 && g_end_calls == 1 && g_present_calls == 0 &&
           g_submit_calls == 0 && g_active_calls == 1 &&
           g_monitor_dup_calls == 0);
  }

  // A failed begin on a non-central local presentation is also a hard
  // failure; no fake provider success is reported.
  ResetCalls();
  g_begin_result = false;
  g_end_checked_result = true;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 7, false);
    assert(!result.success && result.present_fence == -1);
    assert(g_begin_calls == 1 && g_end_calls == 0 && g_present_calls == 0 &&
           g_submit_calls == 0 && g_active_calls == 1 && !g_last_active &&
           g_monitor_dup_calls == 0);
  }

  // Central rejection is reported as a failed submission and does not invoke
  // a second Android/provider callback or claim composition was active.
  ResetCalls();
  assert(setenv("DARWIN_ART_HOST_IOSURFACE_ID", target_text, 1) == 0);
  g_begin_result = true;
  g_end_result = -1;
  g_end_checked_result = true;
  g_service_present_result = -1;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(
        snapshot, 8, true);
    assert(!result.success && result.present_fence == -1);
    assert(result.receipt.disposition == DARWIN_ART_SF_COMMIT_UNKNOWN);
    assert(g_begin_calls == 1 && g_end_calls == 1 && g_present_calls == 0 &&
           g_submit_calls == 0 && g_active_calls == 1 &&
           g_monitor_dup_calls == 0);
  }

  // A provider exception after a committed End cannot become pre-send reject
  // or cause retry, and its completion descriptor remains owned by the result.
  ResetCalls();
  g_begin_result = true;
  g_end_result = 47;
  g_end_checked_result = true;
  g_throw_active = true;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(snapshot, 10, true);
    assert(!result.success && result.present_fence == 47 && !result.context_restored);
    assert(result.receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED);
    assert(g_end_calls == 1 && g_present_calls == 0 && g_submit_calls == 0);
  }
  ResetCalls();
  g_throw_begin = true;
  {
    auto snapshot = MakeSnapshot(false, true);
    const auto result = darwin_art::window::SubmitSurfaceControlDarwin(snapshot, 11, true);
    assert(!result.success && !result.context_restored);
    assert(g_end_calls == 0 && g_present_calls == 0 && g_submit_calls == 0);
  }
  unsetenv("DARWIN_ART_HOST_IOSURFACE_ID");
  CFRelease(g_buffer_surface);
  g_buffer_surface = nullptr;
  std::puts("surface control Darwin submit actual TU: no-op/structural/direct/fallback/targetless/failure PASS");
}
