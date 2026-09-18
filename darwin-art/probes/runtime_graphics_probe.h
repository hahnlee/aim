#pragma once

#include <jni.h>
#include <cstdint>

#include "darwin_surface_bridge.h"
#include "../runtime/embedding/graphics_state.h"

namespace darwin_art_graphics {
jboolean present_content(GraphicsState* state, JNIEnv* env, jclass unused,
                         jobject view, jint width, jint height);
void debug_view_text_state(JNIEnv* env, jobject root);
int32_t dispatch_pointer(GraphicsState* state, uint32_t action, float x, float y);
int32_t dispatch_pointer_v2(GraphicsState* state,
                            const DarwinArtPointerEventV2* event);
int32_t dispatch_key_v1(GraphicsState* state,
                        const DarwinArtKeyEventV1* event);
int32_t pump_frame(GraphicsState* state, jlong frame_time_nanos);
int32_t pump_main_looper(GraphicsState* state);
}  // namespace darwin_art_graphics

extern "C" int32_t darwin_art_dispatch_pointer(uint32_t action, float x,
                                                 float y);
extern "C" int32_t darwin_art_dispatch_pointer_v2(
    const DarwinArtPointerEventV2* event);
extern "C" int32_t darwin_art_dispatch_key_v1(
    const DarwinArtKeyEventV1* event);
extern "C" int32_t darwin_art_pump_framework_frame(jlong frame_time_nanos);
