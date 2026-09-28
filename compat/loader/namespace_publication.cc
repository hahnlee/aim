#include "namespace_load.h"
#include "namespace_dependency_groups.h"
#include "namespace_operation.h"

namespace aim::loader {
namespace {
void Release(void* image) {
  aim_elf_selected_image_release(static_cast<AimElfSelectedImage*>(image));
}
void* Retain(void* image) {
  AimElfSelectedImage* copy = nullptr;
  if (aim_elf_selected_image_clone(static_cast<AimElfSelectedImage*>(image),
      &copy, nullptr) != AIM_ELF_OK) return nullptr;
  return copy;
}
}

bool NamespaceHandles::Publish(const LoadedGraph& loaded, bool global, std::string* error,
    LinkerPublication** token) {
  if (token) *token = nullptr;
  if (error) error->clear();
  auto fail = [error](const char* message) {
    if (error) *error = message;
    return false;
  };
  if (!loaded || loaded.placements.empty()) return fail("missing admitted loaded graph");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter SDK publication operation");
  const int target_sdk = TargetSdkVersion();
  using Image = std::unique_ptr<AimElfSelectedImage, decltype(&Release)>;
  std::vector<Image> images;
  std::vector<AimImagePublication> records;
  std::vector<AimLocalGroupMetadata> groups;
  for (const auto& placement : loaded.placements) {
    AimElfSelectedImage* raw = nullptr;
    if (aim_elf_select_image(loaded.get(), placement.soname.c_str(),
        &raw, nullptr) != AIM_ELF_OK) return fail("missing mapped graph member");
    Image image(raw, Release);
    AimElfGlobalSource source{};
    uint64_t flags = 0;
    AimLocalGroupMetadata group{};
    if (aim_elf_selected_group_info(raw, &group.id, &group.is_root, nullptr)
        != AIM_ELF_OK) return fail("missing original local-group identity");
    if (aim_elf_selected_image_source(raw, &source, &flags, nullptr) != AIM_ELF_OK)
      return fail("invalid mapped graph metadata");
    records.push_back({placement.namespace_id, source.soname,
        placement.canonical_path.c_str(), raw, Retain, Release, flags,
        static_cast<uint8_t>(global), AIM_IMAGE_ELF_SELECTED,
        placement.file.device, placement.file.inode, placement.file.offset});
    images.push_back(std::move(image));
    groups.push_back(group);
  }
  // Rust owns atomic publication and callback rollback. No handle-table lock
  // is held while image retain/release callbacks run.
  std::vector<AimGroupDependency> dependencies;
  for (size_t index = 0; index < images.size(); ++index)
    if (!CollectGroupDependencies(index, images[index].get(), &dependencies, error)) return false;
  const int status = aim_linker_namespace_publish_linked_groups_for_sdk(configured_->registry(),
      records.data(), groups.data(), records.size(), dependencies.data(), dependencies.size(), token,
      target_sdk);
  if (status != 0) return fail("loaded graph publication failed");
  return true;
}
}
