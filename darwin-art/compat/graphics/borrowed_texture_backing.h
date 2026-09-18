#pragma once
#include "surface_backing_owner.h"

namespace darwin_art::graphics {
// Transferred to Skia's textureReleaseProc. Cache replacement cannot retire
// this exact native tuple until Skia releases its borrowed texture.
struct BorrowedTextureBacking final {
  SurfaceBackingOwner::Handle backing;
  static void Release(void* context) noexcept {
    delete static_cast<BorrowedTextureBacking*>(context);
  }
};
}  // namespace darwin_art::graphics
