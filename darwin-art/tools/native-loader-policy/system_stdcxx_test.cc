#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);

// Execute original Android operators/guard code through the selected ELF handle.
// This is not a claim that all C++ exception or thread contracts are complete.
void TestSystemStdcxx(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libstdc++.so", 2, &request);
  assert(library);
  auto allocate = reinterpret_cast<void* (*)(uint64_t)>(
      darwin_art_linker_dlsym(library, "_Znwm"));
  auto release = reinterpret_cast<void (*)(void*)>(
      darwin_art_linker_dlsym(library, "_ZdlPv"));
  auto acquire_guard = reinterpret_cast<int (*)(uint64_t*)>(
      darwin_art_linker_dlsym(library, "__cxa_guard_acquire"));
  auto release_guard = reinterpret_cast<void (*)(uint64_t*)>(
      darwin_art_linker_dlsym(library, "__cxa_guard_release"));
  auto abort_guard = reinterpret_cast<void (*)(uint64_t*)>(
      darwin_art_linker_dlsym(library, "__cxa_guard_abort"));
  assert(allocate && release && acquire_guard && release_guard && abort_guard);
  auto* bytes = static_cast<unsigned char*>(allocate(128));
  assert(bytes);
  std::memset(bytes, 0x5a, 128);
  for (size_t i = 0; i < 128; ++i) assert(bytes[i] == 0x5a);
  release(bytes);
  uint64_t guard = 0;
  assert(acquire_guard(&guard) == 1);
  abort_guard(&guard);
  assert(acquire_guard(&guard) == 1);
  release_guard(&guard);
  assert(acquire_guard(&guard) == 0);
  assert(darwin_art_linker_dlclose(library) == 0);
  std::puts("original Android libstdc++: allocation/delete, guard abort/retry/release PASS");
}
