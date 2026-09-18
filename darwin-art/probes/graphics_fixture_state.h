#pragma once

#include <jni.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <vector>

#include "../runtime/embedding/graphics_state.h"
#include "darwin_surface_bridge.h"
#include "fixture_input_queue.h"
#include "fixture_input_dispatch.h"

#if defined(DARWIN_ART_REAL_GRAPHICS)
namespace android::uirenderer {
class AnimationContext;
namespace renderthread {
class TimeLord;
}  // namespace renderthread
}  // namespace android::uirenderer
#endif

namespace darwin_art_graphics_fixture {

// TEST ONLY: channel handoff, completion, and consumption are independent.
// delivered records a complete frame submitted to the imported channel, not
// proof that Java ran. Only the finish ACK proves InputStage completion.
// A submitted event whose finish ACK is deferred is not an unhandled event.
struct FixtureInputDispatchResult final {
  bool delivered = false;
  bool completed = false;
  bool handled = false;
  bool failed = false; // Submission/owner progress exception, not unhandled.

  constexpr bool ShouldRetryUnhandled() const noexcept {
    return delivered && completed && !handled && !failed;
  }
};

// TEST ONLY: synthetic fixture ViewRoots have no WMS focus publisher. Ordinary
// APKs must use authoritative InputChannel focus controls instead.
bool set_view_root_focus(JNIEnv* env, jobject view_root, bool focused);

// Context retained by the surface input binding. The fixture owns packet
// ordering, MOVE coalescing, and pending acknowledgement; the production
// surface never inspects this queue.
struct SurfaceInputSinkContext {
  std::atomic<uint32_t> references{1};
  FixtureInputQueue queue;
};

// State used only by the fixture driver. Nothing in this structure is part of
// the installed-APK GraphicsState ABI; ownership is keyed by GraphicsState in
// this fixture-only translation unit and is released before ART teardown.
struct GraphicsFixtureState {
  GraphicsFixtureState();
  ~GraphicsFixtureState();

  jobject interactive_root = nullptr;
  std::vector<std::shared_ptr<FixtureInputEndpoint>> input_endpoints;
  bool input_endpoints_retired = false;
  std::atomic<bool> retiring{false};
  size_t active_invocations = 0; // Protected by fixture registry mutex.
  bool cleanup_started = false; // Protected by fixture registry mutex.
  jobject interactive_view_root = nullptr;
  jobject focused_view_root = nullptr;
  jobject hardware_context = nullptr;
  jint interactive_width = 0;
  jint interactive_height = 0;
  DarwinArtSurface* gpu_surface = nullptr;
  bool owner_wake_bound = false;
  SurfaceInputSinkContext* input_sink_context = nullptr;

  jclass probe_canvas_class = nullptr;
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
  jfloat pointer_down_x = 0.0f;
  jfloat pointer_down_y = 0.0f;
  jint pointer_touch_slop = 8;
  bool pointer_click_candidate = false;
#if defined(DARWIN_ART_REAL_GRAPHICS)
  std::unique_ptr<::android::uirenderer::renderthread::TimeLord> hwui_time_lord;
  std::unique_ptr<::android::uirenderer::AnimationContext> hwui_animation_context;
#endif
};

GraphicsFixtureState* EnsureGraphicsFixtureState(
    darwin_art_graphics::GraphicsState* state);
GraphicsFixtureState* GetGraphicsFixtureState(
    darwin_art_graphics::GraphicsState* state);

// Keeps the sidecar and its JNI globals alive across reentrant owner-Looper
// progress. Retirement rejects new invocations; cleanup waits for the last
// admitted invocation to unwind on its JNI owner thread.
class GraphicsFixtureInvocation final {
 public:
  GraphicsFixtureInvocation(darwin_art_graphics::GraphicsState*, JNIEnv*);
  ~GraphicsFixtureInvocation();
  GraphicsFixtureInvocation(const GraphicsFixtureInvocation&) = delete;
  GraphicsFixtureInvocation& operator=(const GraphicsFixtureInvocation&) = delete;
  GraphicsFixtureState* get() const { return fixture_; }
 private:
  darwin_art_graphics::GraphicsState* state_ = nullptr;
  GraphicsFixtureState* fixture_ = nullptr;
  JNIEnv* env_ = nullptr; // Scoped native invocation only.
};

// ProbeCanvas is a fixture-only class. It is kept in an opaque sidecar rather
// than in the production GraphicsState ABI.
void SetProbeCanvasClass(darwin_art_graphics::GraphicsState* state, JNIEnv* env,
                         jclass canvas_class);
jclass LookupProbeCanvasClass(darwin_art_graphics::GraphicsState* state);
bool retain_interactive_root(darwin_art_graphics::GraphicsState* state,
                             JNIEnv* env, jobject root, jint width,
                             jint height);
bool retain_interactive_view_root(darwin_art_graphics::GraphicsState* state,
                                  JNIEnv* env, jobject view_root);
bool retain_hardware_context(darwin_art_graphics::GraphicsState* state,
                             JNIEnv* env, jobject context);
bool install_surface_input_sink(darwin_art_graphics::GraphicsState* state);
bool take_surface_input(darwin_art_graphics::GraphicsState* state,
                        FixtureQueuedInput* packet);
bool acknowledge_surface_input_if_empty(
    darwin_art_graphics::GraphicsState* state);
void ClearProbeCanvasState(darwin_art_graphics::GraphicsState* state, JNIEnv* env);

// Releases every fixture sidecar owned by the current fixture run. The caller
// must invoke this on the ART owner thread while its JNIEnv is valid.
void ClearAllProbeCanvasState(JNIEnv* env);

}  // namespace darwin_art_graphics_fixture
