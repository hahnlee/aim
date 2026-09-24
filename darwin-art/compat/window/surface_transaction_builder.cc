#include "surface_transaction_builder.h"

#include <cstdint>

#include <android/hardware_buffer.h>

#include <algorithm>
#include <iterator>
#include <memory>
#include <utility>

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer);
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer);
extern "C" void ASurfaceControl_acquire(ASurfaceControl* control);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);
extern "C" int darwin_art_bionic_socket_broker_dup(int fd);

namespace darwin_art::window {
namespace {

constexpr size_t kMaxTransparentRegionRects = 8;

int DuplicateDiscardFence(int fence) {
  if (fence < 0) return -1;
  const int duplicate = darwin_art_bionic_socket_broker_dup(fence);
  if (duplicate >= 0) return duplicate;
  // A private discard callback receives -2 when the producer fence could not
  // be duplicated. This preserves the existing quarantine contract without
  // making callback cleanup wait on an unsignaled fence.
  return -2;
}

struct PreparedDiscard final {
  SurfaceTransaction::BufferCallback callback;
  int fence = -1;
};

struct DetachedBufferBatch final {
  AHardwareBuffer* old_buffer = nullptr;
  int old_fence = -1;
  std::vector<PreparedDiscard> callbacks;
  bool armed = false;
  bool disposed = false;

  ~DetachedBufferBatch() { Dispose(); }

  void Arm(AHardwareBuffer* buffer, int fence) {
    old_buffer = buffer;
    old_fence = fence;
    armed = true;
  }

  void Dispose() {
    if (disposed) return;
    disposed = true;
    if (!armed) {
      // Preparation failed before the replacement became visible.  The
      // duplicated descriptors belong to this temporary batch, but the
      // source transaction's buffer, fence, and callbacks remain untouched.
      for (auto& prepared : callbacks) {
        if (prepared.fence >= 0)
          (void)darwin_art_bionic_socket_broker_close(prepared.fence);
        prepared.fence = -1;
      }
      return;
    }
    // Every callback and duplicate fence was prepared before the first
    // callback runs. No transaction pointer is touched from this point on.
    for (auto& prepared : callbacks) {
      if (prepared.callback.discard != nullptr) {
        // The callback owns the duplicated fence it receives.  In the normal
        // path it consumes that descriptor while returning the producer slot;
        // do not close it a second time here.  A missing callback has no
        // recipient, so retain cleanup ownership locally instead.
        prepared.callback.discard(prepared.callback.context, prepared.fence);
        prepared.fence = -1;
      } else if (prepared.fence >= 0) {
        (void)darwin_art_bionic_socket_broker_close(prepared.fence);
        prepared.fence = -1;
      }
    }
    if (old_buffer != nullptr) AHardwareBuffer_release(old_buffer);
    if (old_fence >= 0)
      (void)darwin_art_bionic_socket_broker_close(old_fence);
  }
};

}  // namespace

bool SurfaceTransactionBuilder::EnsureUpdate(
    ASurfaceControl* control, SurfaceTransaction::Update** update) {
  if (transaction_ == nullptr || control == nullptr || update == nullptr)
    return false;
  try {
    auto found = std::find_if(
        transaction_->updates.begin(), transaction_->updates.end(),
        [control](const SurfaceTransaction::Update& candidate) {
          return candidate.opaque == control;
        });
    const bool missing_update = found == transaction_->updates.end();
    const bool missing_control =
        std::find(transaction_->controls.begin(), transaction_->controls.end(),
                  control) == transaction_->controls.end();
    // Reserve both vectors before changing either one. Once reserved, the
    // pointer returned below remains stable until this setter finishes.
    if (missing_update) transaction_->updates.reserve(transaction_->updates.size() + 1);
    if (missing_control)
      transaction_->controls.reserve(transaction_->controls.size() + 1);
    if (missing_update) {
      transaction_->updates.emplace_back();
      found = std::prev(transaction_->updates.end());
      found->opaque = control;
    }
    if (missing_control) {
      ASurfaceControl_acquire(control);
      transaction_->controls.push_back(control);
    }
    *update = &*found;
    return true;
  } catch (...) {
    return false;
  }
}

bool SurfaceTransactionBuilder::EnsureUpdateWithReference(
    ASurfaceControl* control, ASurfaceControl* reference,
    SurfaceTransaction::Update** update) {
  if (transaction_ == nullptr || control == nullptr || update == nullptr)
    return false;
  if (reference == nullptr) return EnsureUpdate(control, update);
  try {
    const auto has_update = [this](ASurfaceControl* candidate) {
      return std::find_if(
                 transaction_->updates.begin(), transaction_->updates.end(),
                 [candidate](const SurfaceTransaction::Update& value) {
                   return value.opaque == candidate;
                 }) != transaction_->updates.end();
    };
    const auto has_control = [this](ASurfaceControl* candidate) {
      return std::find(transaction_->controls.begin(),
                       transaction_->controls.end(), candidate) !=
             transaction_->controls.end();
    };
    const bool missing_control_update = !has_update(control);
    const bool distinct_reference = reference != control;
    const bool missing_reference_update =
        distinct_reference && !has_update(reference);
    const bool missing_control_ref = !has_control(control);
    const bool missing_reference_ref =
        distinct_reference && !has_control(reference);
    const size_t update_additions =
        static_cast<size_t>(missing_control_update) +
        static_cast<size_t>(missing_reference_update);
    const size_t control_additions = static_cast<size_t>(missing_control_ref) +
                                     static_cast<size_t>(missing_reference_ref);
    // Prepare every allocation before publishing either referenced control.
    // The returned pointer is found only after both vectors are stable.
    if (update_additions != 0)
      transaction_->updates.reserve(transaction_->updates.size() +
                                    update_additions);
    if (control_additions != 0)
      transaction_->controls.reserve(transaction_->controls.size() +
                                     control_additions);
    if (missing_control_update)
      transaction_->updates.emplace_back(SurfaceTransaction::Update{.opaque = control});
    if (missing_reference_update)
      transaction_->updates.emplace_back(
          SurfaceTransaction::Update{.opaque = reference});
    if (missing_control_ref) {
      ASurfaceControl_acquire(control);
      transaction_->controls.push_back(control);
    }
    if (missing_reference_ref) {
      ASurfaceControl_acquire(reference);
      transaction_->controls.push_back(reference);
    }
    *update = &*std::find_if(
        transaction_->updates.begin(), transaction_->updates.end(),
        [control](const SurfaceTransaction::Update& value) {
          return value.opaque == control;
        });
    return true;
  } catch (...) {
    return false;
  }
}

bool SurfaceTransactionBuilder::Remember(ASurfaceControl* control) {
  SurfaceTransaction::Update* ignored = nullptr;
  return EnsureUpdate(control, &ignored);
}

bool SurfaceTransactionBuilder::SetReparent(ASurfaceControl* control,
                                            ASurfaceControl* parent) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdateWithReference(control, parent, &update)) return false;
  update->has_parent = true;
  update->parent = parent;
  return true;
}

