#pragma once
#include "darwin_surface_bridge.h"

#include <cstdint>

#include "darwin_art/darwin_art.h"
#include "../../embedding/graphics_state.h"

namespace darwin_art_graphics::input {

// Android-owned input ingress.  Host adapters submit complete Android
// MotionEvent/KeyEvent packets here; ViewRootImpl's InputChannel and input
// stages remain the event target.  This API deliberately has no view-tree
// traversal or fixture click fallback.
int32_t dispatch_pointer(GraphicsState* state, uint32_t action, float x,
                         float y);
int32_t dispatch_pointer_v2(GraphicsState* state,
                            const DarwinArtPointerEventV2* event);
int32_t dispatch_key_v1(GraphicsState* state,
                        const DarwinArtKeyEventV1* event);
int32_t pump_main_looper(GraphicsState* state);

// Installs the Android-owned ingress sink on a host surface. The Darwin
// surface provider owns only the generic retained binding; these callbacks
// own InputChannel routing and its terminal no-target/backpressure status.
// Installation requires ownership of a live surface excluding concurrent
// destruction. The binding retains that exact desktop root, not the surface.
// Root retention is identity only; it does not establish a focus grant.
extern "C" DarwinArtSurfaceResult darwin_art_android_input_sink_install(
    DarwinArtSurface* surface);

}  // namespace darwin_art_graphics::input
