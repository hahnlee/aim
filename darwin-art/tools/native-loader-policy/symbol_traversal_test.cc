#include "loader/namespace_symbol_lookup.h"
#include "loader/namespace_elf_group.h"
#include "loader/namespace_operation.h"
#include <cassert>
#include <cstdio>
#include <fstream>
#include <iterator>

void TestSymbolTraversal(const char* directory) {
  using namespace darwin_art::loader;
  const char* names[] = {"libsymbol-root.so", "libsymbol-left.so", "libsymbol-right.so", "libsymbol-deep.so"};
  std::vector<std::vector<uint8_t>> bytes;
  for (auto* name : names) {
    std::ifstream stream(std::string(directory) + "/" + name, std::ios::binary);
    assert(stream.good());
    bytes.emplace_back(std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>());
  }
  DarwinArtElfGraphSource sources[4];
  for (size_t i = 0; i < 4; ++i) sources[i] = {names[i], bytes[i].data(), bytes[i].size()};
  DarwinArtElfGraphHandle* graph = nullptr;
  assert(darwin_art_elf_graph_load(names[0], sources, 4, nullptr, 0, nullptr, &graph, nullptr) == DARWIN_ART_ELF_OK);
  for (bool isolated : {false, true}) {
    auto* registry = darwin_art_linker_registry_create();
    uint64_t app = 0, foreign = 0;
    assert(darwin_art_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &app) == 0);
    assert(darwin_art_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &foreign) == 0);
    std::string error;
    for (size_t i = 0; i < 4; ++i) {
      DarwinArtElfSelectedImage* selected = nullptr;
      assert(darwin_art_elf_select_image(graph, names[i], &selected, nullptr) == DARWIN_ART_ELF_OK);
      const auto primary = isolated && (i == 1 || i == 3) ? foreign : app;
      assert(PublishNamespaceElf(registry, primary, (std::string("/") + names[i]).c_str(), selected, false, &error));
      darwin_art_elf_selected_image_release(selected);
    }
    {
      NamespaceOperation operation(registry);
      assert(operation);
      LinkerImageLease* root = nullptr;
      assert(darwin_art_linker_namespace_find(registry, app, names[0], &root) == 0);
      LinkerImageLease* right = nullptr;
      assert(darwin_art_linker_namespace_find(registry, app, names[2], &right) == 0);
      LinkerImageLease* winner_defining = nullptr;
      auto winner = LookupNamespaceSymbol(registry, root, "winner", &error, nullptr,
          &winner_defining);
      assert(winner && error.empty());
      assert(reinterpret_cast<int(*)()>(winner)() == 22); // BFS, not DFS's 33.
      int32_t same = 0;
      assert(winner_defining);
      assert(darwin_art_linker_image_same(winner_defining, right, &same) == 0 && same);
      darwin_art_linker_image_release(winner_defining);
      auto left = LookupNamespaceSymbol(registry, root, "left_only", &error);
      assert(left && reinterpret_cast<int(*)()>(left)() == 11); // Direct parent grants access.
      auto deep = LookupNamespaceSymbol(registry, root, "deep_only", &error);
      if (isolated) assert(!deep && !error.empty()); // No transitive visibility.
      else assert(deep && reinterpret_cast<int(*)()>(deep)() == 33);
      LinkerImageLease* miss_defining = darwin_art_linker_image_clone(right);
      assert(miss_defining);
      LinkerImageLease* old_miss_defining = miss_defining;
      assert(!LookupNamespaceSymbol(registry, root, "absent_symbol_for_graph_test_2", &error,
          nullptr, &miss_defining));
      assert(!miss_defining);
      darwin_art_linker_image_release(old_miss_defining);
      LinkerImageLease* after_root_defining = nullptr;
      auto after_root = LookupNamespaceSymbolAfter(registry, root, root, "winner", &error,
          nullptr, &after_root_defining);
      assert(after_root && reinterpret_cast<int(*)()>(after_root)() == 22);
      assert(after_root_defining);
      assert(darwin_art_linker_image_same(after_root_defining, right, &same) == 0 && same);
      darwin_art_linker_image_release(after_root_defining);
      LinkerImageLease* after_right_defining = darwin_art_linker_image_clone(right);
      assert(after_right_defining);
      LinkerImageLease* old_after_right_defining = after_right_defining;
      auto after_right = LookupNamespaceSymbolAfter(registry, root, right, "winner", &error,
          nullptr, &after_right_defining);
      if (isolated) {
        assert(!after_right && !error.empty());
        assert(!after_right_defining);
      } else {
        assert(after_right && reinterpret_cast<int(*)()>(after_right)() == 33);
        assert(after_right_defining);
        darwin_art_linker_image_release(after_right_defining);
      }
      // Lookup clears the caller-provided slot without freeing its prior
      // lease, then returns a fresh defining-image clone on success.
      darwin_art_linker_image_release(old_after_right_defining);
      // Skipping the final node must not accidentally restart at the root.
      LinkerImageLease* last = nullptr;
      assert(darwin_art_linker_namespace_find(registry, isolated ? foreign : app,
          names[3], &last) == 0);
      assert(!LookupNamespaceSymbolAfter(registry, root, last, "winner", &error));
      assert(!error.empty());
      darwin_art_linker_image_release(last);
      darwin_art_linker_image_release(right);
      darwin_art_linker_image_release(root);
    }
    darwin_art_linker_registry_destroy(registry);
  }
  assert(darwin_art_elf_graph_unload(&graph, nullptr) == DARWIN_ART_ELF_OK);
  std::puts("ELF symbol BFS: sibling order, access pruning, NEXT skip-through-caller PASS");
}
