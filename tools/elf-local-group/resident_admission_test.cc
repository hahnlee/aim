#include "namespace_admission.h"
#include "namespace_elf_group.h"
#include <cassert>
#include <cstring>
#include <cstdio>

namespace {
int NoOpen(const char*, char*, size_t, int*) { assert(false && "resident must not reopen a file"); return -1; }
int NoErrno() { return 2; }
}

void TestResidentAdmission(AimElfGraphHandle* original, AimElfGraphHandle* other) {
  for (bool substitute : {false, true}) {
    auto* registry = aim_linker_registry_create();
    uint64_t ns = 0;
    assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &ns) == 0);
    AimElfSelectedImage *root = nullptr, *child = nullptr;
    assert(aim_elf_select_image(original, "liblocal-root.so", &root, nullptr) == AIM_ELF_OK);
    assert(aim_elf_select_image(substitute ? other : original, "liblocal-child.so", &child, nullptr) == AIM_ELF_OK);
    std::string error;
    assert(aim::loader::PublishNamespaceElf(registry, ns, "/liblocal-root.so", root, false, &error));
    assert(aim::loader::PublishNamespaceElf(registry, ns, "/liblocal-child.so", child, false, &error));
    aim_elf_selected_image_release(root);
    aim_elf_selected_image_release(child);
    auto* discovery = aim_linker_discovery_create(registry, ns, "/client.so", NoOpen, NoOpen, NoErrno);
    assert(discovery);
    {
      aim::loader::NamespaceAdmission admission(discovery);
      AimElfAdmission parent{}, dependency{};
      assert(aim::loader::AdmitNamespaceDependency(&admission, 1, "client.so",
          "liblocal-root.so", nullptr, &parent) == 0);
      assert(parent.kind == 2 && parent.resident);
      const char* const* names = nullptr;
      size_t count = 0;
      assert(aim::loader::ReadNamespaceResidentDependencies(&admission, parent.resident, &names, &count) == 0);
      assert(count >= 1 && std::strcmp(names[0], "liblocal-child.so") == 0);
      const auto result = aim::loader::AdmitNamespaceDependency(&admission, parent.image,
          "liblocal-root.so", names[0], nullptr, &dependency);
      if (substitute) {
        assert(result < 0 && !dependency.resident && dependency.kind == 0);
      } else {
        assert(result == 0 && dependency.kind == 2 && dependency.resident);
        dependency.release(dependency.resident);
      }
      parent.release(parent.resident);
    }
    aim_linker_discovery_destroy(discovery);
    aim_linker_registry_destroy(registry);
  }
  std::puts("resident-admission: original ELF edge accepted, same-name replacement rejected PASS");
}
