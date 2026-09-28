#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <iomanip>
#include <iostream>

#include "include/core/SkCanvas.h"
#include "include/core/SkColor.h"
#include "include/core/SkData.h"
#include "include/core/SkFont.h"
#include "include/core/SkFontMgr.h"
#include "include/core/SkImageInfo.h"
#include "include/core/SkPaint.h"
#include "include/core/SkPixmap.h"
#include "include/core/SkSurface.h"
#include "include/core/SkTypeface.h"
#include "include/ports/SkFontMgr_data.h"

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

int main(int argc, char** argv) {
  if (argc != 2) {
    std::cerr << "usage: skia-text-raster-smoke ROBOTO_REGULAR_TTF\n";
    return 2;
  }

  sk_sp<SkData> roboto = SkData::MakeFromFileName(argv[1]);
  if (!roboto || roboto->isEmpty()) {
    std::cerr << "skia-text-raster-smoke: could not read pinned Roboto\n";
    return 3;
  }
  std::array<sk_sp<SkData>, 1> font_data = {roboto};
  sk_sp<SkFontMgr> font_manager = SkFontMgr_New_Custom_Data(SkSpan(font_data));
  if (!font_manager || font_manager->countFamilies() != 1) {
    std::cerr << "skia-text-raster-smoke: FreeType data font manager failed\n";
    return 4;
  }
  sk_sp<SkTypeface> typeface = font_manager->makeFromData(roboto);
  if (!typeface) {
    std::cerr << "skia-text-raster-smoke: Roboto typeface creation failed\n";
    return 5;
  }

  constexpr char kText[] = "Click";
  constexpr int kWidth = 192;
  constexpr int kHeight = 80;
  SkFont font(typeface, 42.0f);
  font.setEdging(SkFont::Edging::kAntiAlias);
  font.setHinting(SkFontHinting::kNormal);
  SkGlyphID glyphs[5] = {};
  const int glyph_count = font.textToGlyphs(kText, std::strlen(kText),
                                            SkTextEncoding::kUTF8, glyphs, 5);
  if (glyph_count != 5) {
    std::cerr << "skia-text-raster-smoke: unexpected glyph count\n";
    return 6;
  }
  for (SkGlyphID glyph : glyphs) {
    if (glyph == 0) {
      std::cerr << "skia-text-raster-smoke: missing Roboto glyph\n";
      return 7;
    }
  }

  sk_sp<SkSurface> surface =
      SkSurfaces::Raster(SkImageInfo::MakeN32Premul(kWidth, kHeight));
  if (!surface) {
    std::cerr << "skia-text-raster-smoke: raster surface creation failed\n";
    return 8;
  }
  SkCanvas* canvas = surface->getCanvas();
  canvas->clear(SK_ColorWHITE);
  SkPaint paint;
  paint.setAntiAlias(true);
  paint.setColor(SK_ColorBLACK);
  canvas->drawSimpleText(kText, std::strlen(kText), SkTextEncoding::kUTF8,
                         8.0f, 56.0f, font, paint);

  SkPixmap pixels;
  if (!surface->peekPixels(&pixels) || pixels.addr() == nullptr) {
    std::cerr << "skia-text-raster-smoke: pixel access failed\n";
    return 9;
  }
  std::size_t ink_pixels = 0;
  for (int y = 0; y < pixels.height(); ++y) {
    const auto* row = static_cast<const std::uint32_t*>(pixels.addr(0, y));
    for (int x = 0; x < pixels.width(); ++x) {
      if (row[x] != SK_ColorWHITE) {
        ++ink_pixels;
      }
    }
  }
  if (ink_pixels == 0) {
    std::cerr << "skia-text-raster-smoke: Roboto produced no glyph pixels\n";
    return 10;
  }
  const std::uint64_t hash = Fnv1a64(pixels.addr(), pixels.computeByteSize());
  std::cout << "Skia Android text raster: Click glyphs=" << glyph_count
            << " ink=" << ink_pixels << " rowBytes=" << pixels.rowBytes()
            << " hash=" << std::hex << std::setw(16) << std::setfill('0')
            << hash << "\n";
  return 0;
}
