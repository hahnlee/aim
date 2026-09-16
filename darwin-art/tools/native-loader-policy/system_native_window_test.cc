#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include "loader/native_window_image.h"
#include <cassert>
#include <cstdio>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);
void TestWindowFrameRate(void*);

void TestSystemNativeWindow(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libnativewindow.so", 2, &request);
  assert(library);
  TestWindowFrameRate(library); // Actual facade arguments and producer errors.
  for (const char* denied : {"malloc", "AAssetManager_open", "ASurfaceTransaction_create",
                            "ANativeWindow_clearFrameRate", "darwin_art_platform_symbol"}) {
    assert(!darwin_art_linker_dlsym(library, denied));
    assert(darwin_art_linker_dlerror());
  }
  auto acquire = darwin_art_linker_dlsym(library, "ANativeWindow_acquire");
  assert(acquire);
  LinkerImageLease* image = nullptr;
  assert(owner.FindResident(request.library_namespace, "libnativewindow.so", &image) == 0);
  assert(darwin_art::loader::IsNativeWindowImage(image));
  void* payload = reinterpret_cast<void*>(uintptr_t{1});
  assert(darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_MACHO, &payload) == -4);
  assert(!payload);
  assert(darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_NATIVE_WINDOW, &payload) == 0);
  assert(payload);
  uintptr_t address = 1;
  std::string error;
  assert(darwin_art::loader::ResolveNativeWindowImage(image, "ANativeWindow_acquire",
      "INVALID_VERSION", &address, &error) == 1 && address == 0);
  darwin_art_linker_image_release(image);
  assert(darwin_art_linker_dlclose(library) == 0);
  auto reopened = android_dlopen_ext("libnativewindow.so", 2, &request);
  assert(reopened);
  assert(darwin_art_linker_dlsym(reopened, "ANativeWindow_acquire") == acquire);
  assert(darwin_art_linker_dlclose(reopened) == 0);
  std::puts("system NativeWindow: distinct typed owner, actual facade, isolation, reopen PASS");
}
