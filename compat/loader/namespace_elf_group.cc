#include "namespace_elf_group.h"
#include <array>

namespace aim::loader {
namespace {
void* Retain(void* source) {
  AimElfSelectedImage* copy = nullptr;
  if (aim_elf_selected_image_clone(
          static_cast<AimElfSelectedImage*>(source), &copy, nullptr)
      != AIM_ELF_OK) return nullptr;
  return copy;
}
void Release(void* source) {
  aim_elf_selected_image_release(
      static_cast<AimElfSelectedImage*>(source));
}
bool Fail(std::string* error, const char* text) {
  if (error) *error = text;
  return false;
}
}  // namespace

int ResolveNamespaceElf(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* address, std::string* error) {
  if (error) error->clear();
  if (address) *address = 0;
  void* payload = nullptr;
  if (!address || !symbol || !*symbol ||
      aim_linker_image_typed_payload(lease, AIM_IMAGE_ELF_SELECTED, &payload) != 0) {
    Fail(error, "invalid selected ELF symbol request");
    return -1;
  }
  std::array<char, 1024> storage{};
  AimElfErrorBuffer detail{storage.data(), storage.size(), 0};
  if (aim_elf_selected_image_lookup_android(static_cast<AimElfSelectedImage*>(payload),
          symbol, version, address, &detail) != AIM_ELF_OK) {
    Fail(error, storage.data());
    return -1;
  }
  return *address ? 0 : 1;
}

bool PublishNamespaceElf(LinkerRegistry* registry, uint64_t id,
    const char* path, AimElfSelectedImage* image, bool global,
    std::string* error) {
  AimElfGlobalSource source{};
  uint64_t flags = 0;
  std::array<char, 1024> storage{};
  AimElfErrorBuffer buffer{storage.data(), storage.size(), 0};
  if (aim_elf_selected_image_source(image, &source, &flags, &buffer)
      != AIM_ELF_OK) return Fail(error, storage.data());
  if (aim_linker_namespace_publish_image(registry, id, source.soname,
          path, image, Retain, Release, flags, global ? 1 : 0,
          AIM_IMAGE_ELF_SELECTED) != 0)
    return Fail(error, "ELF namespace publication failed");
  return true;
}

bool NamespaceElfGroup::Capture(LinkerRegistry* registry, uint64_t id,
                                std::string* error) {
  // Failure must not leave a previous successful group available for loading.
  sources_.clear();
  group_.reset();
  LinkerImageGroup* raw = nullptr;
  if (aim_linker_group_snapshot(registry, id, 1, &raw) != 0)
    return Fail(error, "ELF namespace global snapshot failed");
  std::unique_ptr<LinkerImageGroup, Drop> group(raw);
  std::vector<AimElfGlobalSource> sources;
  for (size_t index = 0;; ++index) {
    void* payload = nullptr;
    const int status = aim_linker_group_typed_item(
        raw, index, AIM_IMAGE_ELF_SELECTED, &payload);
    if (status == 1) break;
    // A native provider needs an explicit resolver contract. Never omit it or
    // reinterpret a Mach-O handle as an ELF selected-image pointer.
    if (status != 0)
      return Fail(error, "global group contains a non-ELF representation");
    AimElfGlobalSource source{};
    uint64_t flags = 0;
    std::array<char, 1024> storage{};
    AimElfErrorBuffer buffer{storage.data(), storage.size(), 0};
    if (aim_elf_selected_image_source(
            static_cast<AimElfSelectedImage*>(payload), &source, &flags,
            &buffer) != AIM_ELF_OK)
      return Fail(error, storage.data());
    sources.push_back(source);
  }
  group_ = std::move(group);
  sources_ = std::move(sources);
  return true;
}
}  // namespace aim::loader