bool SurfaceTransactionBuilder::SetVisibility(ASurfaceControl* control,
                                              bool visible) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->has_visibility = true;
  update->visible = visible;
  return true;
}

bool SurfaceTransactionBuilder::SetZOrder(ASurfaceControl* control,
                                          int32_t z_order) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->has_z_order = true;
  update->has_relative_layer = false;
  update->relative_to = nullptr;
  update->z_order = z_order;
  return true;
}

bool SurfaceTransactionBuilder::SetRelativeLayer(ASurfaceControl* control,
                                                 ASurfaceControl* relative_to,
                                                 int32_t z_order) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdateWithReference(control, relative_to, &update)) return false;
  update->has_z_order = false;
  update->has_relative_layer = true;
  update->relative_to = relative_to;
  update->z_order = z_order;
  return true;
}

bool SurfaceTransactionBuilder::SetBuffer(ASurfaceControl* control,
                                          AHardwareBuffer* buffer,
                                          int fence_fd) {
  return SetBufferInternal(control, buffer, fence_fd, 0);
}

bool SurfaceTransactionBuilder::SetBufferWithSubmissionCookie(
    ASurfaceControl* control, AHardwareBuffer* buffer, int fence_fd,
    uint64_t submission_cookie) {
  return SetBufferInternal(control, buffer, fence_fd, submission_cookie);
}

bool SurfaceTransactionBuilder::SetBufferInternal(
    ASurfaceControl* control, AHardwareBuffer* buffer, int fence_fd,
    uint64_t submission_cookie) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) {
    if (fence_fd >= 0)
      (void)darwin_art_bionic_socket_broker_close(fence_fd);
    return false;
  }

  DetachedBufferBatch detached;
  bool acquired_replacement = false;
  bool owns_incoming_fence = fence_fd >= 0;
  try {
    AHardwareBuffer* old_buffer = update->buffer;
    const int old_fence = update->acquire_fence;
    const size_t callback_count = static_cast<size_t>(std::count_if(
        transaction_->buffer_callbacks.begin(),
        transaction_->buffer_callbacks.end(),
        [control](const SurfaceTransaction::BufferCallback& callback) {
          return callback.control == control;
        }));
    detached.callbacks.reserve(callback_count);
    for (const auto& callback : transaction_->buffer_callbacks) {
      if (callback.control != control) continue;
      PreparedDiscard prepared{callback, DuplicateDiscardFence(old_fence)};
      try {
        detached.callbacks.push_back(prepared);
      } catch (...) {
        if (prepared.fence >= 0)
          (void)darwin_art_bionic_socket_broker_close(prepared.fence);
        throw;
      }
    }
    // Retain the replacement before publishing it. The retained reference is
    // owned by the transaction even if a callback deletes/reuses that object.
    if (buffer != nullptr) {
      AHardwareBuffer_acquire(buffer);
      acquired_replacement = true;
    }

    // Commit replacement and detach callbacks without invoking user code.
    // From the first callback below, transaction_ is never accessed again.
    update->buffer = buffer;
    update->acquire_fence = fence_fd;
    update->submission_cookie = submission_cookie;
    update->has_buffer = true;
    transaction_->buffer_callbacks.erase(
        std::remove_if(
            transaction_->buffer_callbacks.begin(),
            transaction_->buffer_callbacks.end(),
            [control](const SurfaceTransaction::BufferCallback& callback) {
              return callback.control == control;
            }),
        transaction_->buffer_callbacks.end());
    detached.Arm(old_buffer, old_fence);
    owns_incoming_fence = false;
  } catch (...) {
    if (acquired_replacement) AHardwareBuffer_release(buffer);
    if (owns_incoming_fence)
      (void)darwin_art_bionic_socket_broker_close(fence_fd);
    return false;
  }
  detached.Dispose();
  return true;
}

