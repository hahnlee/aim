#include "namespace_admission.h"
#include "angle_image.h"
#include "native_window_image.h"
#include "vulkan_image.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"

namespace darwin_art::loader {
int ReadNamespaceResidentDependencies(void* opaque, void* resident,
    const char* const** names, size_t* count) {
  if (!names || !count) return -1;
  *names = nullptr;
  *count = 0;
  if (!opaque || !resident) return -1;
  try {
    auto& state = *static_cast<NamespaceAdmission*>(opaque);
    auto* lease = static_cast<LinkerImageLease*>(resident);
    // Process-resident ANGLE/Vulkan backends own their Mach-O dependencies in
    // dyld. They are not Android ELF dependency edges.
    if (IsAngleImage(lease) || IsNativeWindowImage(lease) || IsVulkanImage(lease) ||
        (IsGraphicsNdkImage(lease) || IsLinkerImage(lease))) return 0;
    void* payload = nullptr;
    if (darwin_art_linker_image_typed_payload(lease, DARWIN_ART_IMAGE_BIONIC_PROVIDER, &payload) == 0)
      return 0;
    if (darwin_art_linker_image_typed_payload(lease, DARWIN_ART_IMAGE_ELF_SELECTED, &payload) != 0)
      return -1;
    auto found = state.dependencies.find(payload);
    if (found == state.dependencies.end()) {
      std::vector<const char*> needed;
      for (size_t i = 0;; ++i) {
        const char* name = nullptr;
        if (darwin_art_elf_selected_image_needed(static_cast<DarwinArtElfSelectedImage*>(payload),
                i, &name, nullptr) != DARWIN_ART_ELF_OK) return -1;
        if (!name) break;
        needed.push_back(name);
      }
      found = state.dependencies.emplace(payload, std::move(needed)).first;
    }
    *names = found->second.data();
    *count = found->second.size();
    return 0;
  } catch (...) {
    return -1;
  }
}
}
