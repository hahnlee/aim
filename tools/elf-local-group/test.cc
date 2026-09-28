#include "aim_elf_loader.h"
#include "namespace_elf_group.h"
#include <cassert>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>

void TestSelectedImage(AimElfGraphHandle**, int expected);
void TestSelectedImageIdentity(AimElfGraphHandle*, AimElfGraphHandle*);
void TestResidentAdmission(AimElfGraphHandle*, AimElfGraphHandle*);

static std::vector<uint8_t> Read(const char* path) {
  std::ifstream file(path, std::ios::binary);
  assert(file.good());
  return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}
int main(int argc, char** argv) {
  assert(argc == 4 || argc == 5 || argc == 6);
  const int expected = std::atoi(argv[3]);
  auto root = Read(argv[1]);
  auto child = Read(argv[2]);
  AimElfGraphSource sources[] = {
    {"liblocal-root.so", root.data(), root.size()},
    {"liblocal-child.so", child.data(), child.size()}
  };
  AimElfGraphHandle* graph = nullptr;
  char message[1024] = {};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  AimElfGraphHandle* global = nullptr;
  if (argc >= 5) {
    auto bytes = Read(argv[4]);
    AimElfGraphSource source{"libglobal.so", bytes.data(), bytes.size()};
    const auto status = aim_elf_graph_load("libglobal.so", &source, 1, nullptr, 0,
        nullptr, &global, &error);
    if (argc == 6) {
      assert(std::strcmp(argv[5], "expect-nodelete-rejection") == 0);
      assert(status == AIM_ELF_CAPABILITY && global == nullptr);
      std::puts("NODELETE execution still unsupported: capability guard preserved PASS");
      return 0;
    }
    assert(status == AIM_ELF_OK);
  }
  AimElfGlobalSource selected{global, "libglobal.so"};
  AimElfSelectedImage* selected_owner = nullptr;
  auto namespace_group = std::make_unique<aim::loader::NamespaceElfGroup>();
  if (global != nullptr) {
    AimElfGlobalSource invalid{global, "absent.so"};
    assert(aim_elf_graph_load_with_globals("liblocal-root.so", sources, 2,
        nullptr, 0, nullptr, nullptr, &invalid, 1, &graph, &error) != AIM_ELF_OK);
    assert(graph == nullptr);
    AimElfSelectedImage* original = nullptr;
    assert(aim_elf_select_image(global, "absent.so", &original, &error)
        != AIM_ELF_OK && original == nullptr);
    assert(aim_elf_select_image(global, "libglobal.so", &original, &error)
        == AIM_ELF_OK);
    assert(aim_elf_selected_image_clone(original, &selected_owner, &error)
        == AIM_ELF_OK);
    aim_elf_selected_image_release(original);
    uint64_t flags = 0;
    assert(aim_elf_selected_image_source(selected_owner, &selected, &flags, &error)
        == AIM_ELF_OK);
    assert((flags & 1) != 0); // Actual -z now fixture, not synthetic metadata.
    assert(aim_elf_graph_unload(&global, &error) == AIM_ELF_OK);
    assert(global == nullptr);
    auto* registry = aim_linker_registry_create();
    uint64_t id = 0;
    assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr,
        nullptr, 0, &id) == 0);
    std::string failure;
    assert(aim::loader::PublishNamespaceElf(registry, id, argv[4],
        selected_owner, false, &failure));
    LinkerImageLease* published = nullptr;
    assert(aim_linker_namespace_find(registry, id, "libglobal.so", &published) == 0);
    // The single-image legacy publication did not register a complete group.
    AimLocalGroupMetadata unknown_group{99, 1};
    assert(aim_linker_image_group_metadata(published, &unknown_group) == -4);
    assert(unknown_group.id == 0 && unknown_group.is_root == 0);
    uint8_t unknown_global = 9, unknown_nodelete = 9;
    assert(aim_linker_image_group_retention_flags(published,
        &unknown_global, &unknown_nodelete) == -4);
    assert(unknown_global == 0 && unknown_nodelete == 0);
    uint8_t effective_global = 0, effective_nodelete = 0;
    assert(aim_linker_image_retention_flags(published, &effective_global,
        &effective_nodelete) == 0);
    assert(effective_global == ((flags & 2) != 0));
    assert(effective_nodelete == ((flags & 8) != 0));
    auto* shared_lease = aim_linker_image_clone(published);
    assert(shared_lease);
    assert(aim_linker_image_promote_nodelete(published) == 0);
    assert(aim_linker_image_retention_flags(shared_lease, &effective_global,
        &effective_nodelete) == 0 && effective_nodelete == 1);
    aim_linker_image_release(shared_lease);
    effective_global = effective_nodelete = 1;
    assert(aim_linker_image_retention_flags(nullptr, &effective_global,
        &effective_nodelete) != 0 && effective_global == 0 && effective_nodelete == 0);
    std::puts("retention flags: actual ELF flags and shared-image NODELETE promotion PASS");
    uintptr_t value = 0;
    assert(aim::loader::ResolveNamespaceElf(published, "parent_value", nullptr, &value, &failure) == 0);
    assert(*reinterpret_cast<const int*>(value) == 73);
    assert(aim::loader::ResolveNamespaceElf(published, "parent_value", "ABSENT_VERSION", &value, &failure) == 1 && value == 0);
    assert(aim::loader::ResolveNamespaceElf(published, "absent_export", nullptr, &value, &failure) == 1 && value == 0);
    aim_linker_image_release(published);
    assert(namespace_group->Capture(registry, id, &failure));
    assert(namespace_group->size() == 1);
    // Failed recapture may not silently reuse a stale successful snapshot.
    assert(!namespace_group->Capture(registry, 0, &failure));
    assert(namespace_group->size() == 0);
    uint64_t opaque_id = 0;
    assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr,
        nullptr, 0, &opaque_id) == 0);
    auto retain = +[](void* token) -> void* {
      AimElfSelectedImage* copy = nullptr;
      assert(aim_elf_selected_image_clone(
          static_cast<AimElfSelectedImage*>(token), &copy, nullptr)
          == AIM_ELF_OK);
      return copy;
    };
    auto release = +[](void* token) {
      aim_elf_selected_image_release(
          static_cast<AimElfSelectedImage*>(token));
    };
    assert(aim_linker_namespace_publish_flags(registry, opaque_id,
        selected.soname, argv[4], selected_owner, retain, release, flags, 0) == 0);
    // A real ELF stored through the old opaque ABI must still be rejected:
    // representation is a contract, not something inferred from pointer bytes.
    assert(!namespace_group->Capture(registry, opaque_id, &failure));
    assert(namespace_group->size() == 0);
    assert(namespace_group->Capture(registry, id, &failure));
    aim_linker_registry_destroy(registry);
    aim_elf_selected_image_release(selected_owner);
    selected_owner = nullptr;
  }
  auto status = aim_elf_graph_load_with_globals("liblocal-root.so", sources, 2,
      nullptr, 0, nullptr, nullptr, namespace_group->data(), namespace_group->size(),
      &graph, &error);
  if (status != AIM_ELF_OK) {
    std::fprintf(stderr, "local-group load failed: %s\n", message);
    return 1;
  }
  AimElfGraphHandle* independent = nullptr;
  assert(aim_elf_graph_load_with_globals("liblocal-root.so", sources, 2,
      nullptr, 0, nullptr, nullptr, namespace_group->data(), namespace_group->size(),
      &independent, &error) == AIM_ELF_OK);
  namespace_group.reset();
  TestSelectedImageIdentity(graph, independent);
  TestResidentAdmission(graph, independent);
  assert(aim_elf_graph_unload(&independent, &error) == AIM_ELF_OK);
  uintptr_t address = 0;
  assert(aim_elf_graph_lookup_root(graph, "local_group_value", &address, &error)
      == AIM_ELF_OK);
  const int result = reinterpret_cast<int (*)()>(address)();
  if (result != expected) {
    std::fprintf(stderr, "local-group: expected %d, got %d\n", expected, result);
    aim_elf_graph_unload(&graph, &error);
    return 1;
  }
  TestSelectedImage(&graph, expected);
  assert(graph == nullptr);
  std::printf("local-group: executed result=%d PASS\n", result);
}
