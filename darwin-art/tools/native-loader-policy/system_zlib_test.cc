#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);

// Executes the original Android ELF, not Darwin's zlib or a symbol-only fixture.
void TestSystemZlib(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libz.so", 2, &request);
  if (!library) std::fprintf(stderr, "original zlib open: %s\n", darwin_art_linker_dlerror());
  assert(library);
  using Transform = int (*)(unsigned char*, uint64_t*, const unsigned char*, uint64_t);
  auto compress = reinterpret_cast<Transform>(darwin_art_linker_dlsym(library, "compress"));
  auto uncompress = reinterpret_cast<Transform>(darwin_art_linker_dlsym(library, "uncompress"));
  assert(compress && uncompress);
  const unsigned char source[] = "Android original ELF compression round trip: abcabcabcabcabcabc";
  unsigned char packed[256] = {}, unpacked[256] = {};
  uint64_t packed_size = sizeof(packed), unpacked_size = sizeof(unpacked);
  assert(compress(packed, &packed_size, source, sizeof(source)) == 0);
  assert(uncompress(unpacked, &unpacked_size, packed, packed_size) == 0);
  assert(unpacked_size == sizeof(source) && !std::memcmp(source, unpacked, sizeof(source)));
  unpacked_size = 1;
  assert(uncompress(unpacked, &unpacked_size, packed, packed_size) == -5); // Z_BUF_ERROR
  const unsigned char invalid[] = {0xff, 0xff, 0xff, 0xff};
  unpacked_size = sizeof(unpacked);
  assert(uncompress(unpacked, &unpacked_size, invalid, sizeof(invalid)) == -3); // Z_DATA_ERROR
  assert(!darwin_art_linker_dlsym(library, "AStatus_newOk"));
  assert(darwin_art_linker_dlerror());
  assert(darwin_art_linker_dlclose(library) == 0);
  std::puts("original Android libz: compression round trip, buffer/data errors, SONAME isolation PASS");
}
