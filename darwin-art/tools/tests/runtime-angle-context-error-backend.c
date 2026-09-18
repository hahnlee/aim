// Test-only ANGLE-shaped provider. It supplies only deterministic EGL error
// state; no production graphics object or host ANGLE library is reused.
#include <stdint.h>
#include <stdlib.h>

static _Thread_local uint32_t error_code;

__attribute__((visibility("default"))) void* eglCreateContext(
    void* display, void* config, void* share, const int32_t* attributes) {
  (void)display;
  (void)config;
  (void)share;
  (void)attributes;
  const char* requested = getenv("DARWIN_ART_TEST_EGL_ERROR");
  error_code = requested == NULL ? 0x3009u : (uint32_t)strtoul(requested, NULL, 0);
  return NULL;  // EGL_NO_CONTEXT; preserve the backend error for eglGetError.
}

__attribute__((visibility("default"))) uint32_t eglGetError(void) {
  const uint32_t result = error_code;
  error_code = 0x3000u;  // EGL_SUCCESS
  return result;
}

#define EGL_MOCK(name) __attribute__((visibility("default"))) int name(int ignored, ...) { (void)ignored; return 1; }
EGL_MOCK(eglGetDisplay)
EGL_MOCK(eglInitialize)
EGL_MOCK(eglChooseConfig)
EGL_MOCK(eglGetConfigs)
EGL_MOCK(eglGetConfigAttrib)
EGL_MOCK(eglCreatePbufferSurface)
EGL_MOCK(eglCreatePbufferFromClientBuffer)
EGL_MOCK(eglMakeCurrent)
EGL_MOCK(eglDestroyContext)
EGL_MOCK(eglDestroySurface)
EGL_MOCK(eglTerminate)
EGL_MOCK(eglSwapBuffers)
EGL_MOCK(eglBindTexImage)
EGL_MOCK(eglReleaseTexImage)
EGL_MOCK(eglQueryContext)
EGL_MOCK(eglQuerySurface)
EGL_MOCK(eglQueryString)
EGL_MOCK(eglGetCurrentDisplay)
EGL_MOCK(eglGetCurrentContext)
EGL_MOCK(eglGetCurrentSurface)
EGL_MOCK(eglGetProcAddress)
EGL_MOCK(eglReleaseThread)
EGL_MOCK(eglWaitGL)
EGL_MOCK(eglWaitNative)
#undef EGL_MOCK
