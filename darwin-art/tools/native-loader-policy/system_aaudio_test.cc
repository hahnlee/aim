#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include <cassert>
#include <cstdio>
extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);
void TestSystemAAudio(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libaaudio.so", 0x1002, &request);
  assert(library);
  auto create = reinterpret_cast<int32_t(*)(void**)>(darwin_art_linker_dlsym(library, "AAudio_createStreamBuilder"));
  auto destroy = reinterpret_cast<int32_t(*)(void*)>(darwin_art_linker_dlsym(library, "AAudioStreamBuilder_delete"));
  assert(create && destroy);
  assert(create(nullptr) < 0 && destroy(nullptr) < 0);
  void* builder = nullptr;
  assert(create(&builder) == 0 && builder);
  assert(destroy(builder) == 0);
  assert(!darwin_art_linker_dlsym(library, "ASurfaceTransaction_create"));
  assert(darwin_art_linker_dlerror());
  assert(darwin_art_linker_dlclose(library) == 0);
  std::puts("system AAudio: real builder allocation/deletion, null rejection, SONAME isolation PASS");
}
