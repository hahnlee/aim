#pragma once

#include "surface_transaction_state.h"

#include <android/surface_control.h>

#include <cstddef>
#include <cstdint>

namespace darwin_art::window {

// Android-owned transaction construction. This is the only producer-facing
// API allowed to retain referenced controls, create updates, or replace an
// incoming buffer/fence. Setter callers never receive an Update pointer: a
// reentrant discard callback may clear, delete, and reuse the public
// transaction while the detached replacement batch is being disposed.
class SurfaceTransactionBuilder final {
 public:
  explicit SurfaceTransactionBuilder(SurfaceTransaction* transaction)
      : transaction_(transaction) {}

  bool Remember(ASurfaceControl* control);
  bool SetReparent(ASurfaceControl* control, ASurfaceControl* parent);
  bool SetVisibility(ASurfaceControl* control, bool visible);
  bool SetZOrder(ASurfaceControl* control, int32_t z_order);
  bool SetRelativeLayer(ASurfaceControl* control, ASurfaceControl* relative_to,
                        int32_t z_order);
  bool SetBuffer(ASurfaceControl* control, AHardwareBuffer* buffer,
                 int fence_fd);
  // The cookie is committed atomically with the buffer replacement.  Zero is
  // the ordinary NDK path and explicitly clears any prior submission cookie.
  bool SetBufferWithSubmissionCookie(ASurfaceControl* control,
                                     AHardwareBuffer* buffer, int fence_fd,
                                     uint64_t submission_cookie);
  bool SetGeometry(ASurfaceControl* control, const ARect& source,
                   const ARect& destination, int32_t transform);
  // layer_state_t::destinationFrame: scale the whole buffer into `frame`
  // (layer-local coordinates). Keeps an explicit source set in this update.
  bool SetDestinationFrame(ASurfaceControl* control, const ARect& frame);
  bool SetCrop(ASurfaceControl* control, const ARect& crop);
  bool SetPosition(ASurfaceControl* control, int32_t x, int32_t y);
  bool SetBufferTransform(ASurfaceControl* control, int32_t transform);
  bool SetScale(ASurfaceControl* control, float x, float y);
  bool SetDamageRegion(ASurfaceControl* control, const ARect* rects,
                       uint32_t count);
  bool SetTransparentRegion(ASurfaceControl* control, const int32_t* rects,
                            size_t count);
  bool SetAlpha(ASurfaceControl* control, float alpha);

 private:
  bool EnsureUpdate(ASurfaceControl* control,
                    SurfaceTransaction::Update** update);
  bool EnsureUpdateWithReference(ASurfaceControl* control,
                                 ASurfaceControl* reference,
                                 SurfaceTransaction::Update** update);
  bool SetBufferInternal(ASurfaceControl* control, AHardwareBuffer* buffer,
                         int fence_fd, uint64_t submission_cookie);

  SurfaceTransaction* transaction_ = nullptr;
};

}  // namespace darwin_art::window
