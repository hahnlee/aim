#include "runtime_graphics_probe.h"

#include "aim/aim.h"

namespace art {
class Thread;
}

namespace aim_graphics {

// The headless/runtime flavor intentionally has no GraphicsSession owner.
// Keep the process ABI link closed without making the host discover a
// graphics-session capability that only the real-graphics flavor provides.
__attribute__((weak)) int32_t bind_session_for_process(void*) {
  return AIM_STATUS_GRAPHICS_SESSION_NOT_READY;
}

__attribute__((weak)) int32_t bind_session_art_thread(art::Thread*) {
  return AIM_STATUS_GRAPHICS_SESSION_NOT_READY;
}

__attribute__((weak)) GraphicsState* state_for_context(void*) {
  return nullptr;
}

__attribute__((weak)) int32_t finalize_bound_session(GraphicsState*) {
  return AIM_STATUS_GRAPHICS_SESSION_NOT_READY;
}

}  // namespace aim_graphics
