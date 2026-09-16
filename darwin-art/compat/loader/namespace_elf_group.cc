#include "namespace_elf_group.h"
#include <array>

namespace darwin_art::loader {
namespace {
void* Retain(void* source) {
  DarwinArtElfSelectedImage* copy = nullptr;
  if (darwin_art_elf_selected_image_clone(
          static_cast<DarwinArtElfSelectedImage*>(source), &copy, nullptr)
      != DARWIN_ART_ELF_OK) return nullptr;
  return copy;
}
void Release(void* source) {
  darwin_art_elf_selected_image_release(
      static_cast<DarwinArtElfSelectedImage*>(source));
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
      darwin_art_linker_image_typed_payload(lease, DARWIN_ART_IMAGE_ELF_SELECTED, &payload) != 0) {
    Fail(error, "invalid selected ELF symbol request");
    return -1;
  }
  std::array<char, 1024> storage{};
  DarwinArtElfErrorBuffer detail{storage.data(), storage.size(), 0};
  if (darwin_art_elf_selected_image_lookup_android(static_cast<DarwinArtElfSelectedImage*>(payload),
          symbol, version, address, &detail) != DARWIN_ART_ELF_OK) {
    Fail(error, storage.data());
    return -1;
  }
  return *address ? 0 : 1;
}

bool PublishNamespaceElf(LinkerRegistry* registry, uint64_t id,
    const char* path, DarwinArtElfSelectedImage* image, bool global,
    std::string* error) {
  DarwinArtElfGlobalSource source{};
  uint64_t flags = 0;
  std::array<char, 1024> storage{};
  DarwinArtElfErrorBuffer buffer{storage.data(), storage.size(), 0};
  if (darwin_art_elf_selected_image_source(image, &source, &flags, &buffer)
      != DARWIN_ART_ELF_OK) return Fail(error, storage.data());
  if (darwin_art_linker_namespace_publish_image(registry, id, source.soname,
          path, image, Retain, Release, flags, global ? 1 : 0,
          DARWIN_ART_IMAGE_ELF_SELECTED) != 0)
    return Fail(error, "ELF namespace publication failed");
  return true;
}

bool NamespaceElfGroup::Capture(LinkerRegistry* registry, uint64_t id,
                                std::string* error) {
  // Failure must not leave a previous successful group available for loading.
  sources_.clear();
  group_.reset();
  LinkerImageGroup* raw = nullptr;
  if (darwin_art_linker_group_snapshot(registry, id, 1, &raw) != 0)
    return Fail(error, "ELF namespace global snapshot failed");
  std::unique_ptr<LinkerImageGroup, Drop> group(raw);
  std::vector<DarwinArtElfGlobalSource> sources;
  for (size_t index = 0;; ++index) {
    void* payload = nullptr;
    const int status = darwin_art_linker_group_typed_item(
        raw, index, DARWIN_ART_IMAGE_ELF_SELECTED, &payload);
    if (status == 1) break;
    // A native provider needs an explicit resolver contract. Never omit it or
    // reinterpret a Mach-O handle as an ELF selected-image pointer.
    if (status != 0)
      return Fail(error, "global group contains a non-ELF representation");
    DarwinArtElfGlobalSource source{};
    uint64_t flags = 0;
    std::array<char, 1024> storage{};
    DarwinArtElfErrorBuffer buffer{storage.data(), storage.size(), 0};
    if (darwin_art_elf_selected_image_source(
            static_cast<DarwinArtElfSelectedImage*>(payload), &source, &flags,
            &buffer) != DARWIN_ART_ELF_OK)
      return Fail(error, storage.data());
    sources.push_back(source);
  }
  group_ = std::move(group);
  sources_ = std::move(sources);
  return true;
}
}  // namespace darwin_art::loader
