#pragma once

#include "composition_buffer_lease.h"
#include "egl_ahb_image_owner.h"
#include "egl_context_dispatch.h"
#include "../surfaceflinger/metal_composer.h"
#include "../surfaceflinger/commit_receipt.h"

#include <android/hardware_buffer.h>

#include <cstddef>
#include <cstdint>

namespace darwin_art::graphics {

// The facade owns the EGL sync registry. This value is an owned producer-fence
// token for the duration of one consumer turn; the consumer never inspects the
// registry or its mutex.
struct CompositionProducerFence {
  void* token = nullptr;
  void* shared_event = nullptr;
  std::uint64_t signal_value = 0;
};

// Borrowed, already-resolved operations supplied by darwin_angle_egl.cc. No
// EGL loader/cache or native-fence registry state crosses this boundary.
struct CompositionConsumerBackend {
  EglDisplay (*get_current_display)() = nullptr;
  EglContext (*get_current_context)() = nullptr;
  EglSurface (*get_current_surface)(EglInt) = nullptr;
  MakeCurrentProc make_current = nullptr;
  void* (*get_proc_address)(const char*) = nullptr;
  EglInt (*get_error)() = nullptr;
  std::uint8_t (*gl_is_enabled)(std::uint32_t) = nullptr;
  void (*gl_enable)(std::uint32_t) = nullptr;
  void (*gl_get_integer_v)(std::uint32_t, EglInt*) = nullptr;
  void (*gl_bind_framebuffer)(std::uint32_t, std::uint32_t) = nullptr;
  void (*gl_disable)(std::uint32_t) = nullptr;
  void (*gl_get_float_v)(std::uint32_t, float*) = nullptr;
  void (*gl_clear_color)(float, float, float, float) = nullptr;
  void (*gl_clear)(std::uint32_t) = nullptr;

  bool (*lookup_iosurface)(std::uint32_t, void**, std::uint32_t*,
                           std::uint32_t*) = nullptr;
  void (*release_iosurface)(void*) = nullptr;
  void (*set_composition_active)(void*, bool) = nullptr;

  std::int32_t (*host_surface_width)() = nullptr;
  std::int32_t (*host_surface_height)() = nullptr;
  CompositionProducerFence (*export_producer_fence)(EglDisplay) = nullptr;
  void (*release_producer_fence)(EglDisplay, void*) = nullptr;
  int (*present_remote)(std::uint32_t, std::uint32_t, std::uint32_t,
                        std::uint64_t, const DarwinArtMetalComposerLayer*,
                        std::size_t, void*, std::uint64_t) = nullptr;
  DarwinArtSurfaceFlingerReceipt (*present_remote_receipt)(
      std::uint32_t, std::uint32_t, std::uint32_t, std::uint64_t,
      const DarwinArtMetalComposerLayer*, std::size_t, void*, std::uint64_t) = nullptr;
  bool (*compose_local)(void*, void*, std::uint32_t, std::uint32_t,
                        const DarwinArtMetalComposerLayer*, std::size_t,
                        void*, std::uint64_t, void**, std::uint64_t*) = nullptr;
  int (*completion_fence_fd)(void*, std::uint64_t) = nullptr;
  void (*release_shared_event)(void*) = nullptr;
  // Borrowed descriptor: a monitor must duplicate before consuming it. End
  // returns the original descriptor to the transaction/present-fence owner.
  bool (*track_completion_fence)(int) = nullptr;
  void (*close_completion_fence)(int) = nullptr;
  CompositionBufferLeaseOps lease_ops;
  bool debug = false;
};

bool BeginComposition(const CompositionConsumerBackend& backend,
                      void* opaque, bool clear, std::uint64_t transaction_id);
struct CompositionBeginResult {
  bool started = false;
  bool retry_safe = false;
};
CompositionBeginResult BeginCompositionResult(const CompositionConsumerBackend& backend,
    void* opaque, bool clear, std::uint64_t transaction_id);
struct CompositionEndResult {
  int present_fence = -1;
  bool context_restored = false;
  DarwinArtSurfaceFlingerReceipt receipt{DARWIN_ART_SF_COMMIT_UNKNOWN, 0, -1};
};
CompositionEndResult EndCompositionResult(const CompositionConsumerBackend& backend);
int EndComposition(const CompositionConsumerBackend& backend);
// Same-thread abort/session teardown before terminating the borrowed EGL
// backend. TLS destruction only releases the host target, never calls EGL.
bool ResetComposition(const CompositionConsumerBackend& backend,
                      EglDisplay display = nullptr);
void SetCompositionActive(const CompositionConsumerBackend& backend, bool active);

void PresentHardwareBuffer(
    const CompositionConsumerBackend& backend, void* queue,
    std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint64_t what, std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::int32_t z, void* opaque,
    std::uint32_t transform, std::int32_t source_left, std::int32_t source_top,
    std::int32_t source_right, std::int32_t source_bottom,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    bool has_damage, std::int32_t damage_left, std::int32_t damage_top,
    std::int32_t damage_right, std::int32_t damage_bottom, float alpha);

void PresentSurfaceControlState(
    std::uint32_t owner_process_id, std::uint32_t layer_id,
    std::uint32_t parent_owner_process_id, std::uint32_t parent_id,
    std::uint32_t relative_parent_owner_process_id,
    std::uint32_t relative_parent_id, std::uint64_t what,
    std::uint32_t flags, std::uint32_t mask, std::uint32_t transform,
    std::int32_t destination_left, std::int32_t destination_top,
    std::int32_t destination_right, std::int32_t destination_bottom,
    std::int32_t position_x, std::int32_t position_y, float scale_x,
    float scale_y, bool has_crop, std::int32_t crop_left,
    std::int32_t crop_top, std::int32_t crop_right, std::int32_t crop_bottom,
    std::int32_t z, float alpha, const std::int32_t* transparent_region_rects,
    std::uint32_t transparent_region_count);

}  // namespace darwin_art::graphics
