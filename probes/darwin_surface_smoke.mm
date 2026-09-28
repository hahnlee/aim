#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <vector>

#include "darwin_surface_bridge.h"

namespace {

constexpr uint32_t kWidth = 640;
constexpr uint32_t kHeight = 360;

int FrameCount() {
  const char* value = std::getenv("AIM_SURFACE_FRAMES");
  if (value == nullptr) {
    return 120;
  }
  char* end = nullptr;
  const long parsed = std::strtol(value, &end, 10);
  return end == value ? 120 : static_cast<int>(std::clamp(parsed, 1L, 600L));
}

void FillFrame(std::vector<uint32_t>* pixels, uint32_t frame) {
  for (uint32_t y = 0; y < kHeight; ++y) {
    for (uint32_t x = 0; x < kWidth; ++x) {
      const uint8_t red = static_cast<uint8_t>((x + frame * 2) & 0xff);
      const uint8_t green = static_cast<uint8_t>((y + frame) & 0xff);
      const uint8_t blue = static_cast<uint8_t>((x + y + frame * 3) & 0xff);
      // A 0xAARRGGBB integer is BGRA in little-endian memory.
      (*pixels)[static_cast<size_t>(y) * kWidth + x] =
          0xff000000u | (static_cast<uint32_t>(red) << 16) |
          (static_cast<uint32_t>(green) << 8) | blue;
    }
  }
}

}  // namespace

int main() {
  @autoreleasepool {
    const bool visible = std::getenv("AIM_SURFACE_HEADLESS") == nullptr;
    AimSurfaceCreateInfo create_info{
        .width = kWidth,
        .height = kHeight,
        .title = "aim · persistent IOSurface smoke",
        .visible = visible,
    };
    AimSurfaceResult result = AIM_SURFACE_OK;
    AimSurface* surface =
        aim_surface_create(&create_info, &result);
    if (surface == nullptr) {
      std::cerr << "darwin-surface-smoke: create failed=" << result << "\n";
      return 1;
    }

    std::vector<uint32_t> pixels(static_cast<size_t>(kWidth) * kHeight);
    const int frame_count = FrameCount();
    int presented_frames = 0;
    for (int frame = 0; frame < frame_count; ++frame) {
      FillFrame(&pixels, static_cast<uint32_t>(frame));
      result = aim_surface_update(
          surface, pixels.data(), static_cast<size_t>(kWidth) * sizeof(uint32_t));
      if (result != AIM_SURFACE_OK) {
        std::cerr << "darwin-surface-smoke: update failed=" << result << "\n";
        aim_surface_destroy(surface);
        return 2;
      }
      result = aim_surface_present(surface);
      if (result != AIM_SURFACE_OK) {
        std::cerr << "darwin-surface-smoke: present failed=" << result << "\n";
        aim_surface_destroy(surface);
        return 3;
      }
      ++presented_frames;
      result = aim_surface_pump_events(surface, visible ? 0.016 : 0.0);
      if (result == AIM_SURFACE_WINDOW_CLOSED) {
        break;
      }
      if (result != AIM_SURFACE_OK) {
        std::cerr << "darwin-surface-smoke: pump failed=" << result << "\n";
        aim_surface_destroy(surface);
        return 4;
      }
    }

    if (aim_surface_pump_events(surface, -0.001) !=
            AIM_SURFACE_INVALID_ARGUMENT ||
        aim_surface_pump_events(
            surface, std::numeric_limits<double>::infinity()) !=
            AIM_SURFACE_INVALID_ARGUMENT ||
        aim_surface_pump_events(surface, 30.001) !=
            AIM_SURFACE_INVALID_ARGUMENT) {
      std::cerr << "darwin-surface-smoke: pump bounds validation failed\n";
      aim_surface_destroy(surface);
      return 5;
    }

    result = aim_surface_destroy(surface);
    if (result != AIM_SURFACE_OK) {
      std::cerr << "darwin-surface-smoke: destroy failed=" << result << "\n";
      return 6;
    }
    std::cout << "darwin-surface-smoke: persistent-iosurface-metal frames="
              << presented_frames << "\n";
  }
  return 0;
}
