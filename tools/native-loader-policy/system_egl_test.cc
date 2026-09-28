#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include "loader/angle_image.h"
#include <cassert>
#include <cstdio>

extern "C" void* aim_linker_dlsym(void*, const char*);
extern "C" int aim_linker_dlclose(void*);

void TestSystemEgl(aim::loader::NamespaceHandles& owner) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* library = android_dlopen_ext("libEGL.so", 2, &request);
  assert(library);
  auto get_error = reinterpret_cast<int (*)()>(aim_linker_dlsym(library, "eglGetError"));
  assert(get_error && get_error() == 0x3000); // Actual ANGLE EGL_SUCCESS.
  assert(!aim_linker_dlsym(library, "malloc"));
  assert(aim_linker_dlerror());
  assert(!aim_linker_dlsym(library, "aim_angle_dso_symbol"));
  assert(aim_linker_dlerror());
  assert(!aim_linker_dlsym(library, "eglAimMissingSymbol"));
  assert(aim_linker_dlerror());
  LinkerImageLease* image = nullptr;
  assert(owner.FindResident(request.library_namespace, "libEGL.so", &image) == 0);
  assert(aim::loader::IsAngleImage(image));
  void* payload = reinterpret_cast<void*>(uintptr_t{1});
  assert(aim_linker_image_typed_payload(image, AIM_IMAGE_MACHO, &payload) == -4);
  assert(!payload); // Never expose an EGL owner as a host dlopen handle.
  assert(aim_linker_image_typed_payload(image, AIM_IMAGE_ANGLE, &payload) == 0);
  assert(payload);
  uintptr_t address = 1;
  std::string error;
  assert(aim::loader::ResolveAngleImage(image, "eglGetError", "INVALID_VERSION",
      &address, &error) == 1 && address == 0);
  aim_linker_image_release(image);
  assert(aim_linker_dlclose(library) == 0);
  void* reopened = android_dlopen_ext("libEGL.so", 2, &request);
  assert(reopened);
  assert(aim_linker_dlsym(reopened, "eglGetError") == reinterpret_cast<void*>(get_error));
  assert(aim_linker_dlclose(reopened) == 0);
  std::puts("system EGL: actual ANGLE dispatch, symbol isolation/version, resident reopen PASS");
}
