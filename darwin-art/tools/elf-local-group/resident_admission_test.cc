#include "namespace_admission.h"
#include "namespace_elf_group.h"
#include <cassert>
#include <cstring>
#include <cstdio>

namespace {
int NoOpen(const char*, char*, size_t, int*) { assert(false && "resident must not reopen a file"); return -1; }
int NoErrno() { return 2; }
}

void TestResidentAdmission(DarwinArtElfGraphHandle* original, DarwinArtElfGraphHandle* other) {
  for (bool substitute : {false, true}) {
    auto* registry = darwin_art_linker_registry_create();
    uint64_t ns = 0;
    assert(darwin_art_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &ns) == 0);
    DarwinArtElfSelectedImage *root = nullptr, *child = nullptr;
    assert(darwin_art_elf_select_image(original, "liblocal-root.so", &root, nullptr) == DARWIN_ART_ELF_OK);
    assert(darwin_art_elf_select_image(substitute ? other : original, "liblocal-child.so", &child, nullptr) == DARWIN_ART_ELF_OK);
    std::string error;
    assert(darwin_art::loader::PublishNamespaceElf(registry, ns, "/liblocal-root.so", root, false, &error));
    assert(darwin_art::loader::PublishNamespaceElf(registry, ns, "/liblocal-child.so", child, false, &error));
    darwin_art_elf_selected_image_release(root);
    darwin_art_elf_selected_image_release(child);
    auto* discovery = darwin_art_linker_discovery_create(registry, ns, "/client.so", NoOpen, NoOpen, NoErrno);
    assert(discovery);
    {
      darwin_art::loader::NamespaceAdmission admission(discovery);
      DarwinArtElfAdmission parent{}, dependency{};
      assert(darwin_art::loader::AdmitNamespaceDependency(&admission, 1, "client.so",
          "liblocal-root.so", nullptr, &parent) == 0);
      assert(parent.kind == 2 && parent.resident);
      const char* const* names = nullptr;
      size_t count = 0;
      assert(darwin_art::loader::ReadNamespaceResidentDependencies(&admission, parent.resident, &names, &count) == 0);
      assert(count >= 1 && std::strcmp(names[0], "liblocal-child.so") == 0);
      const auto result = darwin_art::loader::AdmitNamespaceDependency(&admission, parent.image,
          "liblocal-root.so", names[0], nullptr, &dependency);
      if (substitute) {
        assert(result < 0 && !dependency.resident && dependency.kind == 0);
      } else {
        assert(result == 0 && dependency.kind == 2 && dependency.resident);
        dependency.release(dependency.resident);
      }
      parent.release(parent.resident);
    }
    darwin_art_linker_discovery_destroy(discovery);
    darwin_art_linker_registry_destroy(registry);
  }
  std::puts("resident-admission: original ELF edge accepted, same-name replacement rejected PASS");
}
