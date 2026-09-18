#include "graphics_state.h"

namespace darwin_art_graphics {

GraphicsState::GraphicsState() = default;
GraphicsState::~GraphicsState() = default;

void shutdown(GraphicsState* state, JNIEnv*) {
  if (state == nullptr) return;
  state->pointer_down_time_nanos = 0;
  state->key_down_time_nanos.fill(0);
  state->pointer_stream_active = false;
}

}  // namespace darwin_art_graphics