extern "C" bool
darwin_art_android_surface_transaction_set_buffer_with_cookie_checked(
    void* opaque, void* opaque_control, AHardwareBuffer* buffer, int fence_fd,
    uint64_t submission_cookie) {
  auto* transaction =
      reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  return SurfaceTransactionBuilder(transaction).SetBufferWithSubmissionCookie(
      control, buffer, fence_fd, submission_cookie);
}

extern "C" bool darwin_art_android_surface_transaction_set_destination_frame(
    void* opaque, void* opaque_control, ARect frame) {
  auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
  auto* control = reinterpret_cast<ASurfaceControl*>(opaque_control);
  return SurfaceTransactionBuilder(transaction).SetDestinationFrame(control, frame);
}

bool SurfaceTransactionBuilder::SetGeometry(ASurfaceControl* control,
                                            const ARect& source,
                                            const ARect& destination,
                                            int32_t transform) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->source = source;
  update->destination = destination;
  update->has_geometry = true;
  update->transform = transform;
  update->has_transform = true;
  return true;
}

bool SurfaceTransactionBuilder::SetDestinationFrame(ASurfaceControl* control,
                                                    const ARect& frame) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  if (!update->has_geometry) {
    // Whole-buffer source: every presentation consumer clamps the source to
    // the current buffer extent, so later buffer-size changes stay unscaled
    // sources scaled into this frame, as SurfaceFlinger does.
    update->source = ARect{0, 0, INT32_MAX, INT32_MAX};
  }
  update->destination = frame;
  update->has_geometry = true;
  return true;
}

bool SurfaceTransactionBuilder::SetCrop(ASurfaceControl* control,
                                        const ARect& crop) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->crop = crop;
  update->has_crop = true;
  return true;
}

bool SurfaceTransactionBuilder::SetPosition(ASurfaceControl* control, int32_t x,
                                            int32_t y) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->has_position = true;
  update->position_x = x;
  update->position_y = y;
  return true;
}

bool SurfaceTransactionBuilder::SetBufferTransform(ASurfaceControl* control,
                                                   int32_t transform) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->has_transform = true;
  update->transform = transform;
  return true;
}

bool SurfaceTransactionBuilder::SetScale(ASurfaceControl* control, float x,
                                         float y) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->has_scale = true;
  update->scale_x = x;
  update->scale_y = y;
  return true;
}

bool SurfaceTransactionBuilder::SetDamageRegion(ASurfaceControl* control,
                                                const ARect* rects,
                                                uint32_t count) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  try {
    std::vector<ARect> damage;
    if (rects != nullptr && count != 0)
      damage.assign(rects, rects + count);
    update->damage = std::move(damage);
  } catch (...) {
    return false;
  }
  update->has_damage = true;
  return true;
}

bool SurfaceTransactionBuilder::SetTransparentRegion(ASurfaceControl* control,
                                                      const int32_t* rects,
                                                      size_t count) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  try {
    std::vector<ARect> region;
    if (rects == nullptr) count = 0;
    count = std::min(count, kMaxTransparentRegionRects);
    region.reserve(count);
    for (size_t index = 0; index < count; ++index) {
      const int32_t* rect = rects + index * 4;
      if (rect[2] <= rect[0] || rect[3] <= rect[1]) continue;
      region.push_back({rect[0], rect[1], rect[2], rect[3]});
    }
    update->transparent_region = std::move(region);
  } catch (...) {
    return false;
  }
  update->has_transparent_region = true;
  return true;
}

bool SurfaceTransactionBuilder::SetAlpha(ASurfaceControl* control,
                                         float alpha) {
  SurfaceTransaction::Update* update = nullptr;
  if (!EnsureUpdate(control, &update)) return false;
  update->has_alpha = true;
  update->alpha = alpha;
  return true;
}

}  // namespace darwin_art::window
