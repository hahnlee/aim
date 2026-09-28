#include "aim_elf_loader.h"
#include <cassert>
#include <cstdio>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

void TestVersionedSymbols(const char* directory) {
  const char* name = "libversioned-symbol.so";
  std::ifstream stream(std::string(directory) + "/" + name, std::ios::binary);
  assert(stream.good());
  std::vector<uint8_t> bytes{std::istreambuf_iterator<char>(stream), {}};
  AimElfGraphSource source{name, bytes.data(), bytes.size()};
  AimElfGraphHandle* graph = nullptr;
  assert(aim_elf_graph_load(name, &source, 1, nullptr, 0, nullptr,
      &graph, nullptr) == AIM_ELF_OK);
  AimElfSelectedImage* image = nullptr;
  assert(aim_elf_select_image(graph, name, &image, nullptr) == AIM_ELF_OK);
  auto lookup = [&](const char* symbol, const char* version, bool android) {
    uintptr_t address = 0;
    auto resolver = android ? aim_elf_selected_image_lookup_android
                            : aim_elf_selected_image_lookup;
    assert(resolver(image, symbol, version, &address, nullptr) == AIM_ELF_OK);
    return address;
  };
  auto value = [&](const char* symbol, const char* version, int expected) {
    auto address = lookup(symbol, version, true);
    assert(address && reinterpret_cast<int (*)()>(address)() == expected);
  };
  value("versioned_value", nullptr, 22);
  value("versioned_value", "TEST_1", 11);
  value("versioned_value", "TEST_2", 22);
  assert(!lookup("versioned_value", "ABSENT", true));
  value("global_value", "ABSENT", 33);
  assert(!lookup("global_value", "TEST_1", true));
  assert(!lookup("global_value", "ABSENT", false));
  aim_elf_selected_image_release(image);
  assert(aim_elf_graph_unload(&graph, nullptr) == AIM_ELF_OK);
  std::puts("Android ELF versions: hidden/default, unknown GLOBAL fallback, strict API isolation PASS");
}
