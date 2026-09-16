#pragma once
#include "darwin_art_elf_loader.h"
#include "darwin_art_linker_namespace.h"
#include "linker_image.h"
#include "graphics_ndk_image.h"
#include "vulkan_image.h"
#include <memory>
#include <string>
#include <vector>

namespace darwin_art::loader {
// Snapshot actual original ELF edges, never re-resolve names in a namespace.
// Bionic providers retain their separate process-native resource ownership.
inline bool CollectGroupDependencies(size_t index, DarwinArtElfSelectedImage* parent,
    std::vector<DarwinArtGroupDependency>* edges, std::string* error) {
  using Image = std::unique_ptr<DarwinArtElfSelectedImage, decltype(&darwin_art_elf_selected_image_release)>;
  std::vector<Image> seen;
  auto fail = [error]() { if (error) *error = "original dependency group unavailable"; return false; };
  for (size_t i = 0;; ++i) {
    const char* name = nullptr;
    if (darwin_art_elf_selected_image_needed(parent, i, &name, nullptr) != DARWIN_ART_ELF_OK) return fail();
    if (!name) return true;
    DarwinArtElfSelectedImage* raw = nullptr;
    void* native = nullptr;
    if (darwin_art_elf_selected_native_dependency(parent, name, &native, nullptr) == DARWIN_ART_ELF_OK) {
      void* payload = nullptr;
      auto* original = static_cast<LinkerImageLease*>(native);
      // The loader dispatch image is process-resident, not an independently
      // unloadable ELF local group. Its native image lease is already retained.
      if (IsVulkanImage(original) || (IsGraphicsNdkImage(original) || IsLinkerImage(original))) continue;
      if (darwin_art_linker_image_typed_payload(original, DARWIN_ART_IMAGE_BIONIC_PROVIDER, &payload) == 0)
        continue;
      if (darwin_art_linker_image_typed_payload(original, DARWIN_ART_IMAGE_ELF_SELECTED, &payload) != 0 ||
          darwin_art_elf_selected_image_clone(static_cast<DarwinArtElfSelectedImage*>(payload), &raw, nullptr)
              != DARWIN_ART_ELF_OK) return fail();
    } else if (darwin_art_elf_selected_dependency_image(parent, name, &raw, nullptr) != DARWIN_ART_ELF_OK)
      return fail();
    Image dependency(raw, darwin_art_elf_selected_image_release);
    bool duplicate = false;
    for (const auto& previous : seen) {
      int32_t same = 0;
      if (darwin_art_elf_selected_image_same(previous.get(), raw, &same, nullptr) != DARWIN_ART_ELF_OK) return fail();
      if (same) { duplicate = true; break; }
    }
    if (duplicate) continue;
    uint64_t group = 0;
    uint8_t root = 0;
    if (darwin_art_elf_selected_group_info(raw, &group, &root, nullptr) != DARWIN_ART_ELF_OK) return fail();
    edges->push_back({index, group});
    seen.push_back(std::move(dependency));
  }
}
}
