#include "aim_elf_loader.h"
#include "aim_linker_namespace.h"
#include <cassert>
#include <cstring>
#include <cstdio>
#include <fcntl.h>
#include <unistd.h>
static AimElfGraphHandle* active_graph;
static LinkerRegistry* registry;
static uint64_t namespace_id;
static int callbacks;
static void* Retain(void* image) {
  AimElfSelectedImage* copy = nullptr;
  assert(aim_elf_selected_image_clone(static_cast<AimElfSelectedImage*>(image), &copy, nullptr) == AIM_ELF_OK);
  return copy;
}
static void Release(void* image) { aim_elf_selected_image_release(static_cast<AimElfSelectedImage*>(image)); }
static int ObserveLinkedImage() {
  ++callbacks;
  auto* nested = aim_linker_operation_enter(registry);
  assert(nested);
  // Reenter both locks from actual foreign constructor execution.
  AimElfSelectedImage* direct = nullptr;
  assert(aim_elf_select_image(active_graph, "libinitialization.so", &direct, nullptr) == AIM_ELF_OK);
  LinkerImageLease* resident = nullptr;
  assert(aim_linker_namespace_find(registry, namespace_id, "libinitialization.so", &resident) == 0);
  void* payload = nullptr;
  assert(aim_linker_image_typed_payload(resident, AIM_IMAGE_ELF_SELECTED, &payload) == 0);
  int same = 0;
  assert(aim_elf_selected_image_same(direct, static_cast<AimElfSelectedImage*>(payload), &same, nullptr) == AIM_ELF_OK && same);
  AimLocalGroupMetadata published{}, original{};
  assert(aim_elf_selected_group_info(direct, &original.id, &original.is_root, nullptr) == AIM_ELF_OK);
  assert(aim_linker_image_group_metadata(resident, &published) == 0);
  assert(published.id == original.id && published.is_root == original.is_root);
  uint8_t root_global = 9, root_nodelete = 9;
  assert(aim_linker_image_group_retention_flags(resident,
      &root_global, &root_nodelete) == 0);
  assert(root_global == 0 && root_nodelete == 0);
  size_t opens = 0;
  assert(aim_linker_image_open_count(resident, &opens) == 0 && opens == 1);
  // No recursive constructor execution or lock acquisition deadlock.
  assert(aim_elf_graph_initialize(active_graph, nullptr) != AIM_ELF_OK);
  aim_linker_image_release(resident);
  aim_elf_selected_image_release(direct);
  assert(aim_linker_operation_leave(nested) == 0);
  return 42;
}
static AimElfResolveStatus Resolve(void*, const AimElfSymbolRequest* request,
    uintptr_t* address, AimElfErrorBuffer*) {
  *address = 0;
  if (std::strcmp(request->symbol, "observe_linked_image") != 0) return AIM_ELF_RESOLVE_NOT_FOUND;
  *address = reinterpret_cast<uintptr_t>(ObserveLinkedImage);
  return AIM_ELF_RESOLVE_FOUND;
}
// Actual callback capability has process lifetime; retains borrow the immortal
// function slot, not a fabricated runtime service or an ELF mapping.
static auto observer = &ObserveLinkedImage;
static void* RetainObserver(void* value) { assert(value == &observer); return value; }
static void ReleaseObserver(void* value) { assert(value == &observer); }
static int AdmitObserver(void*, uint64_t, const char*, const char* name, const char*, AimElfAdmission* output) {
  assert(std::strcmp(name, "libinitialization-provider.so") == 0);
  *output = {2, -1, 2, &observer, ReleaseObserver};
  return 0;
}
int main(int argc, char** argv) {
  assert(argc == 2);
  const int fd = open(argv[1], O_RDONLY);
  assert(fd >= 0);
  const char* name = "libinitialization.so";
  AimElfDiscoveredGraph* discovered = nullptr;
  int elf = 0;
  assert(aim_elf_discover_resident_graph(fd, 1,
      reinterpret_cast<const uint8_t*>(name), std::strlen(name), AdmitObserver,
      nullptr, &elf, &discovered, nullptr) == AIM_ELF_OK && elf);
  close(fd);
  AimElfImagePlacement placements[] = {{1, 10}, {2, 20}};
  const uint64_t app_visible[] = {1, 2};
  const uint64_t provider_visible[] = {2};
  const AimElfNamespaceScope scopes[] = {
      {10, app_visible, 2, nullptr, 0}, {20, provider_visible, 1, nullptr, 0}};
  assert(aim_elf_discovered_graph_set_namespace_scopes(discovered,
      placements, 2, scopes, 2, 0, nullptr) == AIM_ELF_OK);
  placements[0].image = 999; // A rejected update cannot replace valid decisions.
  assert(aim_elf_discovered_graph_set_namespace_scopes(discovered,
      placements, 2, scopes, 2, 0, nullptr) != AIM_ELF_OK);
  placements[0].image = 1;
  AimElfGraphHandle* graph = nullptr;
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  char detail[2048]{};
  AimElfErrorBuffer error{detail, sizeof(detail), 0};
  AimElfNativeOwner owner{&observer, RetainObserver, ReleaseObserver};
  const AimElfNamespaceScope restricted[] = {
      {10, app_visible, 1, nullptr, 0}, {20, provider_visible, 1, nullptr, 0}};
  assert(aim_elf_discovered_graph_set_namespace_scopes(discovered,
      placements, 2, restricted, 2, 0, nullptr) == AIM_ELF_OK);
  assert(aim_elf_discovered_graph_link_with_owners(discovered, &options,
      nullptr, nullptr, 0, &owner, 1, &graph, &error) != AIM_ELF_OK);
  assert(!graph); // Actual relocation cannot call a non-visible resident.
  assert(aim_elf_discovered_graph_set_namespace_scopes(discovered,
      placements, 2, scopes, 2, 0, nullptr) == AIM_ELF_OK);
  auto status = aim_elf_discovered_graph_link_with_owners(discovered, &options,
      nullptr, nullptr, 0, &owner, 1, &graph, &error);
  if (status != AIM_ELF_OK) std::fprintf(stderr, "linked initializer: %s\n", detail);
  assert(status == AIM_ELF_OK);
  aim_elf_discovered_graph_destroy(&discovered);
  AimElfSelectedImage* selected = nullptr;
  assert(aim_elf_select_image(graph, name, &selected, nullptr) == AIM_ELF_OK);
  uintptr_t address = 0;
  assert(aim_elf_selected_image_lookup(selected, "initialized_value", nullptr,
      &address, nullptr) == AIM_ELF_OK && address);
  auto read = reinterpret_cast<int (*)()>(address);
  assert(read() == 0);
  registry = aim_linker_registry_create();
  assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &namespace_id) == 0);
  AimLocalGroupMetadata group{};
  assert(aim_elf_selected_group_info(selected, &group.id, &group.is_root, nullptr) == AIM_ELF_OK);
  AimImagePublication publication{namespace_id, name,
      "/system/lib64/libinitialization.so", selected, Retain, Release, 0, 0,
      AIM_IMAGE_ELF_SELECTED, 0, 0, 0};
  assert(aim_linker_namespace_publish_groups(registry, &publication,
      &group, 1, nullptr) == 0);
  LinkerImageLease* lookup = nullptr;
  LinkerImageLease* outer_open = nullptr;
  assert(aim_linker_namespace_find(registry, namespace_id, name, &lookup) == 0);
  size_t unknown_edges = 99;
  assert(aim_linker_image_dependency_count(lookup, &unknown_edges) == -4 && unknown_edges == 0);
  assert(aim_linker_image_acquire_open(lookup, &outer_open) == 0);
  aim_linker_image_release(lookup);
  active_graph = graph;
  auto* operation = aim_linker_operation_enter(registry);
  assert(operation);
  assert(aim_elf_graph_initialize(graph, nullptr) == AIM_ELF_OK);
  assert(aim_linker_operation_leave(operation) == 0);
  assert(callbacks == 1);
  assert(read() == 42);
  aim_linker_image_release(outer_open);
  std::puts("discovery namespace ABI: invisible dependency rejected, restored scope executes42 PASS");
  assert(aim_elf_graph_initialize(graph, nullptr) != AIM_ELF_OK);
  assert(callbacks == 1);
  uintptr_t finalized_address = 0;
  assert(aim_elf_selected_image_lookup(selected, "finalized_count", nullptr,
      &finalized_address, nullptr) == AIM_ELF_OK && finalized_address);
  auto finalized = reinterpret_cast<int (*)()>(finalized_address);
  assert(finalized() == 0);
  assert(aim_elf_selected_group_finalize(selected, nullptr) == AIM_ELF_OK);
  assert(finalized() == 1);
  assert(aim_elf_graph_finalize(graph, nullptr) == AIM_ELF_OK);
  assert(finalized() == 1);
  aim_elf_graph_unload(&graph, nullptr);
  // Test-only observer accesses retained storage, not torn-down app behavior.
  assert(finalized() == 1);
  assert(read() == 42);
  aim_elf_selected_image_release(selected);
  aim_linker_registry_destroy(registry);
}
