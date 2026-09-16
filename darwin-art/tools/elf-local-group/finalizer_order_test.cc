#include "darwin_art_elf_loader.h"
#include "darwin_art_linker_namespace.h"
#include "loader/namespace_group_release.h"
#include "loader/namespace_close.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>
static std::vector<int> finalized;
static LinkerRegistry* registry;
static uint64_t namespace_id;
static DarwinArtElfSelectedImage* active_root;
static void Record(int node) {
  finalized.push_back(node);
  LinkerImageLease* lookup = nullptr;
  assert(darwin_art_linker_namespace_find(registry, namespace_id, "liborder0.so", &lookup) == 0);
  LinkerImageLease* opened = nullptr;
  assert(darwin_art_linker_image_acquire_open(lookup, &opened) != 0 && !opened);
  darwin_art_linker_image_release(lookup);
  if (node == 0) {
    assert(darwin_art_elf_selected_group_finalize(active_root, nullptr) == DARWIN_ART_ELF_OK);
    assert(finalized.size() == 1); // Reentry cannot finalize children early.
  }
}
static void* Retain(void* value) {
  DarwinArtElfSelectedImage* copy = nullptr;
  assert(darwin_art_elf_selected_image_clone(static_cast<DarwinArtElfSelectedImage*>(value), &copy, nullptr) == DARWIN_ART_ELF_OK);
  return copy;
}
static void Release(void* value) { darwin_art_elf_selected_image_release(static_cast<DarwinArtElfSelectedImage*>(value)); }
static DarwinArtElfResolveStatus Resolve(void*, const DarwinArtElfSymbolRequest* request,
    uintptr_t* address, DarwinArtElfErrorBuffer*) {
  *address = 0;
  if (std::strcmp(request->symbol, "record_finalizer") != 0) return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
  *address = reinterpret_cast<uintptr_t>(Record);
  return DARWIN_ART_ELF_RESOLVE_FOUND;
}
int main(int argc, char** argv) {
  assert(argc == 5);
  const char* names[] = {"liborder0.so", "liborder1.so", "liborder2.so", "liborder3.so"};
  std::vector<uint8_t> bytes[4];
  DarwinArtElfGraphSource sources[4]{};
  for (int i = 0; i < 4; ++i) {
    std::ifstream stream(argv[i + 1], std::ios::binary);
    assert(stream.good());
    bytes[i] = {std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>()};
    sources[i] = {names[i], bytes[i].data(), bytes[i].size()};
  }
  DarwinArtElfGraphHandle* graph = nullptr;
  DarwinArtElfLoadOptions options{DARWIN_ART_ELF_ABI_VERSION, Resolve, nullptr};
  char detail[2048]{};
  DarwinArtElfErrorBuffer error{detail, sizeof(detail), 0};
  const char* providers[] = {"liborder-provider.so"};
  auto status = darwin_art_elf_graph_load(names[0], sources, 4, providers, 1, &options, &graph, &error);
  if (status != DARWIN_ART_ELF_OK) std::fprintf(stderr, "%s\n", detail);
  assert(status == DARWIN_ART_ELF_OK);
  DarwinArtElfSelectedImage* root = nullptr;
  assert(darwin_art_elf_select_image(graph, names[0], &root, &error) == DARWIN_ART_ELF_OK);
  active_root = root;
  registry = darwin_art_linker_registry_create();
  auto* operation = darwin_art_linker_operation_enter(registry);
  assert(operation);
  assert(darwin_art_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &namespace_id) == 0);
  DarwinArtElfSelectedImage* images[4]{};
  DarwinArtImagePublication records[4]{};
  DarwinArtLocalGroupMetadata groups[4]{};
  for (int i = 0; i < 4; ++i) {
    assert(darwin_art_elf_select_image(graph, names[i], &images[i], &error) == DARWIN_ART_ELF_OK);
    assert(darwin_art_elf_selected_group_info(images[i], &groups[i].id, &groups[i].is_root, &error) == DARWIN_ART_ELF_OK);
    records[i] = {namespace_id, names[i], "/fixture.so", images[i], Retain, Release, 0, 0,
        DARWIN_ART_IMAGE_ELF_SELECTED, 0, 0, 0};
    assert(groups[i].id == groups[0].id);
  }
  DarwinArtGroupDependency edges[] = {{0, groups[0].id}, {0, groups[0].id},
      {1, groups[0].id}, {2, groups[0].id}};
  assert(darwin_art_linker_namespace_publish_linked_groups(registry, records, groups, 4,
      edges, 4, nullptr) == 0);
  for (auto* image : images) darwin_art_elf_selected_image_release(image);
  LinkerImageLease* lookup = nullptr;
  assert(darwin_art_linker_namespace_find(registry, namespace_id, names[0], &lookup) == 0);
  assert(finalized.empty());
  std::vector<darwin_art::loader::ImageLease> dependencies;
  std::string release_error;
  LinkerImageLease* opened = nullptr;
  assert(darwin_art_linker_image_acquire_open(lookup, &opened) == 0);
  uintptr_t handle = 0;
  assert(darwin_art_linker_handle_adopt(registry, &opened, &handle) == 0 && !opened && handle);
  darwin_art::loader::ImageLease owned_lookup(lookup);
  assert(darwin_art::loader::ReleaseNamespaceGroup(registry, owned_lookup, &dependencies, &release_error) == 1);
  assert(finalized.empty() && dependencies.empty() && owned_lookup);
  owned_lookup.reset();
  lookup = nullptr;
  assert(darwin_art::loader::CloseNamespaceLibrary(registry, handle, &release_error) == 0);
  assert(darwin_art::loader::CloseNamespaceLibrary(registry, handle, &release_error) == -1);
  assert(!owned_lookup && dependencies.empty());
  LinkerImageLease* stale = nullptr;
  assert(darwin_art_linker_handle_image(registry, handle, &stale) != 0 && !stale);
  assert((finalized == std::vector<int>{0, 1, 2, 3}));
  LinkerImageLease* absent = nullptr;
  assert(darwin_art_linker_namespace_find(registry, namespace_id, names[0], &absent) == 1 && !absent);
  assert(darwin_art_linker_operation_leave(operation) == 0);
  darwin_art_linker_registry_destroy(registry);
  // Group once guard also covers later automatic mapping release.
  assert(darwin_art_elf_selected_group_finalize(root, &error) == DARWIN_ART_ELF_OK);
  darwin_art_elf_selected_image_release(root);
  assert(darwin_art_elf_graph_unload(&graph, &error) == DARWIN_ART_ELF_OK);
  assert((finalized == std::vector<int>{0, 1, 2, 3}));
  std::puts("actual ELF finalizers: root,left,right,shared-leaf exactly once PASS");
}
