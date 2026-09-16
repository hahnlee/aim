#include "darwin_art_elf_loader.h"
#include "namespace_elf_group.h"
#include <cassert>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>

void TestSelectedImage(DarwinArtElfGraphHandle**, int expected);
void TestSelectedImageIdentity(DarwinArtElfGraphHandle*, DarwinArtElfGraphHandle*);
void TestResidentAdmission(DarwinArtElfGraphHandle*, DarwinArtElfGraphHandle*);

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
  DarwinArtElfGraphSource sources[] = {
    {"liblocal-root.so", root.data(), root.size()},
    {"liblocal-child.so", child.data(), child.size()}
  };
  DarwinArtElfGraphHandle* graph = nullptr;
  char message[1024] = {};
  DarwinArtElfErrorBuffer error{message, sizeof(message), 0};
  DarwinArtElfGraphHandle* global = nullptr;
  if (argc >= 5) {
    auto bytes = Read(argv[4]);
    DarwinArtElfGraphSource source{"libglobal.so", bytes.data(), bytes.size()};
    const auto status = darwin_art_elf_graph_load("libglobal.so", &source, 1, nullptr, 0,
        nullptr, &global, &error);
    if (argc == 6) {
      assert(std::strcmp(argv[5], "expect-nodelete-rejection") == 0);
      assert(status == DARWIN_ART_ELF_CAPABILITY && global == nullptr);
      std::puts("NODELETE execution still unsupported: capability guard preserved PASS");
      return 0;
    }
    assert(status == DARWIN_ART_ELF_OK);
  }
  DarwinArtElfGlobalSource selected{global, "libglobal.so"};
  DarwinArtElfSelectedImage* selected_owner = nullptr;
  auto namespace_group = std::make_unique<darwin_art::loader::NamespaceElfGroup>();
  if (global != nullptr) {
    DarwinArtElfGlobalSource invalid{global, "absent.so"};
    assert(darwin_art_elf_graph_load_with_globals("liblocal-root.so", sources, 2,
        nullptr, 0, nullptr, nullptr, &invalid, 1, &graph, &error) != DARWIN_ART_ELF_OK);
    assert(graph == nullptr);
    DarwinArtElfSelectedImage* original = nullptr;
    assert(darwin_art_elf_select_image(global, "absent.so", &original, &error)
        != DARWIN_ART_ELF_OK && original == nullptr);
    assert(darwin_art_elf_select_image(global, "libglobal.so", &original, &error)
        == DARWIN_ART_ELF_OK);
    assert(darwin_art_elf_selected_image_clone(original, &selected_owner, &error)
        == DARWIN_ART_ELF_OK);
    darwin_art_elf_selected_image_release(original);
    uint64_t flags = 0;
    assert(darwin_art_elf_selected_image_source(selected_owner, &selected, &flags, &error)
        == DARWIN_ART_ELF_OK);
    assert((flags & 1) != 0); // Actual -z now fixture, not synthetic metadata.
    assert(darwin_art_elf_graph_unload(&global, &error) == DARWIN_ART_ELF_OK);
    assert(global == nullptr);
    auto* registry = darwin_art_linker_registry_create();
    uint64_t id = 0;
    assert(darwin_art_linker_namespace_create(registry, 0, nullptr, nullptr,
        nullptr, 0, &id) == 0);
    std::string failure;
    assert(darwin_art::loader::PublishNamespaceElf(registry, id, argv[4],
        selected_owner, false, &failure));
    LinkerImageLease* published = nullptr;
    assert(darwin_art_linker_namespace_find(registry, id, "libglobal.so", &published) == 0);
    // The single-image legacy publication did not register a complete group.
    DarwinArtLocalGroupMetadata unknown_group{99, 1};
    assert(darwin_art_linker_image_group_metadata(published, &unknown_group) == -4);
    assert(unknown_group.id == 0 && unknown_group.is_root == 0);
    uint8_t unknown_global = 9, unknown_nodelete = 9;
    assert(darwin_art_linker_image_group_retention_flags(published,
        &unknown_global, &unknown_nodelete) == -4);
    assert(unknown_global == 0 && unknown_nodelete == 0);
    uint8_t effective_global = 0, effective_nodelete = 0;
    assert(darwin_art_linker_image_retention_flags(published, &effective_global,
        &effective_nodelete) == 0);
    assert(effective_global == ((flags & 2) != 0));
    assert(effective_nodelete == ((flags & 8) != 0));
    auto* shared_lease = darwin_art_linker_image_clone(published);
    assert(shared_lease);
    assert(darwin_art_linker_image_promote_nodelete(published) == 0);
    assert(darwin_art_linker_image_retention_flags(shared_lease, &effective_global,
        &effective_nodelete) == 0 && effective_nodelete == 1);
    darwin_art_linker_image_release(shared_lease);
    effective_global = effective_nodelete = 1;
    assert(darwin_art_linker_image_retention_flags(nullptr, &effective_global,
        &effective_nodelete) != 0 && effective_global == 0 && effective_nodelete == 0);
    std::puts("retention flags: actual ELF flags and shared-image NODELETE promotion PASS");
    uintptr_t value = 0;
    assert(darwin_art::loader::ResolveNamespaceElf(published, "parent_value", nullptr, &value, &failure) == 0);
    assert(*reinterpret_cast<const int*>(value) == 73);
    assert(darwin_art::loader::ResolveNamespaceElf(published, "parent_value", "ABSENT_VERSION", &value, &failure) == 1 && value == 0);
    assert(darwin_art::loader::ResolveNamespaceElf(published, "absent_export", nullptr, &value, &failure) == 1 && value == 0);
    darwin_art_linker_image_release(published);
    assert(namespace_group->Capture(registry, id, &failure));
    assert(namespace_group->size() == 1);
    // Failed recapture may not silently reuse a stale successful snapshot.
    assert(!namespace_group->Capture(registry, 0, &failure));
    assert(namespace_group->size() == 0);
    uint64_t opaque_id = 0;
    assert(darwin_art_linker_namespace_create(registry, 0, nullptr, nullptr,
        nullptr, 0, &opaque_id) == 0);
    auto retain = +[](void* token) -> void* {
      DarwinArtElfSelectedImage* copy = nullptr;
      assert(darwin_art_elf_selected_image_clone(
          static_cast<DarwinArtElfSelectedImage*>(token), &copy, nullptr)
          == DARWIN_ART_ELF_OK);
      return copy;
    };
    auto release = +[](void* token) {
      darwin_art_elf_selected_image_release(
          static_cast<DarwinArtElfSelectedImage*>(token));
    };
    assert(darwin_art_linker_namespace_publish_flags(registry, opaque_id,
        selected.soname, argv[4], selected_owner, retain, release, flags, 0) == 0);
    // A real ELF stored through the old opaque ABI must still be rejected:
    // representation is a contract, not something inferred from pointer bytes.
    assert(!namespace_group->Capture(registry, opaque_id, &failure));
    assert(namespace_group->size() == 0);
    assert(namespace_group->Capture(registry, id, &failure));
    darwin_art_linker_registry_destroy(registry);
    darwin_art_elf_selected_image_release(selected_owner);
    selected_owner = nullptr;
  }
  auto status = darwin_art_elf_graph_load_with_globals("liblocal-root.so", sources, 2,
      nullptr, 0, nullptr, nullptr, namespace_group->data(), namespace_group->size(),
      &graph, &error);
  if (status != DARWIN_ART_ELF_OK) {
    std::fprintf(stderr, "local-group load failed: %s\n", message);
    return 1;
  }
  DarwinArtElfGraphHandle* independent = nullptr;
  assert(darwin_art_elf_graph_load_with_globals("liblocal-root.so", sources, 2,
      nullptr, 0, nullptr, nullptr, namespace_group->data(), namespace_group->size(),
      &independent, &error) == DARWIN_ART_ELF_OK);
  namespace_group.reset();
  TestSelectedImageIdentity(graph, independent);
  TestResidentAdmission(graph, independent);
  assert(darwin_art_elf_graph_unload(&independent, &error) == DARWIN_ART_ELF_OK);
  uintptr_t address = 0;
  assert(darwin_art_elf_graph_lookup_root(graph, "local_group_value", &address, &error)
      == DARWIN_ART_ELF_OK);
  const int result = reinterpret_cast<int (*)()>(address)();
  if (result != expected) {
    std::fprintf(stderr, "local-group: expected %d, got %d\n", expected, result);
    darwin_art_elf_graph_unload(&graph, &error);
    return 1;
  }
  TestSelectedImage(&graph, expected);
  assert(graph == nullptr);
  std::printf("local-group: executed result=%d PASS\n", result);
}
