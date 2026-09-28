#pragma once

#include <jni.h>

namespace aim_graphics {
struct GraphicsState;
}

namespace aim_graphics_phase {

// Completes the Android-owned content presentation step after Activity setup.
// The heavy RenderNode/Metal implementation remains in runtime_graphics_probe;
// this phase only owns validation and the short-lived JNI orchestration.
int present_and_retain(aim_graphics::GraphicsState* state,
                       JNIEnv* env, jobject decor_view,
                       jclass content_root_class, jobject content_root,
                       jclass probe_view_class, jobject probe_view,
                       bool run_apk_app, bool expect_apk_widgets,
                       bool retain_interactive, jint width, jint height);

}  // namespace aim_graphics_phase
