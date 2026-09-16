#include "namespace_scope_snapshot.h"
#include <map>
namespace darwin_art::loader {
bool NamespaceScopeSnapshot::Capture(DarwinArtElfDiscoveredGraph* graph,
    LinkerDiscovery* context, LinkerRegistry* registry, uint64_t query, std::string* error) {
  auto fail = [error](const char* message) { if (error) *error = message; return false; };
  ready_ = false;
  owners_.clear(); globals_.clear();
  const DarwinArtElfGraphSource* sources = nullptr;
  size_t count = 0;
  if (darwin_art_elf_discovered_graph_sources(graph, &sources, &count, nullptr) != DARWIN_ART_ELF_OK)
    return fail("invalid scope source graph");
  std::vector<uint64_t> images;
  for (size_t i = 0; i < count; ++i) {
    uint64_t image = 0;
    if (darwin_art_elf_discovered_graph_source_image(graph, i, &image, nullptr) != DARWIN_ART_ELF_OK)
      return fail("missing source identity");
    images.push_back(image);
  }
  if (darwin_art_elf_discovered_graph_resident_count(graph, &count, nullptr) != DARWIN_ART_ELF_OK)
    return fail("invalid scope residents");
  for (size_t i = 0; i < count; ++i) {
    const char* name = nullptr; uint64_t image = 0; void* lease = nullptr;
    if (darwin_art_elf_discovered_graph_resident(graph, i, &name, &image, &lease, nullptr) != DARWIN_ART_ELF_OK)
      return fail("missing resident identity");
    images.push_back(image);
  }
  std::vector<DarwinArtElfImagePlacement> placements;
  struct Scope { std::vector<uint64_t> visible; std::vector<size_t> globals; };
  std::map<uint64_t, Scope> scopes;
  for (auto image : images) {
    uint64_t primary = 0; uint8_t visible = 0;
    if (darwin_art_linker_discovery_image_scope(context, image, query, &primary, &visible) != 0)
      return fail("invalid original image scope");
    placements.push_back({image, primary});
    scopes.try_emplace(primary);
  }
  for (auto& [id, scope] : scopes) {
    for (auto image : images) {
      uint64_t primary = 0; uint8_t visible = 0;
      if (darwin_art_linker_discovery_image_scope(context, image, id, &primary, &visible) != 0)
        return fail("cannot inspect namespace visibility");
      if (visible) scope.visible.push_back(image);
    }
    auto group = std::make_unique<NamespaceElfGroup>();
    if (!group->Capture(registry, id, error)) return false;
    for (size_t i = 0; i < group->size(); ++i) {
      scope.globals.push_back(globals_.size());
      globals_.push_back(group->data()[i]);
    }
    owners_.push_back(std::move(group));
  }
  std::vector<DarwinArtElfNamespaceScope> records;
  for (auto& [id, scope] : scopes)
    records.push_back({id, scope.visible.data(), scope.visible.size(), scope.globals.data(), scope.globals.size()});
  char detail[2048]{};
  DarwinArtElfErrorBuffer buffer{detail, sizeof(detail), 0};
  if (darwin_art_elf_discovered_graph_set_namespace_scopes(graph, placements.data(), placements.size(),
      records.data(), records.size(), globals_.size(), &buffer) != DARWIN_ART_ELF_OK)
    return fail(detail);
  ready_ = true;
  return true;
}
}
