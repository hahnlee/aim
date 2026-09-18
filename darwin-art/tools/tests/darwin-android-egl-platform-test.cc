#include "compat/darwin_angle_egl.h"
#include "compat/graphics/egl_error_state.h"

#include <cassert>
#include <cstdint>

namespace {
void* g_display = nullptr;
void* g_config = nullptr;
void* g_window = nullptr;
int g_initialize_calls = 0;
int g_terminate_calls = 0;
int g_create_calls = 0;
}

namespace darwin_art {

std::uint32_t EglInitializeHost(void* display, std::int32_t* major,
                                std::int32_t* minor) {
  ++g_initialize_calls;
  g_display = display;
  if (major != nullptr) *major = 1;
  if (minor != nullptr) *minor = 5;
  return 1;
}

std::uint32_t TerminateHostDisplay(void* display) {
  ++g_terminate_calls;
  assert(display == g_display);
  return 1;
}

}  // namespace darwin_art

extern "C" void* darwin_art_android_eglCreateWindowSurface(
    void* display, void* config, void* window, const std::int32_t*) {
  ++g_create_calls;
  assert(display == g_display);
  g_config = config;
  g_window = window;
  return reinterpret_cast<void*>(0x55);
}
extern "C" std::uint32_t darwin_art_android_eglSwapBuffers(void*, void*) {
  return 1;
}
extern "C" std::uint32_t darwin_art_android_eglDestroySurface(void*, void*) {
  return 1;
}
extern "C" std::uint32_t darwin_art_android_eglMakeCurrent(void*, void*, void*,
                                                             void*) {
  return 1;
}
extern "C" std::uint32_t darwin_art_android_eglQuerySurface(
    void*, void*, std::int32_t, std::int32_t*) {
  return 1;
}
extern "C" std::uint32_t darwin_art_android_eglSurfaceAttrib(
    void*, void*, std::int32_t, std::int32_t) {
  return 1;
}
extern "C" std::uint32_t darwin_art_android_eglSwapInterval(void*,
                                                              std::int32_t) {
  return 1;
}
extern "C" std::uint32_t darwin_art_android_eglSetDamageRegion(
    void*, void*, const std::int32_t*, std::int32_t) {
  return 1;
}
const char* darwin_art::EglQueryStringAndroid(void*, std::int32_t) {
  return "";
}

extern "C" std::uint32_t eglInitialize(void*, std::int32_t*, std::int32_t*);
extern "C" std::uint32_t eglTerminate(void*);
extern "C" void* eglCreateWindowSurface(void*, void*, void*,
                                          const std::int32_t*);
extern "C" std::int32_t eglGetError();
extern "C" std::int32_t darwin_art_android_eglGetError();

int main() {
  auto* display = reinterpret_cast<void*>(0x10);
  auto* config = reinterpret_cast<void*>(0x20);
  auto* window = reinterpret_cast<void*>(0x30);
  std::int32_t major = 0;
  std::int32_t minor = 0;
  assert(eglInitialize(display, &major, &minor) == 1);
  assert(major == 1 && minor == 5 && g_initialize_calls == 1);
  assert(eglCreateWindowSurface(display, config, window, nullptr) ==
         reinterpret_cast<void*>(0x55));
  assert(g_create_calls == 1 && g_config == config && g_window == window);
  darwin_art::graphics::SetEglError(0x3006);
  assert(darwin_art_android_eglGetError() == 0x3006);
  darwin_art::graphics::SetEglError(0x3009);
  assert(eglGetError() == 0x3009);
  assert(eglGetError() == darwin_art::graphics::kEglSuccess);
  assert(eglTerminate(display) == 1 && g_terminate_calls == 1);
}
