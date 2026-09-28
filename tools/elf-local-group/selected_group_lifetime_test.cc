#include "aim_elf_loader.h"
#include "aim_linker_namespace.h"
#include "namespace_dependency_groups.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>
#include <utility>
#include <vector>

static int Admit(void* context, uint64_t, const char*, const char* name,
    const char*, AimElfAdmission* output) {
  assert(std::strcmp(name, "liblocal-child.so") == 0);
  const int fd = open(static_cast<const char*>(context), O_RDONLY);
  if (fd < 0) return -1;
  *output = {1, fd, 2, nullptr, nullptr};
  return 0;
}
static int Publish(void*, uintptr_t, uintptr_t) { return 0; }
static void* RetainSelected(void* image) {
  AimElfSelectedImage* copy = nullptr;
  assert(aim_elf_selected_image_clone(static_cast<AimElfSelectedImage*>(image), &copy, nullptr) == AIM_ELF_OK);
  return copy;
}
static void ReleaseSelected(void* image) {
  aim_elf_selected_image_release(static_cast<AimElfSelectedImage*>(image));
}
static int Finalize(void* context, uintptr_t start, uintptr_t end) {
  static_cast<std::vector<std::pair<uintptr_t, uintptr_t>>*>(context)->emplace_back(start, end);
  return 0;
}
int main(int argc, char** argv) {
  assert(argc == 3);
  const int fd = open(argv[1], O_RDONLY);
  assert(fd >= 0);
  const char* root_name = "liblocal-root.so";
  const char* child_name = "liblocal-child.so";
  AimElfDiscoveredGraph* discovered = nullptr;
  int elf = 0;
  assert(aim_elf_discover_resident_graph(fd, 1,
      reinterpret_cast<const uint8_t*>(root_name), std::strlen(root_name),
      Admit, argv[2], &elf, &discovered, nullptr) == AIM_ELF_OK && elf);
  close(fd);
  const AimElfImagePlacement placements[] = {{1, 10}, {2, 20}};
  const uint64_t root_visible[] = {1, 2}, child_visible[] = {2};
  const AimElfNamespaceScope scopes[] = {
      {10, root_visible, 2, nullptr, 0}, {20, child_visible, 1, nullptr, 0}};
  assert(aim_elf_discovered_graph_set_namespace_scopes(discovered,
      placements, 2, scopes, 2, 0, nullptr) == AIM_ELF_OK);
  std::vector<std::pair<uintptr_t, uintptr_t>> finalized;
  const AimElfLifecycleCallbacks lifecycle{AIM_ELF_ABI_VERSION, Publish, Finalize, &finalized};
  AimElfGraphHandle* graph = nullptr;
  assert(aim_elf_discovered_graph_load_with_owners(discovered, nullptr,
      &lifecycle, nullptr, 0, nullptr, 0, &graph, nullptr) == AIM_ELF_OK);
  aim_elf_discovered_graph_destroy(&discovered);
  uintptr_t root_address = 0;
  assert(aim_elf_graph_lookup_root(graph, "local_group_value", &root_address, nullptr) == AIM_ELF_OK);
  AimElfSelectedImage *root = nullptr, *child = nullptr, *clone = nullptr;
  assert(aim_elf_select_image(graph, root_name, &root, nullptr) == AIM_ELF_OK);
  assert(aim_elf_select_image(graph, child_name, &child, nullptr) == AIM_ELF_OK);
  assert(aim_elf_selected_image_clone(child, &clone, nullptr) == AIM_ELF_OK);
  uint64_t root_group_id = 0, child_group_id = 0, clone_group_id = 0;
  uint8_t root_is_root = 0, child_is_root = 0, clone_is_root = 0;
  assert(aim_elf_selected_group_info(root, &root_group_id, &root_is_root, nullptr)
      == AIM_ELF_OK);
  assert(aim_elf_selected_group_info(child, &child_group_id, &child_is_root, nullptr)
      == AIM_ELF_OK);
  assert(aim_elf_selected_group_info(clone, &clone_group_id, &clone_is_root, nullptr)
      == AIM_ELF_OK);
  assert(root_group_id != 0 && child_group_id != 0 && root_group_id != child_group_id);
  assert(root_is_root == 1 && child_is_root == 1);
  assert(clone_group_id == child_group_id && clone_is_root == child_is_root);
  {
    auto* registry = aim_linker_registry_create();
    uint64_t root_ns = 0, child_ns = 0;
    assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &root_ns) == 0);
    assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &child_ns) == 0);
    AimImagePublication records[] = {
      {root_ns, root_name, "/root.so", root, RetainSelected, ReleaseSelected, 0, 0, AIM_IMAGE_ELF_SELECTED, 0, 0, 0},
      {child_ns, child_name, "/child.so", child, RetainSelected, ReleaseSelected, 0, 0, AIM_IMAGE_ELF_SELECTED, 0, 0, 0},
    };
    AimLocalGroupMetadata groups[] = {{root_group_id, 1}, {child_group_id, 1}};
    std::vector<AimGroupDependency> edges;
    std::string error;
    assert(aim::loader::CollectGroupDependencies(0, root, &edges, &error));
    assert(aim::loader::CollectGroupDependencies(1, child, &edges, &error));
    assert(edges.size() == 1 && edges[0].target_group == child_group_id);
    LinkerPublication* token = nullptr;
    assert(aim_linker_namespace_publish_linked_groups(registry, records, groups, 2,
        edges.data(), edges.size(), &token) == 0);
    LinkerImageLease* observed = nullptr;
    assert(aim_linker_namespace_find(registry, child_ns, child_name, &observed) == 0);
    size_t incoming = 0;
    assert(aim_linker_image_dependency_count(observed, &incoming) == 0 && incoming == 1);
    DetachedGroup* detached_child = nullptr;
    assert(aim_linker_group_detach(registry, observed, &detached_child) == 1 && !detached_child);
    LinkerImageLease* opened = nullptr;
    assert(aim_linker_image_acquire_open(observed, &opened) == 0);
    aim_linker_image_release(opened);
    assert(aim_linker_image_dependency_count(observed, &incoming) == 0 && incoming == 1);
    LinkerImageLease* parent_lookup = nullptr;
    assert(aim_linker_namespace_find(registry, root_ns, root_name, &parent_lookup) == 0);
    DetachedGroup* detached_parent = nullptr;
    assert(aim_linker_group_detach(registry, parent_lookup, &detached_parent) == 0);
    LinkerImageLease* resurrection = nullptr;
    assert(aim_linker_image_acquire_open(parent_lookup, &resurrection) != 0 && !resurrection);
    aim_linker_publication_destroy(token);
    aim_linker_image_release(parent_lookup);
    aim_linker_detached_group_destroy(detached_parent);
    assert(aim_linker_image_dependency_count(observed, &incoming) == 0 && incoming == 0);
    assert(aim_linker_group_detach(registry, observed, &detached_child) == 0);
    aim_linker_image_release(observed);
    aim_linker_detached_group_destroy(detached_child);
    aim_linker_registry_destroy(registry);
    std::puts("original cross-namespace detach: child retained by parent, parent release then child detach PASS");
  }
  AimElfGlobalSource source{};
  uint64_t flags = 0;
  assert(aim_elf_selected_image_source(clone, &source, &flags, nullptr) == AIM_ELF_OK);
  assert(aim_elf_graph_initialize(source.graph, nullptr) != AIM_ELF_OK);
  assert(aim_elf_graph_finalize(source.graph, nullptr) != AIM_ELF_OK);
  AimElfSelectedImage* unrelated = nullptr;
  assert(aim_elf_select_image(source.graph, root_name, &unrelated, nullptr) != AIM_ELF_OK && !unrelated);
  assert(finalized.empty());
  assert(aim_elf_graph_unload(&graph, nullptr) == AIM_ELF_OK && !graph);
  AimElfSelectedImage* original_dependency = nullptr;
  assert(aim_elf_selected_dependency_image(root, child_name,
      &original_dependency, nullptr) == AIM_ELF_OK);
  int32_t dependency_same = 0;
  assert(aim_elf_selected_image_same(original_dependency, child,
      &dependency_same, nullptr) == AIM_ELF_OK && dependency_same);
  AimElfSelectedImage* rejected_dependency = nullptr;
  assert(aim_elf_selected_dependency_image(child, root_name,
      &rejected_dependency, nullptr) != AIM_ELF_OK && !rejected_dependency);
  uint64_t retained_group_id = 0;
  uint8_t retained_is_root = 0;
  assert(aim_elf_selected_group_info(root, &retained_group_id, &retained_is_root, nullptr)
      == AIM_ELF_OK && retained_group_id == root_group_id
      && retained_is_root == root_is_root);
  assert(aim_elf_selected_group_info(clone, &retained_group_id, &retained_is_root, nullptr)
      == AIM_ELF_OK && retained_group_id == clone_group_id
      && retained_is_root == clone_is_root);
  aim_elf_selected_image_release(root);
  uint64_t invalid_group_id = UINT64_C(0xffffffffffffffff);
  uint8_t invalid_is_root = UINT8_C(0xff);
  assert(aim_elf_selected_group_info(nullptr, &invalid_group_id, &invalid_is_root, nullptr)
      != AIM_ELF_OK && invalid_group_id == 0 && invalid_is_root == 0);
  assert(finalized.size() == 1 && finalized[0].first <= root_address && root_address < finalized[0].second);
  aim_elf_selected_image_release(child);
  uintptr_t address = 0;
  assert(aim_elf_selected_image_lookup(clone, "child_value", nullptr, &address, nullptr) == AIM_ELF_OK && address);
  assert(reinterpret_cast<int (*)()>(address)() == 16);
  AimElfSelectedImage* group_root = nullptr;
  assert(aim_elf_selected_group_root(clone, &group_root, nullptr) == AIM_ELF_OK);
  int32_t same = 0;
  assert(aim_elf_selected_image_same(clone, group_root, &same, nullptr) == AIM_ELF_OK && same);
  aim_elf_selected_image_release(group_root);
  assert(finalized.size() == 1);
  aim_elf_selected_image_release(clone);
  assert(finalized.size() == 1);
  uintptr_t retained_address = 0;
  assert(aim_elf_selected_image_lookup(original_dependency, "child_value", nullptr,
      &retained_address, nullptr) == AIM_ELF_OK && retained_address == address);
  assert(reinterpret_cast<int (*)()>(retained_address)() == 16);
  aim_elf_selected_image_release(original_dependency);
  assert(finalized.size() == 2 && finalized[1].first <= address && address < finalized[1].second);
  std::puts("selected C ABI: parent finalized, child executes16; read-only source and last-release finalization PASS");
}
