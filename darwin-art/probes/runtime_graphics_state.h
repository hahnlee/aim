#pragma once

#include <jni.h>

#include <array>
#include <chrono>
#include <cstdint>
#include <memory>

struct DarwinArtSurface;

#if defined(DARWIN_ART_REAL_GRAPHICS)
namespace android::uirenderer {
class AnimationContext;
namespace renderthread {
class TimeLord;
}  // namespace renderthread
}  // namespace android::uirenderer
#endif

namespace darwin_art_graphics {

// All JNI/HWUI references belong to one GraphicsSession. The session owns
// this object as a single ABI unit; every native consumer includes this header
// and must be rebuilt when its layout changes. Probe/input/phase code only
// receives a borrowed pointer while running on the ART owner thread. The
// incremental graph treats this header as an ABI dependency of the session
// allocator and every field-accessing translation unit.
struct GraphicsState {
  struct MainLooperCache {
    jclass looper_class = nullptr;
    jclass queue_class = nullptr;
    jclass message_class = nullptr;
    jclass handler_class = nullptr;
    jclass clock_class = nullptr;
    jmethodID my_queue = nullptr;
    jmethodID queue_next = nullptr;
    jfieldID queue_messages = nullptr;
    jfieldID message_when = nullptr;
    jfieldID message_what = nullptr;
    jfieldID message_next = nullptr;
    jfieldID message_target = nullptr;
    jfieldID message_callback = nullptr;
    jmethodID is_asynchronous = nullptr;
    jmethodID recycle = nullptr;
    jmethodID dispatch = nullptr;
    jmethodID uptime_millis = nullptr;
  } main_looper;
  jclass probe_canvas_class = nullptr;
  jobject interactive_root = nullptr;
  jobject interactive_view_root = nullptr;
  jobject focused_view_root = nullptr;
  jobject hardware_context = nullptr;
  jobject gpu_render_node = nullptr;
  bool gpu_render_node_recorded = false;
  jint gpu_last_traversal_barrier = -1;
  bool gpu_ripple_overlay_active = false;
  jfloat gpu_ripple_overlay_x = 0.0f;
  jfloat gpu_ripple_overlay_y = 0.0f;
  std::chrono::steady_clock::time_point gpu_ripple_overlay_started{};
  jobject pressed_view = nullptr;
  jobject pointer_dispatch_root = nullptr;
  jobject pointer_dispatch_view_root = nullptr;
  jfloat pointer_dispatch_offset_x = 0.0f;
  jfloat pointer_dispatch_offset_y = 0.0f;
  bool pointer_dispatch_is_window = false;
  bool pointer_dispatch_outside_only = false;
  uint32_t pending_pressed_action = 0;
  jfloat pending_pressed_x = 0.0f;
  jfloat pending_pressed_y = 0.0f;
  int64_t pointer_down_time_nanos = 0;
  std::array<int64_t, 512> key_down_time_nanos{};
  bool pointer_stream_active = false;
  jfloat pointer_down_x = 0.0f;
  jfloat pointer_down_y = 0.0f;
  jint pointer_touch_slop = 8;
  bool pointer_click_candidate = false;
  jint interactive_width = 0;
  jint interactive_height = 0;
  // Borrowed from the surface owner. GraphicsState never destroys this.
  DarwinArtSurface* gpu_surface = nullptr;
  // Set once the Android owner Looper wake token is attached to the AppKit
  // surface input mailbox. The callback is cleared during shutdown.
  bool owner_wake_bound = false;
#if defined(DARWIN_ART_REAL_GRAPHICS)
  std::unique_ptr<::android::uirenderer::renderthread::TimeLord> hwui_time_lord;
  std::unique_ptr<::android::uirenderer::AnimationContext> hwui_animation_context;
#endif

  GraphicsState();
  GraphicsState(const GraphicsState&) = delete;
  GraphicsState& operator=(const GraphicsState&) = delete;
  ~GraphicsState();
};

// Graphics state is process-local and is deliberately kept behind this ABI
// boundary.  The presentation, input, and shutdown translation units only
// borrow these slots; this TU owns their storage and cleanup order.
void set_probe_canvas_class(GraphicsState* state, JNIEnv* env, jclass canvas_class);
bool retain_interactive_root(GraphicsState* state, JNIEnv* env, jobject root,
                             jint width, jint height);
bool retain_interactive_view_root(GraphicsState* state, JNIEnv* env,
                                  jobject view_root);
bool retain_hardware_context(GraphicsState* state, JNIEnv* env, jobject context);
void shutdown(GraphicsState* state, JNIEnv* env);

#if defined(DARWIN_ART_REAL_GRAPHICS)
::android::uirenderer::renderthread::TimeLord* hwui_time_lord_for_state(GraphicsState* state);
::android::uirenderer::AnimationContext* hwui_animation_context_for_state(GraphicsState* state);
#endif

}  // namespace darwin_art_graphics
