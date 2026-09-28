#include <iostream>

#include "include/android/SkAndroidFrameworkUtils.h"
#include "include/core/SkCanvas.h"
#include "include/core/SkImageInfo.h"
#include "include/core/SkRect.h"
#include "include/core/SkSurface.h"

#ifndef SK_BUILD_FOR_ANDROID_FRAMEWORK
#error "This gate must use the same Android framework ABI as libskia.a"
#endif

int main() {
  constexpr int kWidth = 64;
  constexpr int kHeight = 64;
  sk_sp<SkSurface> surface =
      SkSurfaces::Raster(SkImageInfo::MakeN32Premul(kWidth, kHeight));
  if (!surface) {
    std::cerr << "SkSurfaces::Raster failed\n";
    return 1;
  }

  SkCanvas* canvas = surface->getCanvas();
  if (SkAndroidFrameworkUtils::getBaseWrappedCanvas(canvas) != canvas) {
    std::cerr << "getBaseWrappedCanvas did not preserve a base canvas\n";
    return 2;
  }
  sk_sp<SkSurface> recovered =
      SkAndroidFrameworkUtils::getSurfaceFromCanvas(canvas);
  if (recovered.get() != surface.get()) {
    std::cerr << "getSurfaceFromCanvas did not recover its owner\n";
    return 3;
  }

  canvas->clipRect(SkRect::MakeWH(8, 9));
  SkIRect clip;
  if (!canvas->getDeviceClipBounds(&clip) || clip.width() != 8 ||
      clip.height() != 9) {
    std::cerr << "failed to establish clipped bounds\n";
    return 4;
  }
  SkAndroidFrameworkUtils::ResetClip(canvas);
  if (!canvas->getDeviceClipBounds(&clip) || clip.width() != kWidth ||
      clip.height() != kHeight) {
    std::cerr << "ResetClip did not restore device bounds\n";
    return 5;
  }

  std::cout << "Skia Android framework utils: base-canvas=same surface=same "
               "reset-clip="
            << clip.width() << "x" << clip.height() << "\n";
  return 0;
}
