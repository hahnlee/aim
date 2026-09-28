#include "aim_elf_loader.h"
#include <android/bitmap.h>
#include <android/imagedecoder.h>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <vector>

extern "C" void* aim_linker_dlsym(void*, const char*);
void TestGraphicsNdkElfAbi(void* library) {
  const char* path = std::getenv("AIM_TEST_GRAPHICS_ABI");
  assert(path && "runner must supply actual NDK-built graphics ABI consumer");
  std::ifstream input(path, std::ios::binary);
  assert(input.good());
  std::vector<uint8_t> bytes{std::istreambuf_iterator<char>(input), {}};
  AimElfGraphSource source{"libgraphics-abi.so", bytes.data(), bytes.size()};
  AimElfGraphHandle* graph = nullptr;
  assert(aim_elf_graph_load("libgraphics-abi.so", &source, 1, nullptr, 0,
      nullptr, &graph, nullptr) == AIM_ELF_OK);
  AimElfSelectedImage* image = nullptr;
  assert(aim_elf_select_image(graph, "libgraphics-abi.so", &image, nullptr)
      == AIM_ELF_OK);
  uintptr_t entry = 0;
  assert(aim_elf_selected_image_lookup(image, "graphics_ndk_abi", nullptr,
      &entry, nullptr) == AIM_ELF_OK && entry);
  struct GraphicsApi {
    decltype(&AndroidBitmap_compress) compress;
    decltype(&AImageDecoder_createFromBuffer) create;
    decltype(&AImageDecoder_setCrop) crop;
    decltype(&AImageDecoder_decodeImage) decode;
    decltype(&AImageDecoder_delete) destroy;
  } api{};
#define RESOLVE(field, name) api.field = reinterpret_cast<decltype(api.field)>(aim_linker_dlsym(library, #name)); assert(api.field)
  RESOLVE(compress, AndroidBitmap_compress);
  RESOLVE(create, AImageDecoder_createFromBuffer);
  RESOLVE(crop, AImageDecoder_setCrop);
  RESOLVE(decode, AImageDecoder_decodeImage);
  RESOLVE(destroy, AImageDecoder_delete);
#undef RESOLVE
  const int result = reinterpret_cast<int (*)(const GraphicsApi*)>(entry)(&api);
  if (result) std::fprintf(stderr, "graphics ELF ABI failed at stage %d\n", result);
  assert(result == 0);
  aim_elf_selected_image_release(image);
  assert(aim_elf_graph_unload(&graph, nullptr) == AIM_ELF_OK);
  std::puts("graphics NDK ELF ABI: Android callback, ARect value, cropped blue pixel PASS");
}
