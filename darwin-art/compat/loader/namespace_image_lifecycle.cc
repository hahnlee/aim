#include "namespace_image_lifecycle.h"
#include "image_lifecycle.h"
#include <new>

namespace darwin_art::loader {
namespace {
using Context = std::shared_ptr<ImageLifecycle>;
void* Retain(void* value) { return new (std::nothrow) Context(*static_cast<Context*>(value)); }
void Release(void* value) { delete static_cast<Context*>(value); }
int Publish(void* value, uintptr_t start, uintptr_t end) {
  return (*static_cast<Context*>(value))->Publish(start, end);
}
int Finalize(void* value, uintptr_t start, uintptr_t end) {
  return (*static_cast<Context*>(value))->Finalize(start, end);
}
}
OwnedImageLifecycle CreateNamespaceImageLifecycle(const DiscoveredGraph& discovered,
    std::string* error) try {
  if (error) error->clear();
  auto fail = [error](const char* reason) -> OwnedImageLifecycle {
    if (error) *error = reason;
    return {};
  };
  const DarwinArtElfGraphSource* sources = nullptr;
  size_t count = 0, residents = 0;
  const char* root = nullptr;
  if (!discovered ||
      darwin_art_elf_discovered_graph_sources(discovered.get(), &sources, &count, nullptr) != DARWIN_ART_ELF_OK ||
      darwin_art_elf_discovered_graph_root_soname(discovered.get(), &root, nullptr) != DARWIN_ART_ELF_OK ||
      darwin_art_elf_discovered_graph_resident_count(discovered.get(), &residents, nullptr) != DARWIN_ART_ELF_OK ||
      count != discovered.placements.size()) return fail("missing admitted lifecycle metadata");
  std::vector<const char*> paths, providers;
  for (size_t i = 0; i < count; ++i) {
    if (discovered.placements[i].soname != sources[i].soname)
      return fail("lifecycle source placement mismatch");
    paths.push_back(discovered.placements[i].canonical_path.c_str());
  }
  for (size_t i = 0; i < residents; ++i) {
    const char* name = nullptr;
    uint64_t id = 0;
    void* lease = nullptr;
    if (darwin_art_elf_discovered_graph_resident(discovered.get(), i, &name, &id, &lease, nullptr) != DARWIN_ART_ELF_OK)
      return fail("missing lifecycle resident metadata");
    providers.push_back(name);
  }
  namespace images = android::darwin_art_image_registry;
  ImageLifecycle::Registry registry(images::CreateWithPaths(root, sources, count,
      paths.data(), providers.data(), providers.size(), error), images::Destroy);
  if (!registry) return {};
  Context context = ImageLifecycle::Create(std::move(registry));
  if (!context) return fail("cannot create Bionic image lifecycle");
  DarwinArtElfLifecycleCallbacks callbacks{DARWIN_ART_ELF_ABI_VERSION, Publish, Finalize, &context};
  DarwinArtElfLifecycleOwner* owner = nullptr;
  if (darwin_art_elf_lifecycle_owner_create(&callbacks, Retain, Release, &owner, nullptr) != DARWIN_ART_ELF_OK)
    return fail("cannot retain Bionic image lifecycle");
  return OwnedImageLifecycle(owner);
} catch (...) {
  if (error) *error = "cannot allocate namespace image lifecycle";
  return {};
}
}
