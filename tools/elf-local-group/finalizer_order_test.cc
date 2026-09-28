#include "aim_elf_loader.h"
#include "aim_linker_namespace.h"
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
static AimElfSelectedImage* active_root;
static void Record(int node) {
  finalized.push_back(node);
  LinkerImageLease* lookup = nullptr;
  assert(aim_linker_namespace_find(registry, namespace_id, "liborder0.so", &lookup) == 0);
  LinkerImageLease* opened = nullptr;
  assert(aim_linker_image_acquire_open(lookup, &opened) != 0 && !opened);
  aim_linker_image_release(lookup);
  if (node == 0) {
    assert(aim_elf_selected_group_finalize(active_root, nullptr) == AIM_ELF_OK);
    assert(finalized.size() == 1); // Reentry cannot finalize children early.
  }
}
static void* Retain(void* value) {
  AimElfSelectedImage* copy = nullptr;
  assert(aim_elf_selected_image_clone(static_cast<AimElfSelectedImage*>(value), &copy, nullptr) == AIM_ELF_OK);
  return copy;
}
static void Release(void* value) { aim_elf_selected_image_release(static_cast<AimElfSelectedImage*>(value)); }
static AimElfResolveStatus Resolve(void*, const AimElfSymbolRequest* request,
    uintptr_t* address, AimElfErrorBuffer*) {
  *address = 0;
  if (std::strcmp(request->symbol, "record_finalizer") != 0) return AIM_ELF_RESOLVE_NOT_FOUND;
  *address = reinterpret_cast<uintptr_t>(Record);
  return AIM_ELF_RESOLVE_FOUND;
}
int main(int argc, char** argv) {
  assert(argc == 5);
  const char* names[] = {"liborder0.so", "liborder1.so", "liborder2.so", "liborder3.so"};
  std::vector<uint8_t> bytes[4];
  AimElfGraphSource sources[4]{};
  for (int i = 0; i < 4; ++i) {
    std::ifstream stream(argv[i + 1], std::ios::binary);
    assert(stream.good());
    bytes[i] = {std::istreambuf_iterator<char>(stream), std::istreambuf_iterator<char>()};
    sources[i] = {names[i], bytes[i].data(), bytes[i].size()};
  }
  AimElfGraphHandle* graph = nullptr;
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  char detail[2048]{};
  AimElfErrorBuffer error{detail, sizeof(detail), 0};
  const char* providers[] = {"liborder-provider.so"};
  auto status = aim_elf_graph_load(names[0], sources, 4, providers, 1, &options, &graph, &error);
  if (status != AIM_ELF_OK) std::fprintf(stderr, "%s\n", detail);
  assert(status == AIM_ELF_OK);
  AimElfSelectedImage* root = nullptr;
  assert(aim_elf_select_image(graph, names[0], &root, &error) == AIM_ELF_OK);
  active_root = root;
  registry = aim_linker_registry_create();
  auto* operation = aim_linker_operation_enter(registry);
  assert(operation);
  assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &namespace_id) == 0);
  AimElfSelectedImage* images[4]{};
  AimImagePublication records[4]{};
  AimLocalGroupMetadata groups[4]{};
  for (int i = 0; i < 4; ++i) {
    assert(aim_elf_select_image(graph, names[i], &images[i], &error) == AIM_ELF_OK);
    assert(aim_elf_selected_group_info(images[i], &groups[i].id, &groups[i].is_root, &error) == AIM_ELF_OK);
    records[i] = {namespace_id, names[i], "/fixture.so", images[i], Retain, Release, 0, 0,
        AIM_IMAGE_ELF_SELECTED, 0, 0, 0};
    assert(groups[i].id == groups[0].id);
  }
  AimGroupDependency edges[] = {{0, groups[0].id}, {0, groups[0].id},
      {1, groups[0].id}, {2, groups[0].id}};
  assert(aim_linker_namespace_publish_linked_groups(registry, records, groups, 4,
      edges, 4, nullptr) == 0);
  for (auto* image : images) aim_elf_selected_image_release(image);
  LinkerImageLease* lookup = nullptr;
  assert(aim_linker_namespace_find(registry, namespace_id, names[0], &lookup) == 0);
  assert(finalized.empty());
  std::vector<aim::loader::ImageLease> dependencies;
  std::string release_error;
  LinkerImageLease* opened = nullptr;
  assert(aim_linker_image_acquire_open(lookup, &opened) == 0);
  uintptr_t handle = 0;
  assert(aim_linker_handle_adopt(registry, &opened, &handle) == 0 && !opened && handle);
  aim::loader::ImageLease owned_lookup(lookup);
  assert(aim::loader::ReleaseNamespaceGroup(registry, owned_lookup, &dependencies, &release_error) == 1);
  assert(finalized.empty() && dependencies.empty() && owned_lookup);
  owned_lookup.reset();
  lookup = nullptr;
  assert(aim::loader::CloseNamespaceLibrary(registry, handle, &release_error) == 0);
  assert(aim::loader::CloseNamespaceLibrary(registry, handle, &release_error) == -1);
  assert(!owned_lookup && dependencies.empty());
  LinkerImageLease* stale = nullptr;
  assert(aim_linker_handle_image(registry, handle, &stale) != 0 && !stale);
  assert((finalized == std::vector<int>{0, 1, 2, 3}));
  LinkerImageLease* absent = nullptr;
  assert(aim_linker_namespace_find(registry, namespace_id, names[0], &absent) == 1 && !absent);
  assert(aim_linker_operation_leave(operation) == 0);
  aim_linker_registry_destroy(registry);
  // Group once guard also covers later automatic mapping release.
  assert(aim_elf_selected_group_finalize(root, &error) == AIM_ELF_OK);
  aim_elf_selected_image_release(root);
  assert(aim_elf_graph_unload(&graph, &error) == AIM_ELF_OK);
  assert((finalized == std::vector<int>{0, 1, 2, 3}));
  std::puts("actual ELF finalizers: root,left,right,shared-leaf exactly once PASS");
}
