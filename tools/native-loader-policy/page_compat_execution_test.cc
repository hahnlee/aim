#include "aim_elf_loader.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>

void TestPageCompatExecution(const char* directory) {
  assert(getpagesize() == 16384);
  int fd = open(directory, O_RDONLY | O_DIRECTORY);
  assert(fd >= 0);
  const char* name = "libpage-compat.so";
  AimElfDiscoveredGraph* discovered = nullptr;
  int is_elf = 0;
  assert(aim_elf_discover_sibling_graph(fd,
      reinterpret_cast<const uint8_t*>(name), std::strlen(name), nullptr, 0,
      &is_elf, &discovered, nullptr) == AIM_ELF_OK);
  close(fd);
  assert(is_elf && discovered);
  AimElfGraphHandle* graph = nullptr;
  char detail[1024]{};
  AimElfErrorBuffer error{detail, sizeof(detail), 0};
  auto link = [&] {
    return aim_elf_discovered_graph_link_with_owners(discovered, nullptr,
        nullptr, nullptr, 0, nullptr, 0, &graph, &error);
  };
  assert(aim_elf_discovered_graph_set_16kb_appcompat(discovered, false, &error)
      == AIM_ELF_OK);
  assert(link() != AIM_ELF_OK && !graph);
  assert(*detail);
  assert(aim_elf_discovered_graph_set_16kb_appcompat(discovered, true, &error)
      == AIM_ELF_OK);
  const auto status = link();
  if (status != AIM_ELF_OK) std::fprintf(stderr, "page compat: %s\n", detail);
  assert(status == AIM_ELF_OK && graph);
  AimElfSelectedImage* image = nullptr;
  assert(aim_elf_select_image(graph, name, &image, &error) == AIM_ELF_OK);
  uintptr_t address = 0;
  assert(aim_elf_selected_image_lookup(image, "page_compat_value", nullptr,
      &address, &error) == AIM_ELF_OK);
  assert(address && reinterpret_cast<int (*)()>(address)() == 47);
  aim_elf_selected_image_release(image);
  assert(aim_elf_graph_unload(&graph, &error) == AIM_ELF_OK);
  aim_elf_discovered_graph_destroy(&discovered);
  std::puts("Android 4K-on-16K: disabled rejection, enabled mapping/execution47 PASS");
}
