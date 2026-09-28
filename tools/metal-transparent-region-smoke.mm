#import <Foundation/Foundation.h>
#import <IOSurface/IOSurface.h>
#import <Metal/Metal.h>
#include "../compat/surfaceflinger/metal_composer.h"
#include <cassert>
#include <cstdio>
#include <unistd.h>

static IOSurfaceRef MakeSurface(uint32_t color) {
  NSDictionary* properties = @{
    (id)kIOSurfaceWidth: @4, (id)kIOSurfaceHeight: @4,
    (id)kIOSurfaceBytesPerElement: @4, (id)kIOSurfaceBytesPerRow: @16,
    (id)kIOSurfaceAllocSize: @64,
    (id)kIOSurfacePixelFormat: @((uint32_t)'BGRA')
  };
  IOSurfaceRef surface = IOSurfaceCreate((CFDictionaryRef)properties);
  assert(surface != nullptr);
  assert(IOSurfaceLock(surface, 0, nullptr) == 0);
  auto* data = static_cast<uint32_t*>(IOSurfaceGetBaseAddress(surface));
  for (unsigned i = 0; i < 16; ++i) data[i] = color;
  IOSurfaceUnlock(surface, 0, nullptr);
  return surface;
}

int main() {
  @autoreleasepool {
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    assert(device != nil);
    IOSurfaceRef blue = MakeSurface(0xff0000ff);
    IOSurfaceRef red = MakeSurface(0xffff0000);
    IOSurfaceRef target = MakeSurface(0);
    AimMetalComposerLayer layers[2]{};
    for (auto& layer : layers) {
      layer.width = layer.height = 4;
      layer.source_right = layer.source_bottom = 4;
      layer.destination_right = layer.destination_bottom = 4;
    }
    layers[0].iosurface = blue;
    layers[1].iosurface = red;
    layers[1].transparent_region_count = 1;
    layers[1].transparent_region[0] = {1, 1, 3, 3};
    for (unsigned pass = 0; pass < 2; ++pass) {
      if (pass == 1) layers[1].transparent_region_count = 0;
      void* completion = nullptr;
      uint64_t value = 0;
      assert(aim_metal_composer_compose(
          device, target, 4, 4, layers, 2, nullptr, 0, &completion, &value));
      id<MTLSharedEvent> event = reinterpret_cast<id<MTLSharedEvent>>(completion);
      for (unsigned wait = 0; event.signaledValue < value && wait < 5000; ++wait)
        usleep(1000);
      assert(event.signaledValue >= value);
      assert(IOSurfaceLock(target, kIOSurfaceLockReadOnly, nullptr) == 0);
      const auto* pixels = static_cast<const uint32_t*>(IOSurfaceGetBaseAddress(target));
      for (unsigned y = 0; y < 4; ++y) {
        for (unsigned x = 0; x < 4; ++x) {
          const bool hole = pass == 0 && x >= 1 && x < 3 && y >= 1 && y < 3;
          assert(pixels[y * 4 + x] == (hole ? 0xff0000ff : 0xffff0000));
        }
      }
      IOSurfaceUnlock(target, kIOSurfaceLockReadOnly, nullptr);
      [event release];
    }
    CFRelease(blue); CFRelease(red); CFRelease(target);
    [device release];
    std::puts("metal-transparent-region-smoke: PASS GPU hole and explicit clear");
  }
}
