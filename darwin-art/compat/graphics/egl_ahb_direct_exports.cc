#include "darwin_angle_egl.h"

#include <cstdint>

namespace {
template <typename Function>
Function AndroidEgl(const char* symbol) {
  return reinterpret_cast<Function>(
      darwin_art::darwin_art_angle_dso_symbol("libEGL.so", symbol));
}
}  // namespace

// AOSP HWUI/Skia links these Android EGL extensions directly. Route those
// relocations through the same Android EGL dispatch used by guest DSOs so an
// AHardwareBuffer is imported from its IOSurface instead of passed to ANGLE's
// native Darwin libEGL entry points.
extern "C" void* eglGetNativeClientBufferANDROID(AHardwareBuffer* buffer) {
  auto function = AndroidEgl<void* (*)(AHardwareBuffer*)>(
      "eglGetNativeClientBufferANDROID");
  return function == nullptr ? nullptr : function(buffer);
}

extern "C" void* eglCreateImageKHR(void* display, void* context,
                                    std::uint32_t target, void* client_buffer,
                                    const std::int32_t* attributes) {
  auto function = AndroidEgl<void* (*)(void*, void*, std::uint32_t, void*,
                                       const std::int32_t*)>("eglCreateImageKHR");
  return function == nullptr
             ? nullptr
             : function(display, context, target, client_buffer, attributes);
}

extern "C" std::uint32_t eglDestroyImageKHR(void* display, void* image) {
  auto function = AndroidEgl<std::uint32_t (*)(void*, void*)>(
      "eglDestroyImageKHR");
  return function == nullptr ? 0 : function(display, image);
}

extern "C" void glEGLImageTargetTexture2DOES(std::uint32_t target,
                                                void* image) {
  darwin_art::darwin_art_android_egl_bind_direct_image_texture(target, image);
}
