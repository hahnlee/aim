#include "namespace_admission.h"
#include "angle_image.h"
#include "native_window_image.h"
#include "vulkan_image.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include <memory>

namespace aim::loader {
namespace {
void Release(void* value) { aim_linker_image_release(static_cast<LinkerImageLease*>(value)); }
}
int AdmitNamespaceDependency(void* opaque, uint64_t parent, const char* needed_by,
    const char* name, const char* runpath, AimElfAdmission* output) {
  if (!opaque || !output) return -1;
  *output = {0, -1, 0, nullptr, nullptr};
  try {
    auto& state = *static_cast<NamespaceAdmission*>(opaque);
    LinkerImageLease* origin_raw = nullptr;
    const auto origin_status = aim_linker_discovery_resident_image(state.files, parent, &origin_raw);
    std::unique_ptr<LinkerImageLease, decltype(&aim_linker_image_release)> origin(
        origin_raw, aim_linker_image_release);
    if (origin_status < 0) return -1;
    LinkerImageLease* raw = nullptr;
    uint64_t resident_id = 0;
    const auto status = aim_linker_discovery_admit_resident(state.files, parent, name, &raw, &resident_id);
    std::unique_ptr<LinkerImageLease, decltype(&aim_linker_image_release)> lease(
        raw, aim_linker_image_release);
    if (status == 0) {
      void* parent_elf = nullptr;
      if (origin && aim_linker_image_typed_payload(origin.get(), AIM_IMAGE_ELF_SELECTED, &parent_elf) == 0) {
        void* dependency_elf = nullptr;
        void* original_native = nullptr;
        int32_t same = 0;
        if (aim_elf_selected_native_dependency(static_cast<AimElfSelectedImage*>(parent_elf),
                name, &original_native, nullptr) == AIM_ELF_OK) {
          if (aim_linker_image_same(static_cast<LinkerImageLease*>(original_native), raw, &same) != 0 || !same)
            return -1;
        } else {
          AimElfSelectedImage* original_raw = nullptr;
          if (aim_elf_selected_dependency_image(
              static_cast<AimElfSelectedImage*>(parent_elf), name,
              &original_raw, nullptr) != AIM_ELF_OK) return -1;
          std::unique_ptr<AimElfSelectedImage, decltype(&aim_elf_selected_image_release)>
              original(original_raw, aim_elf_selected_image_release);
          if (aim_linker_image_typed_payload(raw, AIM_IMAGE_ELF_SELECTED, &dependency_elf) != 0 ||
              aim_elf_selected_image_same(original.get(),
                  static_cast<AimElfSelectedImage*>(dependency_elf), &same, nullptr) != AIM_ELF_OK || !same)
            return -1;
        }
      }
      // Only concrete image owners with a typed resolver may be admitted.
      void* payload = nullptr;
      if (aim_linker_image_typed_payload(raw, AIM_IMAGE_BIONIC_PROVIDER, &payload) != 0 &&
          aim_linker_image_typed_payload(raw, AIM_IMAGE_ELF_SELECTED, &payload) != 0 &&
          !IsAngleImage(raw) && !IsNativeWindowImage(raw) && !IsVulkanImage(raw) &&
          !IsGraphicsNdkImage(raw) && !IsLinkerImage(raw))
        return -1;
      *output = {2, -1, resident_id, lease.release(), Release};
      return 0;
    }
    if (status != 1) return -1;
    // An already-loaded parent's dependency may not be rebound to a new file.
    if (origin) return -1;
    uint64_t id = 0;
    const int fd = aim_linker_discovery_open(state.files, parent, needed_by, name, runpath, &id);
    if (fd < 0) return -1;
    *output = {1, fd, id, nullptr, nullptr};
    return 0;
  } catch (...) {
    return -1; // No C++ exception crosses Rust/C; private leases unwind here.
  }
}
}
