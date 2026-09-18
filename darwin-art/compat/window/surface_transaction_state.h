#pragma once

#include <android/surface_control.h>

#include <cstdint>
#include <vector>

struct ASurfaceTransactionStats;

namespace darwin_art::window {

using SurfaceTransactionCallback =
    void (*)(void*, ASurfaceTransactionStats*);

// Android window transaction state. This type contains only guest-visible
// payload and ownership tokens; cleanup of buffers, controls, fences and
// callbacks remains a platform operation performed after structural changes.
struct SurfaceTransaction {
  struct Update {
    ASurfaceControl* opaque = nullptr;
    AHardwareBuffer* buffer = nullptr;
    int acquire_fence = -1;
    uint64_t submission_cookie = 0;
    bool has_buffer = false;
    ARect source{};
    ARect destination{};
    ARect crop{};
    bool has_geometry = false;
    bool has_crop = false;
    bool has_visibility = false;
    bool visible = true;
    bool has_position = false;
    int32_t position_x = 0;
    int32_t position_y = 0;
    bool has_transform = false;
    int32_t transform = 0;
    bool has_z_order = false;
    int32_t z_order = 0;
    bool has_scale = false;
    float scale_x = 1.0f;
    float scale_y = 1.0f;
    bool has_alpha = false;
    float alpha = 1.0f;
    bool has_parent = false;
    ASurfaceControl* parent = nullptr;
    bool has_relative_layer = false;
    ASurfaceControl* relative_to = nullptr;
    bool has_damage = false;
    std::vector<ARect> damage;
    bool has_transparent_region = false;
    std::vector<ARect> transparent_region;
  };

  using TransactionCallback = SurfaceTransactionCallback;

  struct Callback {
    TransactionCallback function = nullptr;
    void* context = nullptr;
  };

  struct DiscardCallback {
    void (*function)(void*) = nullptr;
    void* context = nullptr;
  };

  struct BufferCallback {
    ASurfaceControl* control = nullptr;
    void* context = nullptr;
    TransactionCallback complete = nullptr;
    void (*discard)(void*, int) = nullptr;
  };

  std::vector<ASurfaceControl*> controls;
  std::vector<Update> updates;
  std::vector<Callback> commits;
  std::vector<Callback> completes;
  std::vector<DiscardCallback> discards;
  std::vector<BufferCallback> buffer_callbacks;
};

}  // namespace darwin_art::window
