#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include "loader/angle_image.h"
#include <cassert>
#include <cstdio>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);

void TestSystemEgl(darwin_art::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libEGL.so", 2, &request);
  assert(library);
  auto get_error = reinterpret_cast<int (*)()>(darwin_art_linker_dlsym(library, "eglGetError"));
  assert(get_error && get_error() == 0x3000); // Actual ANGLE EGL_SUCCESS.
  assert(!darwin_art_linker_dlsym(library, "malloc"));
  assert(darwin_art_linker_dlerror());
  assert(!darwin_art_linker_dlsym(library, "darwin_art_angle_dso_symbol"));
  assert(darwin_art_linker_dlerror());
  assert(!darwin_art_linker_dlsym(library, "eglDarwinArtMissingSymbol"));
  assert(darwin_art_linker_dlerror());
  LinkerImageLease* image = nullptr;
  assert(owner.FindResident(request.library_namespace, "libEGL.so", &image) == 0);
  assert(darwin_art::loader::IsAngleImage(image));
  void* payload = reinterpret_cast<void*>(uintptr_t{1});
  assert(darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_MACHO, &payload) == -4);
  assert(!payload); // Never expose an EGL owner as a host dlopen handle.
  assert(darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_ANGLE, &payload) == 0);
  assert(payload);
  uintptr_t address = 1;
  std::string error;
  assert(darwin_art::loader::ResolveAngleImage(image, "eglGetError", "INVALID_VERSION",
      &address, &error) == 1 && address == 0);
  darwin_art_linker_image_release(image);
  assert(darwin_art_linker_dlclose(library) == 0);
  void* reopened = android_dlopen_ext("libEGL.so", 2, &request);
  assert(reopened);
  assert(darwin_art_linker_dlsym(reopened, "eglGetError") == reinterpret_cast<void*>(get_error));
  assert(darwin_art_linker_dlclose(reopened) == 0);
  std::puts("system EGL: actual ANGLE dispatch, symbol isolation/version, resident reopen PASS");
}
