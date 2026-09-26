#include "runtime_module_jni.h"

#include <dlfcn.h>

#include <array>
#include <cstring>
#include <string_view>

namespace darwin_art::loader {
namespace {

constexpr std::string_view kArtModuleJavalib = "/apex/com.android.art/javalib/";
// libartservice: service-art.jar (ArtJni), built from the pinned ART sources
// into this image.
constexpr std::array<std::string_view, 1> kArtModuleJniLibraries = {"libartservice.so"};

bool IsArtModuleCaller(std::string_view location, const char* android_filesystem_root) {
  // System-server classpath entries are opened at their image-root backing.
  if (android_filesystem_root != nullptr && android_filesystem_root[0] == '/') {
    const std::string_view root(android_filesystem_root);
    if (location.starts_with(root)) location.remove_prefix(root.size());
  }
  return location.starts_with(kArtModuleJavalib) &&
         location.find("/../") == std::string_view::npos;
}

}  // namespace

void* OpenRuntimeModuleJniLibrary(const char* soname, const char* caller_location,
                                  const char* android_filesystem_root) {
  if (soname == nullptr || caller_location == nullptr) return nullptr;
  bool module_library = false;
  for (std::string_view library : kArtModuleJniLibraries) module_library |= library == soname;
  if (!module_library || !IsArtModuleCaller(caller_location, android_filesystem_root)) {
    return nullptr;
  }
  Dl_info image{};
  if (dladdr(reinterpret_cast<const void*>(&OpenRuntimeModuleJniLibrary), &image) == 0 ||
      image.dli_fname == nullptr) {
    return nullptr;
  }
  return dlopen(image.dli_fname, RTLD_NOW | RTLD_NOLOAD);
}

}  // namespace darwin_art::loader
