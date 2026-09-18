#pragma once

#include <cstdint>

#include "darwin_art/darwin_art.h"
#include "darwin_surface_bridge.h"
#include "graphics_state.h"

namespace art {
class Thread;
}

namespace darwin_art_graphics {

// Binds the pre-created opaque handle to the one process run. These calls are
// made on the ART owner thread; no JNI/HWUI object crosses the ABI.
int32_t bind_session_for_process(void* context);
int32_t bind_session_art_thread(art::Thread* thread);
GraphicsState* state_for_context(void* context);

// Session state is an opaque owner-thread token. GraphicsState contains only
// input-stream timing; Android framework/rendering owners hold JNI/HWUI state.
darwin_art_graphics_session_t* create_session();
int32_t close_session(darwin_art_graphics_session_t* session);
// Shutdown's read-only preflight; it does not transition the session.
bool bound_session_quiescent(GraphicsState* state);
// Marks the bound session finalized once shutdown has proven it closed and
// quiescent, before input timing reset and DestroyJavaVM. Rust may then drop the
// opaque owner after VM teardown without re-entering ART.
int32_t finalize_bound_session(GraphicsState* state);
int32_t destroy_session(darwin_art_graphics_session_t* session);
int32_t dispatch_pointer(darwin_art_graphics_session_t* session,
                         uint32_t action, float x, float y);
int32_t dispatch_pointer_v2(darwin_art_graphics_session_t* session,
                            const DarwinArtPointerEventV2* event);
int32_t dispatch_key_v1(darwin_art_graphics_session_t* session,
                        const DarwinArtKeyEventV1* event);
int32_t pump_main_looper(darwin_art_graphics_session_t* session);
int32_t wait_main_looper(darwin_art_graphics_session_t* session,
                         int32_t timeout_ms);
int32_t pump_frame(darwin_art_graphics_session_t* session,
                   int64_t frame_time_nanos);

}  // namespace darwin_art_graphics
