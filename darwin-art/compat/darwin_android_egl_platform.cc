#include "darwin_angle_egl.h"

#include "graphics/egl_error_state.h"

#include <cstdint>

// Standard EGL entry points are supplied by the project ANGLE dylibs.  These
// surface operations are the Android libEGL dispatch boundary: an Android
// ANativeWindow is a BufferQueue producer, not the CALayer expected by ANGLE's
// Darwin window backend.  The bridge renders through ANGLE into the dequeued
// IOSurface-backed AHardwareBuffer and queues it to Android composition.
extern "C" void* eglCreateWindowSurface(void* display, void* config,
                                         void* native_window,
                                         const std::int32_t* attributes) {
  return darwin_art_android_eglCreateWindowSurface(
      display, config, native_window, attributes);
}

extern "C" std::int32_t darwin_art_android_eglGetError() {
  return darwin_art::graphics::ConsumeEglError();
}

extern "C" std::int32_t eglGetError() {
  return darwin_art_android_eglGetError();
}

extern "C" std::uint32_t eglInitialize(void* display, std::int32_t* major,
                                        std::int32_t* minor) {
  return darwin_art::EglInitializeHost(display, major, minor);
}

extern "C" std::uint32_t eglTerminate(void* display) {
  return darwin_art::TerminateHostDisplay(display);
}

extern "C" std::uint32_t eglSwapBuffers(void* display, void* surface) {
  return darwin_art_android_eglSwapBuffers(display, surface);
}

extern "C" std::uint32_t eglSwapBuffersWithDamageKHR(
    void* display, void* surface, const std::int32_t*, std::int32_t) {
  return darwin_art_android_eglSwapBuffers(display, surface);
}

extern "C" std::uint32_t eglSwapBuffersWithDamageEXT(
    void* display, void* surface, const std::int32_t*, std::int32_t) {
  return darwin_art_android_eglSwapBuffers(display, surface);
}

extern "C" std::uint32_t eglDestroySurface(void* display, void* surface) {
  return darwin_art_android_eglDestroySurface(display, surface);
}

extern "C" std::uint32_t eglMakeCurrent(void* display, void* draw, void* read,
                                         void* context) {
  return darwin_art_android_eglMakeCurrent(display, draw, read, context);
}

extern "C" std::uint32_t eglQuerySurface(void* display, void* surface,
                                          std::int32_t attribute,
                                          std::int32_t* value) {
  return darwin_art_android_eglQuerySurface(display, surface, attribute, value);
}

extern "C" std::uint32_t eglSurfaceAttrib(void* display, void* surface,
                                           std::int32_t attribute,
                                           std::int32_t value) {
  return darwin_art_android_eglSurfaceAttrib(display, surface, attribute, value);
}

extern "C" std::uint32_t eglSwapInterval(void* display,
                                          std::int32_t interval) {
  return darwin_art_android_eglSwapInterval(display, interval);
}

extern "C" std::uint32_t eglSetDamageRegionKHR(
    void* display, void* surface, const std::int32_t* rects,
    std::int32_t count) {
  return darwin_art_android_eglSetDamageRegion(display, surface, rects, count);
}

extern "C" const char* eglQueryString(void* display, std::int32_t name) {
  return darwin_art::EglQueryStringAndroid(display, name);
}

// Android's libEGL adds this advisory frame boundary for HWUI tracing.  ANGLE
// does not export it on Darwin; rendering and synchronization remain owned by
// eglSwapBuffersWithDamageKHR and the native-fence bridge.
void eglBeginFrame(void* display, void* surface) {
  (void)display;
  (void)surface;
}
