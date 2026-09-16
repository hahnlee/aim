#include "system_native_images.h"
#include "darwin_art_dso_namespace.h"

namespace darwin_art::loader {
std::unique_ptr<SystemNativeImages> SystemNativeImages::Create(
    const std::string& android_unwind_path, std::string* error) try {
  if (error) error->clear();
  if (android_unwind_path.empty() || android_unwind_path.front() != '/') {
    if (error) *error = "installed Android unwind artifact requires an absolute path";
    return nullptr;
  }
  auto images = std::unique_ptr<SystemNativeImages>(new SystemNativeImages());
  images->providers_ = SharedBionicProviders(CreateBionicProviderSet(error));
  if (!images->providers_) return nullptr;
  auto unwind = LoadAndroidUnwindImage(android_unwind_path.c_str(), images->providers_, error);
  if (!unwind) return nullptr;
  using Backend = NativeProviderPlacement::Backend;
  // These identities describe actual ported implementations in this runtime.
  // They do not add namespace links, exempt lists, or successful placeholders
  // for missing Android libraries. Publication verifies each concrete owner.
  images->placements_ = {
      {"default", "libc.so", "/apex/com.android.runtime/lib64/bionic/libc.so", unwind},
      {"default", "libdl.so", "/apex/com.android.runtime/lib64/bionic/libdl.so", {}},
      {"default", "ld-android.so", "/apex/com.android.runtime/bin/linker64", {}, Backend::Linker},
      {"default", "libm.so", "/apex/com.android.runtime/lib64/bionic/libm.so", {}},
      {"default", "liblog.so", "/system/lib64/liblog.so", {}},
      {"default", "libandroid.so", "/system/lib64/libandroid.so", {}},
      {"default", "libaaudio.so", "/system/lib64/libaaudio.so", {}},
      {"default", "libEGL.so", "/system/lib64/libEGL.so", {}, Backend::Angle},
      {"default", "libGLESv2.so", "/system/lib64/libGLESv2.so", {}, Backend::Angle},
      {"default", "libGLESv3.so", "/system/lib64/libGLESv3.so", {}, Backend::Angle},
      {"default", "libGLESv1_CM.so", "/system/lib64/libGLESv1_CM.so", {}, Backend::Angle},
      {"default", "libnativewindow.so", "/system/lib64/libnativewindow.so", {}, Backend::NativeWindow},
      {"default", "libjnigraphics.so", "/system/lib64/libjnigraphics.so", {}, Backend::GraphicsNdk},
  };
  // Publish only the actual resident backend, never the legacy no-driver PFNs.
  // Missing backend does not invent a library or waive original preload checks.
  if (darwin_art_bionic_vulkan_provider_ready() == 1) {
    images->placements_.push_back({"default", "libvulkan.so",
        "/system/lib64/libvulkan.so", {}, Backend::Vulkan});
  }
  return images;
} catch (...) {
  return nullptr;
}
}
