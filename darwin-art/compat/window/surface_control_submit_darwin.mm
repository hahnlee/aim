#include "surface_control_submit_darwin.h"
#include "../darwin_angle_egl.h"
#include "../darwin_surface_bridge.h"
#include "../graphics/hardware_buffer_owner.h"
#include "../surfaceflinger/service_darwin.h"
#import <IOSurface/IOSurface.h>
#include <algorithm>
#include <array>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <type_traits>
#include <unistd.h>
extern "C" int darwin_art_bionic_socket_broker_dup(int fd);
namespace darwin_art::window {
namespace {
struct ReleaseIOSurface {
  void operator()(IOSurfaceRef surface) const {
    if (surface != nullptr) CFRelease(surface);
  }
};
bool DebugSurfaceTransactions() {
  return std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr;
}
}
SurfaceControlSubmitResult SubmitSurfaceControlDarwin(
    const SurfaceControlSnapshot& snapshot, uint64_t transaction_id,
    bool central_surfaceflinger) {
  DarwinArtSurfaceFlingerReceipt receipt{DARWIN_ART_SF_COMMIT_REJECTED, EIO, -1};
  bool boundary_healthy = true;
  try {
  if (snapshot.presentations.empty() && snapshot.control_states.empty() &&
      snapshot.frontend_updates.empty()) {
    darwin_art_android_set_hardware_buffer_composition_active(false);
    return {true, -1, {DARWIN_ART_SF_COMMIT_UNKNOWN, 0, -1}, true, true};
  }
  int present_fence = -1;
  bool composition_started = false;
  if (!snapshot.presentations.empty()) {
    boundary_healthy = false;
    if (!darwin_art_android_begin_hardware_buffer_composition_checked(
          snapshot.presentations.front().buffer, true, transaction_id,
          &composition_started)) return {false, -1, receipt, false};
    boundary_healthy = !composition_started;
  }
  bool actual_target_composition_marked = false;
  if (composition_started) {
    boundary_healthy = false;  // Switched scope must reach a successful End.
    for (const auto& state : snapshot.control_states) {
      std::array<int32_t, kDarwinArtMaxTransparentRegionRects * 4>
          transparent_region_rects{};
      for (uint32_t index = 0; index < state.transparent_region_count;
           ++index) {
        const auto& rect = state.transparent_region[index];
        transparent_region_rects[index * 4 + 0] = rect.left;
        transparent_region_rects[index * 4 + 1] = rect.top;
        transparent_region_rects[index * 4 + 2] = rect.right;
        transparent_region_rects[index * 4 + 3] = rect.bottom;
      }
      darwin_art_android_present_surface_control_state(
          state.owner_process_id, state.layer_id,
          state.parent_owner_process_id, state.parent_id,
          state.relative_parent_owner_process_id, state.relative_parent_id,
          state.what, state.flags, state.mask, state.transform,
          state.destination_left, state.destination_top, state.destination_right,
          state.destination_bottom, state.position_x, state.position_y,
          state.scale_x, state.scale_y, state.has_crop, state.crop_left,
          state.crop_top, state.crop_right, state.crop_bottom, state.z,
          state.alpha,
          state.transparent_region_count == 0
              ? nullptr
              : transparent_region_rects.data(),
          state.transparent_region_count);
    }
    for (const auto& presentation : snapshot.presentations) {
      if (DebugSurfaceTransactions()) {
        std::fprintf(stderr,
                     "ART Android SurfaceTransaction: present pid=%d name=%s "
                     "source=[%d,%d,%d,%d] destination=[%d,%d,%d,%d] "
                     "alpha=%.3f z=%d transform=%d\n",
                     getpid(), presentation.name.c_str(),
                     presentation.source.left, presentation.source.top,
                     presentation.source.right, presentation.source.bottom,
                     presentation.destination.left, presentation.destination.top,
                     presentation.destination.right,
                     presentation.destination.bottom, presentation.alpha,
                     presentation.z_order, presentation.transform);
      }
      darwin_art_android_present_hardware_buffer(
          presentation.opaque, presentation.owner_process_id,
          presentation.layer_id, presentation.parent_owner_process_id,
          presentation.parent_id,
          DARWIN_ART_SF_BUFFER_CHANGED |
              (presentation.reparented ? DARWIN_ART_SF_REPARENT : 0),
          presentation.relative_parent_owner_process_id,
          presentation.relative_parent_id,
          presentation.z_order, presentation.buffer,
          static_cast<uint32_t>(presentation.transform),
          presentation.source.left,
          presentation.source.top, presentation.source.right,
          presentation.source.bottom, presentation.destination.left,
          presentation.destination.top, presentation.destination.right,
          presentation.destination.bottom, presentation.has_damage,
          presentation.damage.left, presentation.damage.top,
          presentation.damage.right, presentation.damage.bottom,
          presentation.alpha);
    }
    receipt = {DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
    boundary_healthy = false;
    const bool restored = darwin_art_android_end_hardware_buffer_composition_receipt(&receipt);
    const bool accepted = receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED &&
                          receipt.error == 0 && receipt.completion_fd >= 0;
    darwin_art_android_set_hardware_buffer_composition_active(accepted);
    // End may have published even when no completion descriptor arrived.
    // Never fall through into a second central submission.
    return {accepted && restored, receipt.completion_fd, receipt, restored};
  }
  if (!composition_started && central_surfaceflinger &&
      snapshot.presentations.empty() && !snapshot.control_states.empty()) {
    receipt = {DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
    receipt = darwin_art_surfaceflinger_service_commit_receipt(
        transaction_id, snapshot.control_states.data(),
        snapshot.control_states.size());
    darwin_art_android_set_hardware_buffer_composition_active(false);
    return {receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED &&
                receipt.error == 0 && receipt.completion_fd >= 0,
            receipt.completion_fd, receipt, true};
  }
  if (!composition_started && central_surfaceflinger &&
      (!snapshot.control_states.empty() || !snapshot.presentations.empty())) {
    const char* encoded_target = std::getenv("DARWIN_ART_HOST_IOSURFACE_ID");
    char* end = nullptr;
    const unsigned long parsed =
        encoded_target == nullptr ? 0 : std::strtoul(encoded_target, &end, 10);
    IOSurfaceRef target =
        encoded_target != nullptr && end != encoded_target && *end == '\0' &&
                parsed > 0 && parsed <= UINT32_MAX
            ? IOSurfaceLookup(static_cast<uint32_t>(parsed))
            : nullptr;
    std::unique_ptr<std::remove_pointer_t<IOSurfaceRef>, ReleaseIOSurface> target_owner(target);
    const bool has_explicit_target = target != nullptr;
    const uint32_t target_width = has_explicit_target
        ? static_cast<uint32_t>(IOSurfaceGetWidth(target))
        : 0;
    const uint32_t target_height = has_explicit_target
        ? static_cast<uint32_t>(IOSurfaceGetHeight(target))
        : 0;
    {
      const jint configured_width = darwin_art::DarwinAngleHostSurfaceWidth();
      const jint configured_height = darwin_art::DarwinAngleHostSurfaceHeight();
      const uint32_t logical_target_width =
          configured_width > 0 ? static_cast<uint32_t>(configured_width)
                               : target_width;
      const uint32_t logical_target_height =
          configured_height > 0 ? static_cast<uint32_t>(configured_height)
                                : target_height;

      // The normal path appends each retained AHardwareBuffer to the Metal
      // composer after publishing the structural control states.  Keep the
      // same layer payload when the client-side ANGLE context is unavailable:
      // the central service can import the IOSurface directly, and does not
      // need a producer EGL context to latch an already fence-waited buffer.
      // AHardwareBuffer references in `presentations` remain held until the
      // synchronous service call has serialized every IOSurface id below.
      std::vector<DarwinArtMetalComposerLayer> fallback_layers =
          snapshot.control_states;
      fallback_layers.reserve(snapshot.control_states.size() +
                              snapshot.presentations.size());
      for (const auto& presentation : snapshot.presentations) {
        if (presentation.opaque == nullptr || presentation.buffer == nullptr)
          return {false, -1, {DARWIN_ART_SF_COMMIT_REJECTED, EINVAL, -1}, true};
        AHardwareBuffer_Desc description{};
        AHardwareBuffer_describe(presentation.buffer, &description);
        void* iosurface =
            darwin_art_android_hardware_buffer_iosurface(presentation.buffer);
        if (iosurface == nullptr || description.width == 0 ||
            description.height == 0) {
          return {false, -1, {DARWIN_ART_SF_COMMIT_REJECTED, EINVAL, -1}, true};
        }
        const int32_t buffer_width =
            static_cast<int32_t>(description.width);
        const int32_t buffer_height =
            static_cast<int32_t>(description.height);
        const int32_t source_left =
            std::clamp(presentation.source.left, 0, buffer_width);
        const int32_t source_top =
            std::clamp(presentation.source.top, 0, buffer_height);
        const int32_t source_right =
            std::clamp(presentation.source.right, 0, buffer_width);
        const int32_t source_bottom =
            std::clamp(presentation.source.bottom, 0, buffer_height);
        const int32_t destination_left = has_explicit_target
            ? std::clamp(presentation.destination.left, 0,
                         static_cast<int32_t>(logical_target_width))
            : presentation.destination.left;
        const int32_t destination_top = has_explicit_target
            ? std::clamp(presentation.destination.top, 0,
                         static_cast<int32_t>(logical_target_height))
            : presentation.destination.top;
        const int32_t destination_right = has_explicit_target
            ? std::clamp(presentation.destination.right, 0,
                         static_cast<int32_t>(logical_target_width))
            : presentation.destination.right;
        const int32_t destination_bottom = has_explicit_target
            ? std::clamp(presentation.destination.bottom, 0,
                         static_cast<int32_t>(logical_target_height))
            : presentation.destination.bottom;
        fallback_layers.push_back({
            .owner_process_id = presentation.owner_process_id,
            .layer_id = presentation.layer_id,
            .parent_owner_process_id = presentation.parent_owner_process_id,
            .parent_id = presentation.parent_id,
            .relative_parent_owner_process_id =
                presentation.relative_parent_owner_process_id,
            .relative_parent_id = presentation.relative_parent_id,
            .what = static_cast<uint64_t>(
                DARWIN_ART_SF_BUFFER_CHANGED |
                (presentation.reparented ? DARWIN_ART_SF_REPARENT : 0) |
                // Unset destination frame: bounds follow this buffer.
                (presentation.explicit_geometry
                     ? 0 : DARWIN_ART_SF_BUFFER_DEFINES_BOUNDS)),
            .flags = 0,
            .mask = 0,
            .transform = static_cast<uint32_t>(presentation.transform),
            .producer_bottom_left =
                (description.usage & AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY) == 0,
            .iosurface = iosurface,
            .width = description.width,
            .height = description.height,
            .source_left = source_left,
            .source_top = source_top,
            .source_right = source_right,
            .source_bottom = source_bottom,
            .destination_left = destination_left,
            .destination_top = destination_top,
            .destination_right = destination_right,
            .destination_bottom = destination_bottom,
            .z = presentation.z_order,
            .alpha = presentation.alpha,
        });
      }
      receipt = {DARWIN_ART_SF_COMMIT_UNKNOWN, EIO, -1};
      receipt = has_explicit_target
          ? darwin_art_surfaceflinger_service_present_receipt(
                static_cast<uint32_t>(parsed), logical_target_width,
                logical_target_height, transaction_id, fallback_layers.data(),
                fallback_layers.size(), nullptr, 0)
          : darwin_art_surfaceflinger_service_submit_receipt(
                transaction_id, fallback_layers.data(),
                fallback_layers.size());
      present_fence = receipt.completion_fd;
      composition_started = receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED &&
                            receipt.error == 0 && present_fence >= 0;
      if (!composition_started) {
        std::fprintf(stderr,
                     "ART Android SurfaceTransaction: central submission failed transaction=%llu\n",
                     static_cast<unsigned long long>(transaction_id));
        return {false, present_fence, receipt, true};
      }
      // This path does not enter ANGLE, so its thread-local EGL target may be
      // empty or stale. Mark the target actually submitted to the central
      // service and register a private copy of its completion fence with the
      // host scanout monitor. The Android-facing stats fence remains owned by
      // the transaction callbacks; the duplicate is consumed by the monitor.
      if (has_explicit_target) {
        darwin_art_surface_gpu_set_iosurface_composition_active(
            target, composition_started);
        actual_target_composition_marked = true;
      }
      if (has_explicit_target && composition_started) {
        DarwinArtSurface* host = darwin_art_surface_active_gpu();
        const int monitor_fence =
            host == nullptr
                ? -1
                : darwin_art_bionic_socket_broker_dup(present_fence);
        if (host != nullptr && monitor_fence >= 0) {
          if (!darwin_art_surface_gpu_track_composition_fence(
                  host, monitor_fence) &&
              DebugSurfaceTransactions()) {
            std::fprintf(stderr,
                         "ART Android SurfaceTransaction: fallback readiness "
                         "monitor rejected fence=%d\n",
                         monitor_fence);
          }
        }
      }
    }
  }
  // The host and Chromium renderer import the same IOSurface in different
  // processes. Publish SurfaceFlinger's retained-layer visibility with that
  // shared object so parent HWUI never samples a detached child layer.
  if (!actual_target_composition_marked) {
    darwin_art_android_set_hardware_buffer_composition_active(
        composition_started);
  }
  // Structural-only/no-op transactions can succeed without producing a
  // presentation fence. A requested buffer presentation cannot: failed local
  // begin/end has no acknowledged submission to report to Android.
  return {composition_started || snapshot.presentations.empty(), present_fence,
          receipt, true};
  } catch (...) {
    // Once transport may have started, its unknown/committed classification
    // cannot turn into safe rejection because a later provider threw.
    if (receipt.disposition == DARWIN_ART_SF_COMMIT_REJECTED)
      receipt.error = ENOMEM;
    return {false, receipt.completion_fd, receipt, boundary_healthy};
  }
}
SurfaceControlSubmissionEnvironment QuerySurfaceControlSubmissionEnvironment() {
  const char* service = std::getenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
  const char* application = std::getenv("DARWIN_ART_APK_APP_PACKAGE");
  return {service != nullptr && service[0] != '\0',
          application != nullptr && application[0] != '\0'};
}
}
