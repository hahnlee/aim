#include "namespace_group_release.h"
#include "darwin_art_elf_loader.h"
#include <cstdlib>
#include <set>
namespace darwin_art::loader {
namespace {
int Finalize(void* root, void* context) {
  return darwin_art_elf_selected_group_finalize(
      static_cast<DarwinArtElfSelectedImage*>(root),
      static_cast<DarwinArtElfErrorBuffer*>(context)) == DARWIN_ART_ELF_OK ? 0 : -1;
}
}
int ReleaseNamespaceGroup(LinkerRegistry* registry, ImageLease& source,
    std::vector<ImageLease>* dependencies, std::string* error) {
  if (error) error->clear();
  auto fail = [error](const char* reason) { if (error) *error = reason; return -1; };
  if (!dependencies || !dependencies->empty() || !source) return fail("invalid group release output");
  DarwinArtLocalGroupMetadata group{};
  if (darwin_art_linker_image_group_metadata(source.get(), &group) != 0)
    return fail("unknown release group");
  std::vector<ImageLease> targets;
  std::set<uint64_t> seen;
  for (size_t index = 0;; ++index) {
    LinkerImageLease* raw = nullptr;
    const int status = darwin_art_linker_image_dependency_root(registry, source.get(), index, &raw);
    ImageLease target(raw);
    if (status == 1) break;
    if (status != 0) return fail("cannot retain original release dependency");
    DarwinArtLocalGroupMetadata metadata{};
    if (darwin_art_linker_image_group_metadata(target.get(), &metadata) != 0)
      return fail("unknown dependency release group");
    // Counter multiplicity stays in the original source edges. Only the later
    // eligibility visit is deduplicated, never the reference decrements.
    if (seen.insert(metadata.id).second) targets.push_back(std::move(target));
  }
  char text[2048]{};
  DarwinArtElfErrorBuffer detail{text, sizeof(text), 0};
  DetachedGroup* detached = nullptr;
  const int status = darwin_art_linker_group_finalize_and_detach(
      registry, source.get(), Finalize, &detail, &detached);
  if (status == 1) return 1;
  if (status == -2) std::abort(); // Poisoning may have happened after finalizers.
  if (status != 0) return fail(*text ? text : "group finalization rejected");
  // No recoverable error after destructors and registry retirement commit.
  if (darwin_art_linker_handles_retire_group(registry, group.id) != 0) std::abort();
  source.reset();
  darwin_art_linker_detached_group_destroy(detached);
  *dependencies = std::move(targets);
  return 0;
}
}
