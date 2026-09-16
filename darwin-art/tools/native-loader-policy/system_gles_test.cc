#include "loader/namespace_handles.h"
#include "loader/android_dlext_types.h"
#include "loader/angle_image.h"
#include <cassert>
#include <cstdio>
#include <cstring>

extern "C" void* darwin_art_linker_dlsym(void*, const char*);
extern "C" int darwin_art_linker_dlclose(void*);
namespace {
template <typename T> T Symbol(void* image, const char* name) {
  auto address = reinterpret_cast<T>(darwin_art_linker_dlsym(image, name));
  assert(address);
  return address;
}
}

// Exercise both admitted Android images on actual ANGLE/Metal. The tiny readback
// is a test observation only, never a runtime presentation or CPU fallback path.
static void TestVersion(darwin_art::loader::NamespaceHandles& owner, int version) {
  android_dlextinfo request{};
  request.flags = ANDROID_DLEXT_USE_NAMESPACE;
  request.library_namespace = owner.Exported("default");
  void* egl = android_dlopen_ext("libEGL.so", 2, &request);
  void* gl = android_dlopen_ext(version == 1 ? "libGLESv1_CM.so" :
      version == 3 ? "libGLESv3.so" : "libGLESv2.so", 2, &request);
  assert(egl && gl);
  assert(!darwin_art_linker_dlsym(gl, "eglGetError"));
  assert(darwin_art_linker_dlerror());
  assert(!darwin_art_linker_dlsym(egl, "glClearColor"));
  assert(darwin_art_linker_dlerror());
  auto display = Symbol<void* (*)(void*)>(egl, "eglGetDisplay")(nullptr);
  assert(display);
  int major = 0, minor = 0;
  assert((Symbol<unsigned (*)(void*, int*, int*)>(egl, "eglInitialize")(display, &major, &minor)));
  const int config_attributes[] = {0x3033, 1, 0x3040, version == 1 ? 1 : version == 3 ? 0x40 : 4, 0x3024, 8,
      0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3038};
  void* config = nullptr;
  int count = 0;
  assert((Symbol<unsigned (*)(void*, const int*, void**, int, int*)>(egl, "eglChooseConfig")(
      display, config_attributes, &config, 1, &count)) && count == 1);
  const int context_attributes[] = {0x3098, version, 0x3038};
  auto context = Symbol<void* (*)(void*, void*, void*, const int*)>(egl, "eglCreateContext")(
      display, config, nullptr, context_attributes);
  assert(context);
  const int surface_attributes[] = {0x3057, 4, 0x3056, 4, 0x3038};
  auto surface = Symbol<void* (*)(void*, void*, const int*)>(egl, "eglCreatePbufferSurface")(
      display, config, surface_attributes);
  assert(surface);
  auto make_current = Symbol<unsigned (*)(void*, void*, void*, void*)>(egl, "eglMakeCurrent");
  assert(make_current(display, surface, surface, context));
  auto renderer = Symbol<const unsigned char* (*)(unsigned)>(gl, "glGetString")(0x1f01);
  assert(renderer && std::strstr(reinterpret_cast<const char*>(renderer), "Metal"));
  if (version == 3) {
    int actual_major = 0;
    Symbol<void (*)(unsigned, int*)>(gl, "glGetIntegerv")(0x821b, &actual_major);
    assert(actual_major >= 3);
    unsigned array = 0;
    Symbol<void (*)(int, unsigned*)>(gl, "glGenVertexArrays")(1, &array);
    Symbol<void (*)(unsigned)>(gl, "glBindVertexArray")(array);
    assert(Symbol<unsigned char (*)(unsigned)>(gl, "glIsVertexArray")(array));
    Symbol<void (*)(unsigned)>(gl, "glBindVertexArray")(0);
    Symbol<void (*)(int, const unsigned*)>(gl, "glDeleteVertexArrays")(1, &array);
  }
  Symbol<void (*)(float, float, float, float)>(gl, "glClearColor")(0, 1, 0, 1);
  Symbol<void (*)(unsigned)>(gl, "glClear")(0x4000);
  if (version == 1) {
    auto text = Symbol<const unsigned char* (*)(unsigned)>(gl, "glGetString")(0x1f02);
    assert(text && (std::strstr(reinterpret_cast<const char*>(text), "OpenGL ES-CM 1.") ||
        std::strstr(reinterpret_cast<const char*>(text), "OpenGL ES 1.")));
    int actual_version = 0;
    assert((Symbol<unsigned (*)(void*, void*, int, int*)>(egl, "eglQueryContext")(
        display, context, 0x3098, &actual_version)) && actual_version == 1);
    LinkerImageLease* image = nullptr;
    assert(owner.FindResident(request.library_namespace, "libGLESv1_CM.so", &image) == 0);
    uintptr_t address = 0;
    std::string error;
    assert(darwin_art::loader::ResolveAngleImage(image, "glMatrixMode", "LIBGLESV1_CM",
        &address, &error) == 0 && address);
    assert(darwin_art::loader::ResolveAngleImage(image, "glMatrixMode", "WRONG_VERSION",
        &address, &error) == 1 && !address);
    darwin_art_linker_image_release(image);
    assert(!darwin_art_linker_dlsym(gl, "glCreateShader"));
    assert(darwin_art_linker_dlerror());
    // Fixed-function geometry must overwrite a red buffer, not merely clear it.
    Symbol<void (*)(float, float, float, float)>(gl, "glClearColor")(1, 0, 0, 1);
    Symbol<void (*)(unsigned)>(gl, "glClear")(0x4000);
    Symbol<void (*)(unsigned)>(gl, "glMatrixMode")(0x1701);
    Symbol<void (*)()>(gl, "glLoadIdentity")();
    Symbol<void (*)(unsigned)>(gl, "glMatrixMode")(0x1700);
    Symbol<void (*)()>(gl, "glLoadIdentity")();
    Symbol<void (*)(float, float, float, float)>(gl, "glColor4f")(0, 1, 0, 1);
    const float vertices[] = {-1,-1, 1,-1, -1,1, 1,1};
    Symbol<void (*)(unsigned)>(gl, "glEnableClientState")(0x8074);
    Symbol<void (*)(int, unsigned, int, const void*)>(gl, "glVertexPointer")(2, 0x1406, 0, vertices);
    Symbol<void (*)(unsigned, int, int)>(gl, "glDrawArrays")(0x0005, 0, 4);
    Symbol<void (*)(unsigned)>(gl, "glDisableClientState")(0x8074);
  }
  unsigned char pixels[4 * 4 * 4]{};
  Symbol<void (*)(int, int, int, int, unsigned, unsigned, void*)>(gl, "glReadPixels")(
      0, 0, 4, 4, 0x1908, 0x1401, pixels);
  assert(Symbol<unsigned (*)()>(gl, "glGetError")() == 0);
  for (size_t i = 0; i < sizeof(pixels); i += 4) {
    assert(pixels[i] == 0 && pixels[i + 1] == 255 && pixels[i + 2] == 0 && pixels[i + 3] == 255);
  }
  std::printf("system GLES%d: Metal renderer=%s, 16 actual green pixels PASS\n", version, renderer);
  assert(make_current(display, nullptr, nullptr, nullptr));
  assert((Symbol<unsigned (*)(void*, void*)>(egl, "eglDestroySurface")(display, surface)));
  assert((Symbol<unsigned (*)(void*, void*)>(egl, "eglDestroyContext")(display, context)));
  assert((Symbol<unsigned (*)(void*)>(egl, "eglTerminate")(display)));
  assert(darwin_art_linker_dlclose(gl) == 0 && darwin_art_linker_dlclose(egl) == 0);
}

void TestSystemGles(darwin_art::loader::NamespaceHandles& owner) {
  TestVersion(owner, 1);
  TestVersion(owner, 2);
  TestVersion(owner, 3);
}
