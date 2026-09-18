#pragma once

#include <cstddef>
#include <cstdint>

// SurfaceControl's transparent-region hint is intentionally bounded at the
// compositor boundary.  Android's Region may contain an arbitrary number of
// rectangles; the native shim forwards the first few rectangles and treats a
// truncated hint as conservative (no additional hole beyond those entries).
inline constexpr size_t kDarwinArtMaxTransparentRegionRects = 8;

struct DarwinArtTransparentRegionRect {
  int32_t left = 0;
  int32_t top = 0;
  int32_t right = 0;
  int32_t bottom = 0;
};

struct DarwinArtMetalComposerLayer {
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  uint32_t parent_owner_process_id = 0;
  uint32_t parent_id = 0;
  uint32_t relative_parent_owner_process_id = 0;
  uint32_t relative_parent_id = 0;
  uint64_t what = 0;
  uint32_t flags = 0;
  uint32_t mask = 0;
  uint32_t transform = 0;
  // True when the native producer stored row zero at the bottom of its
  // IOSurface. This is buffer metadata, independent of layer/window names.
  bool producer_bottom_left = false;
  void* iosurface = nullptr;
  uint32_t width = 0;
  uint32_t height = 0;
  int32_t source_left = 0;
  int32_t source_top = 0;
  int32_t source_right = 0;
  int32_t source_bottom = 0;
  int32_t destination_left = 0;
  int32_t destination_top = 0;
  int32_t destination_right = 0;
  int32_t destination_bottom = 0;
  // SurfaceControl geometry is retained independently from a buffer's source
  // and destination rectangles. Structural parents commonly have no buffer,
  // but their transform and crop still constrain every descendant.
  int32_t position_x = 0;
  int32_t position_y = 0;
  float scale_x = 1.0f;
  float scale_y = 1.0f;
  bool has_crop = false;
  int32_t crop_left = 0;
  int32_t crop_top = 0;
  int32_t crop_right = 0;
  int32_t crop_bottom = 0;
  // Resolved display-space ancestor clip. This is produced by the
  // SurfaceFlinger hierarchy resolver, never by an application producer.
  bool has_clip = false;
  int32_t clip_left = 0;
  int32_t clip_top = 0;
  int32_t clip_right = 0;
  int32_t clip_bottom = 0;
  int32_t z = 0;
  float alpha = 1.0f;
  uint32_t transparent_region_count = 0;
  DarwinArtTransparentRegionRect transparent_region[
      kDarwinArtMaxTransparentRegionRects]{};
};

// Darwin's HWC/Composer backend. Every layer and the display target remain
// IOSurface-backed Metal textures. The command buffer waits on the producer
// event in the GPU timeline and signals a retained completion event after the
// atomic display composition. The caller retires completion_event through
// darwin_art_android_metal_shared_event_release, never CFRelease/objc release,
// and must keep that provider ownership until asynchronous consumers finish.
extern "C" bool darwin_art_metal_composer_compose(
    void* metal_device, void* target_iosurface, uint32_t target_width,
    uint32_t target_height, const DarwinArtMetalComposerLayer* layers,
    size_t layer_count, void* producer_event, uint64_t producer_value,
    void** completion_event, uint64_t* completion_value);
