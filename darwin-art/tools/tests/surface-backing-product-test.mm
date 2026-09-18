#import <AppKit/AppKit.h>
#import <IOSurface/IOSurface.h>
#include "compat/darwin_surface_bridge.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <unistd.h>

template <typename T> T Load(void* image, const char* name) {
  auto function = reinterpret_cast<T>(dlsym(image, name));
  if (!function) std::fprintf(stderr, "%s: %s\n", name, dlerror());
  assert(function != nullptr);
  return function;
}

int main(int argc, char** argv) {
  assert(argc == 3);
  const bool graphics = std::strcmp(argv[2], "graphics") == 0;
  assert(graphics || std::strcmp(argv[2], "headless") == 0);
  @autoreleasepool {
    [NSApplication sharedApplication];
    void* image = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
    if (!image) std::fprintf(stderr, "dlopen: %s\n", dlerror());
    assert(image != nullptr);
    auto create = Load<decltype(&darwin_art_surface_create)>(image, "darwin_art_surface_create");
    auto map = Load<decltype(&darwin_art_surface_map_producer)>(image, "darwin_art_surface_map_producer");
    auto unmap = Load<decltype(&darwin_art_surface_unmap_producer)>(image, "darwin_art_surface_unmap_producer");
    auto resize = Load<decltype(&darwin_art_surface_resize)>(image, "darwin_art_surface_resize");
    auto size = Load<decltype(&darwin_art_surface_get_size)>(image, "darwin_art_surface_get_size");
    auto identifier = reinterpret_cast<decltype(&darwin_art_surface_gpu_iosurface_id)>(
        dlsym(image, "darwin_art_surface_gpu_iosurface_id"));
    assert(graphics ? identifier != nullptr : identifier == nullptr);
    auto destroy = Load<decltype(&darwin_art_surface_destroy)>(image, "darwin_art_surface_destroy");
    // Offscreen real GPU backing; no installed APK or desktop app is closed.
    DarwinArtSurfaceCreateInfo info{720, 1280, "backing contract", false, false};
    DarwinArtSurfaceResult result{};
    auto* surface = create(&info, &result);
    assert(surface != nullptr && result == DARWIN_ART_SURFACE_OK);
    const auto old_id = graphics ? identifier(surface) : 0;
    IOSurfaceRef retained = graphics ? IOSurfaceLookup(old_id) : nullptr;
    if (graphics) assert(retained && IOSurfaceGetWidth(retained) == 720);
    DarwinArtSurfaceProducerMapping mapping{};
    assert(map(surface, &mapping) == DARWIN_ART_SURFACE_OK);
    assert(mapping.width == 720 && mapping.height == 1280 && mapping.bytes_per_row >= 720 * 4);
    std::memset(mapping.base_address, 0x37, mapping.bytes_per_row);
    assert(resize(surface, 640, 360) == DARWIN_ART_SURFACE_PRODUCER_ALREADY_MAPPED);
    if (graphics) assert(identifier(surface) == old_id);
    assert(unmap(surface) == DARWIN_ART_SURFACE_OK);
    assert(unmap(surface) == DARWIN_ART_SURFACE_PRODUCER_NOT_MAPPED);
    assert(resize(surface, 640, 360) == DARWIN_ART_SURFACE_OK);
    uint32_t width = 0, height = 0;
    assert(size(surface, &width, &height) && width == 640 && height == 360);
    if (graphics) {
      assert(identifier(surface) != old_id);
      assert(IOSurfaceGetWidth(retained) == 720 && IOSurfaceGetHeight(retained) == 1280);
    }
    assert(resize(surface, 0, 360) == DARWIN_ART_SURFACE_INVALID_ARGUMENT);
    assert(size(surface, &width, &height) && width == 640 && height == 360);
    assert(map(surface, &mapping) == DARWIN_ART_SURFACE_OK);
    assert(mapping.width == 640 && mapping.height == 360);
    assert(unmap(surface) == DARWIN_ART_SURFACE_OK);
    assert(destroy(surface) == DARWIN_ART_SURFACE_OK);
    if (retained) CFRelease(retained);
    std::puts("product backing: real GPU creation/map-resize rejection/exact unmap/invalid candidate/destruction PASS");
    std::puts(graphics ? "graphics: retained IOSurface across replacement PASS"
                       : "headless: GPU surface-id capability remains unexported PASS");
  }
  // Loading the product runtime is process-scoped, not reusable VM unload proof.
  std::fflush(nullptr);
  _exit(0);
}
