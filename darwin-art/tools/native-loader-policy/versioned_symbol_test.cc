#include "darwin_art_elf_loader.h"
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
  DarwinArtElfGraphSource source{name, bytes.data(), bytes.size()};
  DarwinArtElfGraphHandle* graph = nullptr;
  assert(darwin_art_elf_graph_load(name, &source, 1, nullptr, 0, nullptr,
      &graph, nullptr) == DARWIN_ART_ELF_OK);
  DarwinArtElfSelectedImage* image = nullptr;
  assert(darwin_art_elf_select_image(graph, name, &image, nullptr) == DARWIN_ART_ELF_OK);
  auto lookup = [&](const char* symbol, const char* version, bool android) {
    uintptr_t address = 0;
    auto resolver = android ? darwin_art_elf_selected_image_lookup_android
                            : darwin_art_elf_selected_image_lookup;
    assert(resolver(image, symbol, version, &address, nullptr) == DARWIN_ART_ELF_OK);
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
  darwin_art_elf_selected_image_release(image);
  assert(darwin_art_elf_graph_unload(&graph, nullptr) == DARWIN_ART_ELF_OK);
  std::puts("Android ELF versions: hidden/default, unknown GLOBAL fallback, strict API isolation PASS");
}
