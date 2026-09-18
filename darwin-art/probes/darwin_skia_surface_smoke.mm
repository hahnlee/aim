#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <memory>

#include "darwin_surface_bridge.h"
#include "darwin_framework_input_hint.h"
#include "include/core/SkCanvas.h"
#include "include/core/SkColor.h"
#include "include/core/SkImageInfo.h"
#include "include/core/SkPaint.h"
#include "include/core/SkPath.h"
#include "include/core/SkPathBuilder.h"
#include "include/core/SkRect.h"

// The smoke executable intentionally links only the surface bridge and Skia;
// provide inert providers for the runtime-owned input/fence hooks that are
// unreachable in headless mode. Production graphics targets link the real
// implementations from the runtime provider closure.
namespace darwin_art {
DarwinArtInputEnqueueResult EnqueueFrameworkPointerPacket(
    const DarwinArtPointerEventV2&) {
  return DarwinArtInputEnqueueResult::kNoFocusedChannel;
}
DarwinArtInputEnqueueResult EnqueueFrameworkKeyPacket(
    const DarwinArtKeyEventV1&) {
  return DarwinArtInputEnqueueResult::kNoFocusedChannel;
}
}  // namespace darwin_art

struct DarwinArtBionicPollFd {
  int32_t fd;
  int16_t events;
  int16_t revents;
};
extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd*, size_t, int) {
  return 0;
}
extern "C" int darwin_art_bionic_socket_broker_close(int) { return 0; }

namespace {

constexpr uint32_t kWidth = 640;
constexpr uint32_t kHeight = 360;
constexpr int kFrameCount = 120;
constexpr uint64_t kExpectedFinalFrameHash = 0xb32235958413af5eULL;
constexpr uint64_t kExpectedSequenceHash = 0x5fd4ea042fd71d4eULL;

uint64_t Fnv1a64Rows(const DarwinArtSurfaceProducerMapping& mapping) {
  constexpr uint64_t kOffsetBasis = 14695981039346656037ULL;
  constexpr uint64_t kPrime = 1099511628211ULL;
  const auto* pixels = static_cast<const uint8_t*>(mapping.base_address);
  const size_t visible_row_bytes = static_cast<size_t>(mapping.width) * 4;
  uint64_t hash = kOffsetBasis;
  for (uint32_t row = 0; row < mapping.height; ++row) {
    const uint8_t* source = pixels + static_cast<size_t>(row) *
                                        mapping.bytes_per_row;
    for (size_t index = 0; index < visible_row_bytes; ++index) {
      hash ^= source[index];
      hash *= kPrime;
    }
  }
  return hash;
}

uint64_t MixFrameHash(uint64_t sequence_hash, uint64_t frame_hash) {
  constexpr uint64_t kPrime = 1099511628211ULL;
  for (unsigned shift = 0; shift < 64; shift += 8) {
    sequence_hash ^= static_cast<uint8_t>(frame_hash >> shift);
    sequence_hash *= kPrime;
  }
  return sequence_hash;
}

void DrawFrame(SkCanvas* canvas, int frame) {
  canvas->clear(SkColorSetARGB(255, 12, 20, 32));

  SkPaint paint;
  paint.setAntiAlias(false);
  paint.setColor(SkColorSetARGB(255, 61, 220, 132));
  const SkScalar offset = static_cast<SkScalar>((frame * 7) % 220);
  canvas->drawRect(SkRect::MakeXYWH(24 + offset, 28, 180, 72), paint);

  SkPath path = SkPathBuilder()
                    .moveTo(72, 302)
                    .lineTo(320, 116 + static_cast<SkScalar>(frame % 29))
                    .lineTo(568, 302)
                    .close()
                    .detach();
  paint.setColor(SkColorSetARGB(255, 37, 99, 235));
  canvas->drawPath(path, paint);

  paint.setStyle(SkPaint::kStroke_Style);
  paint.setStrokeWidth(8);
  paint.setColor(SkColorSetARGB(255, 248, 250, 252));
  canvas->drawCircle(320, 184, 42 + static_cast<SkScalar>(frame % 17), paint);
}

}  // namespace

