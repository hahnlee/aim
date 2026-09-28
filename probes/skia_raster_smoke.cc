#include <cstddef>
#include <cstdint>
#include <iomanip>
#include <iostream>

#include "include/core/SkCanvas.h"
#include "include/core/SkColor.h"
#include "include/core/SkImageInfo.h"
#include "include/core/SkPaint.h"
#include "include/core/SkPath.h"
#include "include/core/SkPathBuilder.h"
#include "include/core/SkPixmap.h"
#include "include/core/SkSurface.h"

namespace {

std::uint64_t Fnv1a64(const void* data, std::size_t size) {
  const auto* bytes = static_cast<const std::uint8_t*>(data);
  std::uint64_t hash = 14695981039346656037ULL;
  for (std::size_t index = 0; index < size; ++index) {
    hash ^= bytes[index];
    hash *= 1099511628211ULL;
  }
  return hash;
}

}  // namespace

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
  canvas->clear(SkColorSetARGB(255, 12, 20, 32));

  SkPaint paint;
  paint.setAntiAlias(false);
  paint.setColor(SkColorSetARGB(255, 230, 48, 64));
  canvas->drawRect(SkRect::MakeXYWH(4, 5, 27, 19), paint);

  SkPath path = SkPathBuilder()
                    .moveTo(9, 55)
                    .lineTo(34, 24)
                    .lineTo(58, 55)
                    .close()
                    .detach();
  paint.setColor(SkColorSetARGB(255, 52, 168, 83));
  canvas->drawPath(path, paint);

  paint.setStyle(SkPaint::kStroke_Style);
  paint.setStrokeWidth(3);
  paint.setColor(SkColorSetARGB(255, 66, 133, 244));
  canvas->drawCircle(47, 14, 9, paint);

  SkPixmap pixels;
  if (!surface->peekPixels(&pixels) || pixels.addr() == nullptr) {
    std::cerr << "SkSurface::peekPixels failed\n";
    return 2;
  }
  const std::uint64_t hash = Fnv1a64(pixels.addr(), pixels.computeByteSize());
  std::cout << "Skia Darwin raster: " << pixels.width() << "x"
            << pixels.height() << " rowBytes=" << pixels.rowBytes()
            << " hash=" << std::hex << std::setw(16) << std::setfill('0')
            << hash << "\n";
  return 0;
}
