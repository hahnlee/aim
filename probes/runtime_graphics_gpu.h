#pragma once

#include <jni.h>

namespace aim_graphics {

struct GraphicsState;

// The CAMetalLayer/NSWindow is main-thread owned.  APK Activity/View work may
// run on the attached Android UI thread, so create the drawable owner before
// handing Java ownership to that thread.
int prepare_gpu_surface(GraphicsState* state, jint width, jint height);
int refresh_gpu_surface_identity(GraphicsState* state);

// ViewRootImpl/AttachInfo must be created on the Android owner thread. The
// Metal RenderThread may call present_gpu_content later, but it must only
// consume the already-attached hierarchy.
jboolean attach_hardware_hierarchy_on_owner(GraphicsState* state, JNIEnv* env,
                                            jobject view);

jboolean present_gpu_content(GraphicsState* state, JNIEnv* env, jobject view,
                             jint width, jint height);

// Failure-only inspection of the Android-owned traversal/renderer state. It
// performs no drawing and emits output only when the debug environment gate
// is enabled.
void debug_product_view_root(JNIEnv* env, jobject view);

}  // namespace aim_graphics
