#pragma once

#include <jni.h>

#include <array>
#include <cstdint>

namespace darwin_art_graphics {

struct GraphicsState {
  int64_t pointer_down_time_nanos = 0;
  std::array<int64_t, 512> key_down_time_nanos{};
  bool pointer_stream_active = false;

  GraphicsState();
  GraphicsState(const GraphicsState&) = delete;
  GraphicsState& operator=(const GraphicsState&) = delete;
  ~GraphicsState();
};

// Kept as an ABI-compatible call site for the process shutdown owner. The
// production state now contains only Android input-stream timestamps;
// production surface ownership belongs to the framework/rendering owners,
// not this state or a fixture owner.
void shutdown(GraphicsState* state, JNIEnv* env);

}  // namespace darwin_art_graphics