int main() {
  @autoreleasepool {
    const bool visible = std::getenv("DARWIN_ART_SURFACE_HEADLESS") == nullptr;
    DarwinArtSurfaceCreateInfo create_info{
        .width = kWidth,
        .height = kHeight,
        .title = "Darwin ART · direct Skia IOSurface",
        .visible = visible,
    };
    DarwinArtSurfaceResult result = DARWIN_ART_SURFACE_OK;
    DarwinArtSurface* surface =
        darwin_art_surface_create(&create_info, &result);
    if (surface == nullptr) {
      std::cerr << "darwin-skia-surface: create failed=" << result << "\n";
      return 1;
    }

    std::unique_ptr<SkCanvas> canvas;
    void* stable_base_address = nullptr;
    size_t stable_row_bytes = 0;
    uint64_t final_frame_hash = 0;
    uint64_t sequence_hash = 14695981039346656037ULL;
    int presented_frames = 0;

    for (int frame = 0; frame < kFrameCount; ++frame) {
      DarwinArtSurfaceProducerMapping mapping{};
      result = darwin_art_surface_map_producer(surface, &mapping);
      if (result != DARWIN_ART_SURFACE_OK) {
        std::cerr << "darwin-skia-surface: map failed=" << result << "\n";
        darwin_art_surface_destroy(surface);
        return 2;
      }
      if (mapping.width != kWidth || mapping.height != kHeight ||
          mapping.bytes_per_row < static_cast<size_t>(kWidth) * 4 ||
          mapping.allocation_size < mapping.bytes_per_row * mapping.height) {
        std::cerr << "darwin-skia-surface: invalid mapping\n";
        darwin_art_surface_unmap_producer(surface);
        darwin_art_surface_destroy(surface);
        return 3;
      }
      if (frame == 0) {
        DarwinArtSurfaceProducerMapping duplicate_mapping{};
        if (darwin_art_surface_map_producer(surface, &duplicate_mapping) !=
                DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED ||
            darwin_art_surface_present(surface) !=
                DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED) {
          std::cerr << "darwin-skia-surface: single-map guard failed\n";
          darwin_art_surface_unmap_producer(surface);
          darwin_art_surface_destroy(surface);
          return 4;
        }
      }

      if (!canvas) {
        stable_base_address = mapping.base_address;
        stable_row_bytes = mapping.bytes_per_row;
        canvas = SkCanvas::MakeRasterDirect(
            SkImageInfo::MakeN32Premul(kWidth, kHeight),
            mapping.base_address, mapping.bytes_per_row);
        if (!canvas) {
          std::cerr << "darwin-skia-surface: MakeRasterDirect failed\n";
          darwin_art_surface_unmap_producer(surface);
          darwin_art_surface_destroy(surface);
          return 5;
        }
      } else if (mapping.base_address != stable_base_address ||
                 mapping.bytes_per_row != stable_row_bytes) {
        std::cerr << "darwin-skia-surface: IOSurface mapping changed\n";
        darwin_art_surface_unmap_producer(surface);
        darwin_art_surface_destroy(surface);
        return 6;
      }

      DrawFrame(canvas.get(), frame);
      final_frame_hash = Fnv1a64Rows(mapping);
      sequence_hash = MixFrameHash(sequence_hash, final_frame_hash);

      result = darwin_art_surface_unmap_producer(surface);
      if (result != DARWIN_ART_SURFACE_OK) {
        std::cerr << "darwin-skia-surface: unmap failed=" << result << "\n";
        darwin_art_surface_destroy(surface);
        return 7;
      }
      if (frame == 0 &&
          darwin_art_surface_unmap_producer(surface) !=
              DARWIN_ART_SURFACE_PRODUCER_NOT_MAPPED) {
        std::cerr << "darwin-skia-surface: unmap-state guard failed\n";
        darwin_art_surface_destroy(surface);
        return 8;
      }
      result = darwin_art_surface_present(surface);
      if (result != DARWIN_ART_SURFACE_OK) {
        std::cerr << "darwin-skia-surface: present failed=" << result << "\n";
        darwin_art_surface_destroy(surface);
        return 9;
      }
      ++presented_frames;
      result = darwin_art_surface_pump_events(surface, visible ? 0.001 : 0.0);
      if (result == DARWIN_ART_SURFACE_WINDOW_CLOSED) {
        break;
      }
      if (result != DARWIN_ART_SURFACE_OK) {
        std::cerr << "darwin-skia-surface: pump failed=" << result << "\n";
        darwin_art_surface_destroy(surface);
        return 10;
      }
    }

    canvas.reset();
    result = darwin_art_surface_destroy(surface);
    if (result != DARWIN_ART_SURFACE_OK) {
      std::cerr << "darwin-skia-surface: destroy failed=" << result << "\n";
      return 11;
    }
    if (presented_frames != kFrameCount) {
      std::cout << "darwin-skia-surface: window-closed frames="
                << presented_frames << "\n";
      return 0;
    }
    if (final_frame_hash != kExpectedFinalFrameHash ||
        sequence_hash != kExpectedSequenceHash) {
      std::cerr << "darwin-skia-surface: deterministic hash mismatch final="
                << std::hex << final_frame_hash << " sequence="
                << sequence_hash << "\n";
      return 12;
    }

    std::cout << "darwin-skia-surface: direct-iosurface frames="
              << presented_frames << " staging-copies=0 final-hash="
              << std::hex << std::setw(16) << std::setfill('0')
              << final_frame_hash << " sequence-hash=" << std::setw(16)
              << sequence_hash << "\n";
  }
  return 0;
}
