#include "egl_error_state.h"

namespace darwin_art::graphics {
namespace {

thread_local std::int32_t g_egl_error = kEglSuccess;

}  // namespace

void SetEglError(std::int32_t error) {
  // ANGLE reserves values outside the EGL error set for provider extensions;
  // preserve those values rather than translating or consuming them.
  g_egl_error = error;
}

std::int32_t PeekEglError() { return g_egl_error; }

std::int32_t ConsumeEglError() {
  const std::int32_t error = g_egl_error;
  g_egl_error = kEglSuccess;
  return error;
}

}  // namespace darwin_art::graphics
