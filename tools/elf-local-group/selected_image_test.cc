#include "aim_elf_loader.h"
#include <cassert>
#include <cstdio>
#include <cstring>

void TestSelectedImageIdentity(AimElfGraphHandle* graph, AimElfGraphHandle* other) {
  AimElfSelectedImage *root = nullptr, *clone = nullptr, *child = nullptr, *independent = nullptr;
  assert(aim_elf_select_image(graph, "liblocal-root.so", &root, nullptr) == AIM_ELF_OK);
  assert(aim_elf_selected_image_clone(root, &clone, nullptr) == AIM_ELF_OK);
  assert(aim_elf_select_image(graph, "liblocal-child.so", &child, nullptr) == AIM_ELF_OK);
  assert(aim_elf_select_image(other, "liblocal-root.so", &independent, nullptr) == AIM_ELF_OK);
  int32_t same = -1;
  uintptr_t root_address = 0, child_address = 0;
  assert(aim_elf_selected_image_lookup(root, "local_group_value", nullptr, &root_address, nullptr) == AIM_ELF_OK && root_address);
  assert(aim_elf_selected_image_lookup(child, "child_value", nullptr, &child_address, nullptr) == AIM_ELF_OK && child_address);
  assert(aim_elf_selected_contains_address(root, root_address, &same, nullptr) == AIM_ELF_OK && same == 1);
  assert(aim_elf_selected_contains_address(clone, root_address, &same, nullptr) == AIM_ELF_OK && same == 1);
  assert(aim_elf_selected_contains_address(root, child_address, &same, nullptr) == AIM_ELF_OK && same == 0);
  assert(aim_elf_selected_contains_address(independent, root_address, &same, nullptr) == AIM_ELF_OK && same == 0);
  assert(aim_elf_selected_contains_address(root, root_address | uintptr_t{0xab00000000000000}, &same, nullptr) == AIM_ELF_OK && same == 1);
  assert(aim_elf_selected_contains_address(root, 0, &same, nullptr) == AIM_ELF_OK && same == 0);
  assert(aim_elf_selected_contains_address(nullptr, root_address, &same, nullptr) != AIM_ELF_OK && same == 0);
  AimElfSelectedImage *child_group = nullptr, *other_group = nullptr;
  assert(aim_elf_selected_group_root(child, &child_group, nullptr) == AIM_ELF_OK);
  assert(aim_elf_selected_group_root(independent, &other_group, nullptr) == AIM_ELF_OK);
  assert(aim_elf_selected_image_same(root, child_group, &same, nullptr) == AIM_ELF_OK && same);
  assert(aim_elf_selected_image_same(child_group, other_group, &same, nullptr) == AIM_ELF_OK && !same);
  aim_elf_selected_image_release(child_group);
  aim_elf_selected_image_release(other_group);
  child_group = child; // Invalid query clears output without consuming input.
  assert(aim_elf_selected_group_root(nullptr, &child_group, nullptr) != AIM_ELF_OK && !child_group);
  assert(aim_elf_selected_image_same(root, root, &same, nullptr) == AIM_ELF_OK && same == 1);
  assert(aim_elf_selected_image_same(root, clone, &same, nullptr) == AIM_ELF_OK && same == 1);
  assert(aim_elf_selected_image_same(root, child, &same, nullptr) == AIM_ELF_OK && same == 0);
  assert(aim_elf_selected_image_same(root, independent, &same, nullptr) == AIM_ELF_OK && same == 0);
  same = 1;
  assert(aim_elf_selected_image_same(root, nullptr, &same, nullptr) != AIM_ELF_OK && same == 0);
  AimElfSelectedImage* other_child = nullptr;
  assert(aim_elf_select_image(other, "liblocal-child.so", &other_child, nullptr) == AIM_ELF_OK);
  assert(aim_elf_selected_dependency_same(root, "liblocal-child.so", child, &same, nullptr) == AIM_ELF_OK && same == 1);
  assert(aim_elf_selected_dependency_same(root, "liblocal-child.so", other_child, &same, nullptr) == AIM_ELF_OK && same == 0);
  same = 1;
  assert(aim_elf_selected_dependency_same(root, "not-needed.so", child, &same, nullptr) != AIM_ELF_OK && same == 0);
  aim_elf_selected_image_release(other_child);
  aim_elf_selected_image_release(root);
  aim_elf_selected_image_release(clone);
  aim_elf_selected_image_release(child);
  aim_elf_selected_image_release(independent);
  std::puts("selected-image: clones equal, same-SONAME independent mappings distinct PASS");
}

// Consumes the original graph handle. The selected root must retain its real
// executable mapping and dependencies without exposing their private exports.
void TestSelectedImage(AimElfGraphHandle** graph, int expected) {
  AimElfSelectedImage* root = nullptr;
  AimElfSelectedImage* retained = nullptr;
  assert(aim_elf_select_image(*graph, "liblocal-root.so", &root, nullptr)
      == AIM_ELF_OK);
  assert(aim_elf_selected_image_clone(root, &retained, nullptr)
      == AIM_ELF_OK);
  uintptr_t before = 0;
  assert(aim_elf_selected_image_lookup(root, "local_group_value", nullptr,
      &before, nullptr) == AIM_ELF_OK && before != 0);
  uintptr_t absent = 123;
  assert(aim_elf_selected_image_lookup(root, "child_value", nullptr,
      &absent, nullptr) == AIM_ELF_OK && absent == 0);
  absent = 123;
  assert(aim_elf_selected_image_lookup(root, "local_group_value", "NO_SUCH_VERSION",
      &absent, nullptr) == AIM_ELF_OK && absent == 0);
  absent = 123;
  assert(aim_elf_selected_image_lookup(nullptr, "local_group_value", nullptr,
      &absent, nullptr) != AIM_ELF_OK && absent == 0);
  assert(aim_elf_graph_unload(graph, nullptr) == AIM_ELF_OK);
  assert(*graph == nullptr);
  AimElfSelectedImage* group = nullptr;
  assert(aim_elf_selected_group_root(retained, &group, nullptr) == AIM_ELF_OK);
  int32_t same_group = 0;
  assert(aim_elf_selected_image_same(retained, group, &same_group, nullptr) == AIM_ELF_OK && same_group);
  aim_elf_selected_image_release(group);
  aim_elf_selected_image_release(root);
  const char* needed = nullptr;
  assert(aim_elf_selected_image_needed(retained, 0, &needed, nullptr)
      == AIM_ELF_OK && needed != nullptr);
  assert(std::strcmp(needed, "liblocal-child.so") == 0);
  // An index outside the actual table must terminate, not read freed memory.
  assert(aim_elf_selected_image_needed(retained, SIZE_MAX, &needed, nullptr)
      == AIM_ELF_OK && needed == nullptr);
  uintptr_t after = 0;
  assert(aim_elf_selected_image_lookup(retained, "local_group_value", nullptr,
      &after, nullptr) == AIM_ELF_OK && after == before);
  assert(reinterpret_cast<int (*)()>(after)() == expected);
  int32_t contains = 0;
  assert(aim_elf_selected_contains_address(retained, after, &contains, nullptr) == AIM_ELF_OK && contains == 1);
  aim_elf_selected_image_release(retained);
  std::puts("selected-image: scoped lookup and execution after original unload PASS");
}
