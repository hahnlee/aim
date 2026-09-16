#include "namespace_startup.h"
#include "angle_image.h"
#include "vulkan_image.h"
#include "native_window_image.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include <cstring>

namespace darwin_art::loader {
std::unique_ptr<NamespaceHandles> CreateNamespaceRuntime(
    const std::string& config_path, const std::string& executable_path,
    bool asan, bool hwasan, const std::string& library_path,
    const SharedBionicProviders& providers,
    std::span<const NativeProviderPlacement> placements, std::string* error) {
  auto configured = ConfiguredNamespaces::Read(config_path, executable_path,
      asan, hwasan, library_path, error);
  if (!configured) return nullptr;
  for (const auto& placement : placements) {
    const auto id = placement.namespace_name
        ? configured->Find(placement.namespace_name) : 0;
    if (!id) {
      if (error) *error = "native provider placement has no configured namespace";
      return nullptr;
    }
    if (placement.backend == NativeProviderPlacement::Backend::Angle) {
      if (!PublishAngleImage(configured->registry(), id, placement.soname,
          placement.canonical_path, error))
        return nullptr;
    } else if (placement.backend == NativeProviderPlacement::Backend::NativeWindow) {
      if (!PublishNativeWindowImage(configured->registry(), id, placement.soname,
          placement.canonical_path, error)) return nullptr;
    } else if (placement.backend == NativeProviderPlacement::Backend::Linker) {
      if (!PublishLinkerImage(configured->registry(), id, placement.soname,
          placement.canonical_path, error)) return nullptr;
    } else if (placement.backend == NativeProviderPlacement::Backend::Bionic) {
      if (!PublishBionicProviderImage(configured->registry(), id,
          placement.soname, placement.canonical_path, providers, error, placement.unwind))
        return nullptr;
    } else if (placement.backend == NativeProviderPlacement::Backend::Vulkan) {
      if (!PublishVulkanImage(configured->registry(), id, placement.soname,
          placement.canonical_path, error))
        return nullptr;
    } else if (placement.backend == NativeProviderPlacement::Backend::GraphicsNdk) {
      if (!PublishGraphicsNdkImage(configured->registry(), id, placement.soname,
          placement.canonical_path, error)) return nullptr;
    } else {
      if (error) *error = "unknown native provider backend";
      return nullptr;
    }
  }
  return std::make_unique<NamespaceHandles>(std::move(configured));
}
}
