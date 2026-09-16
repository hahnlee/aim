#include "namespace_symbol_lookup.h"
#include "namespace_group_release.h"
#include "darwin_art_elf_loader.h"
#include <algorithm>
#include <deque>
#include <vector>

namespace darwin_art::loader {
namespace {
using Elf = std::unique_ptr<DarwinArtElfSelectedImage, decltype(&darwin_art_elf_selected_image_release)>;
struct Node {
  ImageLease image;
  uint64_t primary = 0;
  bool member = false;
  std::vector<size_t> children;
};
bool Same(const LinkerImageLease* a, const LinkerImageLease* b) {
  int32_t same = 0;
  return darwin_art_linker_image_same(a, b, &same) == 0 && same;
}
}

uintptr_t LookupNamespaceSymbol(LinkerRegistry* registry, const LinkerImageLease* root,
    const char* symbol, std::string* error, const char* version, LinkerImageLease** defining_image) {
  return LookupNamespaceSymbolAfter(registry, root, nullptr, symbol, error, version, defining_image);
}

uintptr_t LookupNamespaceSymbolAfter(LinkerRegistry* registry, const LinkerImageLease* root,
    const LinkerImageLease* after, const char* symbol, std::string* error, const char* version,
    LinkerImageLease** defining_image) try {
  if (defining_image) *defining_image = nullptr;
  auto fail = [error](const char* text) -> uintptr_t { if (error) *error = text; return 0; };
  auto found = [&](const LinkerImageLease* source, uintptr_t address) -> uintptr_t {
    if (!address) return 0;
    if (defining_image) {
      *defining_image = darwin_art_linker_image_clone(source);
      if (!*defining_image) return fail("cannot retain defining symbol image");
    }
    if (error) error->clear();
    return address;
  };
  void* root_payload = nullptr;
  if (darwin_art_linker_image_typed_payload(root, DARWIN_ART_IMAGE_ELF_SELECTED, &root_payload) != 0) {
    if (after) return fail("NEXT dependency root is not an ELF graph");
    uintptr_t address = 0;
    if (ResolveNamespaceImageSymbol(root, symbol, &address, error, version) == 0) return found(root, address);
    if (error && error->empty()) *error = "Android native symbol not found";
    return 0;
  }
  uint64_t requester = 0;
  if (darwin_art_linker_image_registered_primary_namespace(registry, root, &requester) != 0)
    return fail("unregistered symbol lookup root");
  // The root belongs to its own primary namespace and is first in AOSP's
  // walk. Avoid constructing any graph when its own export already answers.
  uintptr_t root_address = 0;
  if (!after) {
    const int root_status = ResolveNamespaceImageSymbol(root, symbol, &root_address, error, version);
    if (root_status <= 0) return root_status == 0 ? found(root, root_address) : 0;
  }
  LinkerImageGroup* raw = nullptr;
  if (darwin_art_linker_registry_image_snapshot(registry, &raw) != 0)
    return fail("cannot snapshot linker images");
  std::unique_ptr<LinkerImageGroup, decltype(&darwin_art_linker_group_destroy)> snapshot(
      raw, darwin_art_linker_group_destroy);
  std::vector<Node> nodes;
  size_t root_index = SIZE_MAX;
  for (size_t i = 0;; ++i) {
    LinkerImageLease* image = nullptr;
    const int status = darwin_art_linker_group_image(raw, i, &image);
    if (status == 1) break;
    if (status != 0) return fail("cannot retain symbol graph image");
    Node node{ImageLease(image), 0, false, {}};
    uint8_t member = 0;
    if (darwin_art_linker_image_primary_namespace(image, &node.primary) != 0 ||
        darwin_art_linker_image_namespace_member(registry, requester, image, &member) != 0)
      return fail("cannot inspect symbol graph membership");
    node.member = member != 0;
    if (Same(image, root)) root_index = nodes.size();
    nodes.push_back(std::move(node));
  }
  if (root_index == SIZE_MAX) return fail("symbol root is absent from snapshot");
  // Build original image edges, not SONAME re-resolution or local-group roots.
  // All resident parents participate in AOSP's direct-primary-parent access rule.
  for (auto& node : nodes) {
    void* payload = nullptr;
    if (darwin_art_linker_image_typed_payload(node.image.get(), DARWIN_ART_IMAGE_ELF_SELECTED, &payload) != 0)
      continue;
    auto* parent = static_cast<DarwinArtElfSelectedImage*>(payload);
    for (size_t edge = 0;; ++edge) {
      const char* name = nullptr;
      if (darwin_art_elf_selected_image_needed(parent, edge, &name, nullptr) != DARWIN_ART_ELF_OK)
        return fail("cannot inspect original DT_NEEDED edge");
      if (!name) break;
      void* native = nullptr;
      DarwinArtElfSelectedImage* selected = nullptr;
      const bool native_edge = darwin_art_elf_selected_native_dependency(parent, name, &native, nullptr) == DARWIN_ART_ELF_OK;
      if (!native_edge && darwin_art_elf_selected_dependency_image(parent, name, &selected, nullptr) != DARWIN_ART_ELF_OK)
        return fail("original dependency identity unavailable");
      Elf dependency(selected, darwin_art_elf_selected_image_release);
      size_t target = SIZE_MAX;
      for (size_t candidate = 0; candidate < nodes.size(); ++candidate) {
        bool same = false;
        if (native_edge) {
          same = Same(static_cast<LinkerImageLease*>(native), nodes[candidate].image.get());
        } else {
          void* other = nullptr;
          if (darwin_art_linker_image_typed_payload(nodes[candidate].image.get(), DARWIN_ART_IMAGE_ELF_SELECTED, &other) == 0) {
            int32_t matches = 0;
            if (darwin_art_elf_selected_image_same(selected, static_cast<DarwinArtElfSelectedImage*>(other),
                &matches, nullptr) != DARWIN_ART_ELF_OK) return fail("cannot compare dependency mapping identity");
            same = matches != 0;
          }
        }
        if (same) {
          if (target != SIZE_MAX) return fail("ambiguous registered dependency identity");
          target = candidate;
        }
      }
      if (target == SIZE_MAX) return fail("original dependency is not registered");
      node.children.push_back(target);
    }
  }
  std::vector<bool> accessible(nodes.size(), false), visited(nodes.size(), false);
  for (size_t i = 0; i < nodes.size(); ++i) {
    if (nodes[i].member) accessible[i] = true;
    if (nodes[i].primary == requester)
      for (auto child : nodes[i].children) accessible[child] = true;
  }
  std::deque<size_t> pending{root_index};
  bool skipping = after != nullptr;
  while (!pending.empty()) {
    const auto index = pending.front(); pending.pop_front();
    if (visited[index]) continue;
    visited[index] = true;
    if (skipping) {
      // AOSP checks skip_until before accessibility and returns kWalkContinue:
      // skipping an inaccessible caller must not prune its children here.
      if (Same(nodes[index].image.get(), after)) skipping = false;
      for (auto child : nodes[index].children) pending.push_back(child);
      continue;
    }
    if (!accessible[index]) continue;  // AOSP kWalkSkip also prunes children.
    uintptr_t address = 0;
    const int status = ResolveNamespaceImageSymbol(nodes[index].image.get(), symbol, &address, error, version);
    if (status < 0) return 0;
    if (status == 0) return found(nodes[index].image.get(), address);
    for (auto child : nodes[index].children) pending.push_back(child);
  }
  return fail("Android symbol not found in accessible dependency graph");
} catch (...) {
  if (error) *error = "cannot allocate symbol lookup graph";
  return 0;
}
}
