#include "graphics_ndk_image.h"
#include <cstring>

namespace darwin_art::loader {
namespace {
const void* Marker() {
  // Process-resident code has no independent allocation to retain. Rust's
  // registry owns image identity, namespace membership and open references.
  static const uint64_t marker = 0x44415257474e444bULL;
  return &marker;
}
void* Retain(void* source) { return source == Marker() ? source : nullptr; }
void Release(void*) {}
bool Fail(std::string* error, const char* message) {
  if (error) *error = message;
  return false;
}
}
bool PublishGraphicsNdkImage(LinkerRegistry* registry, uint64_t namespace_id,
    const char* soname, const char* canonical, std::string* error) {
  if (error) error->clear();
  if (!registry || !soname || std::strcmp(soname, "libjnigraphics.so") != 0 ||
      !canonical || canonical[0] != '/')
    return Fail(error, "graphics NDK image requires libjnigraphics.so and an absolute path");
  if (darwin_art_linker_namespace_publish_image(registry, namespace_id,
          soname, canonical, const_cast<void*>(Marker()), Retain, Release,
          0, 0, DARWIN_ART_IMAGE_GRAPHICS_NDK) != 0)
    return Fail(error, "graphics NDK image publication failed");
  return true;
}
bool IsGraphicsNdkImage(const LinkerImageLease* image) {
  if (!image) return false;
  void* payload = nullptr;
  return darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_GRAPHICS_NDK,
      &payload) == 0 && payload == Marker();
}
int ResolveGraphicsNdkImage(const LinkerImageLease* image, const char* symbol,
    const char* version, uintptr_t* out, std::string* error) {
  if (error) error->clear();
  if (out) *out = 0;
  if (!out || !symbol || !*symbol || !IsGraphicsNdkImage(image)) {
    Fail(error, "invalid graphics NDK image lookup");
    return -1;
  }
  if (version && std::strcmp(version, "LIBJNIGRAPHICS") != 0) {
    Fail(error, "unsupported graphics NDK symbol version");
    return 1;
  }
  *out = GraphicsNdkSymbol(symbol);
  if (!*out) {
    Fail(error, "symbol is not in the original graphics NDK public API");
    return 1;
  }
  return 0;
}
}
