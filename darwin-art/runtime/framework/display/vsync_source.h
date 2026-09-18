#pragma once

#include <cstdint>

#include "../../embedding/graphics_state.h"

namespace darwin_art_graphics::display {

// Publishes one Android display edge to DisplayEventReceiver/Choreographer.
// Completed-target scanout belongs to the independent AppKit display actor;
// detached fixture presentation is not part of this production boundary.
int32_t pump_frame(GraphicsState* state, int64_t frame_time_nanos);

}  // namespace darwin_art_graphics::display
