#include "../compat/loader/elf_graph_cache.h"
#include <cassert>
#include <cstring>
#include <iostream>

// Only the graph FFI is doubled: the production cache implementation is linked.
struct DarwinArtElfGraphHandle { uintptr_t value; };
static int clones = 0;
static int drops = 0;
static DarwinArtElfGraphHandle* remove_during_lookup = nullptr;
extern "C" const char* darwin_art_elf_status_name(int32_t) { return "test"; }
extern "C" DarwinArtElfStatus darwin_art_elf_graph_clone(
    DarwinArtElfGraphHandle* source, DarwinArtElfGraphHandle** out,
    DarwinArtElfErrorBuffer*) {
  (void)android::SnapshotCachedElfSonames(1);
  *out = new DarwinArtElfGraphHandle(*source);
  ++clones;
  return DARWIN_ART_ELF_OK;
}
extern "C" DarwinArtElfStatus darwin_art_elf_graph_unload(
    DarwinArtElfGraphHandle** handle, DarwinArtElfErrorBuffer*) {
  // Destructors can enter loader operations without deadlocking the cache.
  (void)android::SnapshotCachedElfSonames(1);
  delete *handle;
  *handle = nullptr;
  ++drops;
  return DARWIN_ART_ELF_OK;
}
extern "C" DarwinArtElfStatus darwin_art_elf_graph_lookup_root_symbol(
    DarwinArtElfGraphHandle* handle, const char* name, uintptr_t* out,
    DarwinArtElfErrorBuffer*) {
  (void)android::SnapshotCachedElfSonames(1);
  if (remove_during_lookup != nullptr) {
    auto* source = remove_during_lookup;
    remove_during_lookup = nullptr;
    const int before = drops;
    android::UnregisterCachedElfGraph(1, "libsame.so", source);
    assert(drops == before); // The active lookup, not cache membership, owns it.
  }
  if (std::strcmp(name, "value") != 0) return DARWIN_ART_ELF_SYMBOL_NOT_FOUND;
  *out = handle->value;
  return DARWIN_ART_ELF_OK;
}
int main() {
  using namespace android;
  DarwinArtElfGraphHandle a{11}, b{22}, rejected{33};
  std::string error;
  assert(RegisterCachedElfGraph(1, "libsame.so", &a, &error));
  assert(RegisterCachedElfGraph(2, "libsame.so", &b, &error));
  assert(RegisterCachedElfGraph(1, "libsame.so", &a, &error));
  assert(clones == 2);
  UnregisterCachedElfGraph(1, "libsame.so", &rejected);
  assert(drops == 0);
  assert(SnapshotCachedElfSonames(0).empty());
  assert(SnapshotCachedElfSonames(1).size() == 1);
  const char* needed[] = {"libsame.so"};
  DarwinArtElfSymbolRequest request{};
  request.symbol = "value";
  request.needed_libraries = needed;
  request.needed_library_count = 1;
  uintptr_t value = 0;
  assert(ResolveCachedElfProvider(1, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
  assert(value == 11);
  assert(ResolveCachedElfProvider(2, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
  assert(value == 22);
  assert(ResolveCachedElfProvider(0, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_NOT_FOUND);
  request.version_soname = "libsame.so";
  assert(ResolveCachedElfProvider(1, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
  assert(value == 11);
  assert(RegisterCachedElfGraph(1, "libsame.so", &rejected, &error));
  UnregisterCachedElfGraph(1, "libsame.so", &a);
  assert(ResolveCachedElfProvider(1, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
  assert(value == 33);
  UnregisterCachedElfGraph(1, "libsame.so", &rejected);
  assert(SnapshotCachedElfSonames(1).empty());
  assert(ResolveCachedElfProvider(2, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
  UnregisterCachedElfGraph(2, "libsame.so", &b);
  assert(clones == drops);
  assert(RegisterCachedElfGraph(1, "libsame.so", &a, &error));
  remove_during_lookup = &a;
  assert(ResolveCachedElfProvider(1, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
  assert(value == 11);
  assert(SnapshotCachedElfSonames(1).empty());
  assert(clones == drops);
  request.needed_libraries = nullptr;
  assert(ResolveCachedElfProvider(1, &request, &value, nullptr) == DARWIN_ART_ELF_RESOLVE_ERROR);
  std::cout << "ELF namespace cache contract PASS\n";
}
