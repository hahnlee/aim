#pragma once
#include "bionic_provider_image.h"
#include "namespace_handles.h"
#include <span>

namespace darwin_art::loader {
// Trusted runtime installation identities, not an app-supplied SONAME allowlist.
// Each entry must be backed by its concrete owner: sealed Bionic providers or
// the process-resident ANGLE/Vulkan dispatch backends. No empty image
// placeholders.
struct NativeProviderPlacement {
  const char* namespace_name;
  const char* soname;
  const char* canonical_path;
  SharedAndroidUnwind unwind;
  enum class Backend { Bionic, Angle, NativeWindow, Linker, Vulkan, GraphicsNdk } backend = Backend::Bionic;
};
// Build privately, publish actual providers, then expose the complete owner.
// Failure destroys the partial registry; never mutates a live namespace set.
std::unique_ptr<NamespaceHandles> CreateNamespaceRuntime(
    const std::string& config_path, const std::string& executable_path,
    bool asan, bool hwasan, const std::string& library_path,
    const SharedBionicProviders&, std::span<const NativeProviderPlacement>,
    std::string* error);
}
