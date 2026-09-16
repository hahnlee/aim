#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include <cassert>
#include <cstdio>
extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);
void TestWindowFrameRate(void*);
void TestSystemLibandroid(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* handle = android_dlopen_ext("libandroid.so", 0x1002, &request);
  assert(handle);
  TestWindowFrameRate(handle);
  auto create = reinterpret_cast<void*(*)()>(darwin_art_linker_dlsym(handle, "ASurfaceTransaction_create"));
  auto destroy = reinterpret_cast<void(*)(void*)>(darwin_art_linker_dlsym(handle, "ASurfaceTransaction_delete"));
  assert(create && destroy);
  void* transaction = create();
  assert(transaction);
  destroy(transaction);
  assert(darwin_art_linker_dlsym(handle, "AAssetManager_open"));
  assert(darwin_art_linker_dlsym(handle, "android_res_nquery"));
  auto from_surface = reinterpret_cast<void* (*)(void*, void*)>(
      darwin_art_linker_dlsym(handle, "ANativeWindow_fromSurface"));
  assert(from_surface && from_surface(nullptr, nullptr) == nullptr);
  for (const char* symbol : {"AHardwareBuffer_fromHardwareBuffer", "AHardwareBuffer_toHardwareBuffer"}) {
    auto convert = reinterpret_cast<void* (*)(void*, void*)>(darwin_art_linker_dlsym(handle, symbol));
    assert(convert && convert(nullptr, nullptr) == nullptr);
  }
  // AOSP exposes clearFrameRate as a header inline, not a dynamic symbol.
  assert(!darwin_art_linker_dlsym(handle, "ANativeWindow_clearFrameRate"));
  assert(darwin_art_linker_dlerror());
  assert(!darwin_art_linker_dlsym(handle, "darwin_art_android_ANativeWindow_acquire"));
  assert(darwin_art_linker_dlerror());
  assert(!darwin_art_linker_dlsym(handle, "not_a_libandroid_export"));
  assert(darwin_art_linker_dlerror());
  assert(darwin_art_linker_dlclose(handle) == 0);
  std::puts("system libandroid: actual transaction create/delete, asset/DNS exports, private symbols denied PASS");
}
