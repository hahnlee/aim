#include "graphics_fixture_state.h"

#include <new>
#include <vector>

#if defined(DARWIN_ART_REAL_GRAPHICS)
#define private public
#define protected public
#include "AnimationContext.h"
#include "renderthread/TimeLord.h"
#undef protected
#undef private
#endif

namespace darwin_art_graphics_fixture {

bool set_view_root_focus(JNIEnv* env, jobject view_root, bool focused) {
  if (env == nullptr || view_root == nullptr || env->ExceptionCheck()) return false;
  jclass root_class = env->GetObjectClass(view_root);
  jfieldID field = root_class == nullptr || env->ExceptionCheck() ? nullptr
      : env->GetFieldID(root_class, "mInputEventReceiver",
          "Landroid/view/ViewRootImpl$WindowInputEventReceiver;");
  jobject receiver = field == nullptr || env->ExceptionCheck() ? nullptr
      : env->GetObjectField(view_root, field);
  jclass receiver_class = receiver == nullptr || env->ExceptionCheck() ? nullptr
      : env->FindClass("android/view/InputEventReceiver");
  jmethodID method = receiver_class == nullptr || env->ExceptionCheck() ? nullptr
      : env->GetMethodID(receiver_class, "onFocusEvent", "(Z)V");
  const bool invoked = method != nullptr && !env->ExceptionCheck();
  if (invoked) env->CallVoidMethod(receiver, method, focused ? JNI_TRUE : JNI_FALSE);
  if (receiver_class != nullptr) env->DeleteLocalRef(receiver_class);
  if (receiver != nullptr) env->DeleteLocalRef(receiver);
  if (root_class != nullptr) env->DeleteLocalRef(root_class);
  return invoked && !env->ExceptionCheck();
}

namespace {

void RetainSurfaceInputContext(void* context) {
  auto* sink = static_cast<SurfaceInputSinkContext*>(context);
  if (sink != nullptr) {
    sink->references.fetch_add(1, std::memory_order_relaxed);
  }
}

void ReleaseSurfaceInputContext(void* context) {
  auto* sink = static_cast<SurfaceInputSinkContext*>(context);
  if (sink != nullptr &&
      sink->references.fetch_sub(1, std::memory_order_acq_rel) == 1) {
    delete sink;
  }
}

DarwinArtSurfaceInputResult QueueSurfacePointer(
    void* context, const DarwinArtPointerEventV2* event) {
  auto* sink = static_cast<SurfaceInputSinkContext*>(context);
  if (sink == nullptr || event == nullptr) {
    return DARWIN_ART_SURFACE_INPUT_INVALID;
  }
  return sink->queue.enqueue_pointer(*event);
}

DarwinArtSurfaceInputResult QueueSurfaceKey(
    void* context, const DarwinArtKeyEventV1* event) {
  auto* sink = static_cast<SurfaceInputSinkContext*>(context);
  if (sink == nullptr || event == nullptr) {
    return DARWIN_ART_SURFACE_INPUT_INVALID;
  }
  return sink->queue.enqueue_key(*event);
}

std::mutex g_fixture_states_mutex;
std::unordered_map<darwin_art_graphics::GraphicsState*,
                   std::unique_ptr<GraphicsFixtureState>>
    g_fixture_states;

void Cleanup(GraphicsFixtureState* state, JNIEnv* env) {
  if (state == nullptr) return;
  if (state->gpu_surface != nullptr && state->input_sink_context != nullptr) {
    (void)darwin_art_surface_set_input_sink(state->gpu_surface, nullptr);
    ReleaseSurfaceInputContext(state->input_sink_context);
    state->input_sink_context = nullptr;
  }
  if (state->gpu_surface != nullptr && state->owner_wake_bound) {
    // The surface is owned by the fixture session, but its AppKit callback
    // can outlive the sidecar. Unbind only when the active slot still refers
    // to this exact borrowed handle; a stale sidecar must fail closed.
    if (darwin_art_surface_active_gpu() == state->gpu_surface) {
      (void)darwin_art_surface_set_owner_wake(state->gpu_surface, nullptr,
                                               nullptr);
    }
    state->owner_wake_bound = false;
  }
#if defined(DARWIN_ART_REAL_GRAPHICS)
  if (state->hwui_animation_context != nullptr) {
    state->hwui_animation_context->destroy();
    state->hwui_animation_context.reset();
  }
  state->hwui_time_lord.reset();
#endif
  if (env != nullptr) {
    DisposeFixtureInputEndpoints(state, env);
    auto clear_ref = [env](jobject* reference) {
      if (*reference != nullptr) {
        env->DeleteGlobalRef(*reference);
        *reference = nullptr;
      }
    };
    clear_ref(&state->gpu_render_node);
    clear_ref(&state->interactive_view_root);
    clear_ref(&state->focused_view_root);
    clear_ref(&state->interactive_root);
    clear_ref(&state->hardware_context);
    clear_ref(&state->pressed_view);
    clear_ref(&state->pointer_dispatch_root);
    clear_ref(&state->pointer_dispatch_view_root);
    auto clear_class = [env](jclass* reference) {
      if (*reference != nullptr) {
        env->DeleteGlobalRef(*reference);
        *reference = nullptr;
      }
    };
    clear_class(&state->main_looper.looper_class);
    clear_class(&state->main_looper.queue_class);
    clear_class(&state->main_looper.message_class);
    clear_class(&state->main_looper.handler_class);
    clear_class(&state->main_looper.clock_class);
    clear_class(&state->probe_canvas_class);
  }
}

}  // namespace

GraphicsFixtureState::GraphicsFixtureState() = default;
GraphicsFixtureState::~GraphicsFixtureState() = default;

GraphicsFixtureState* EnsureGraphicsFixtureState(
    darwin_art_graphics::GraphicsState* state) {
  if (state == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
  auto found = g_fixture_states.find(state);
  if (found != g_fixture_states.end()) return found->second.get();
  auto fixture = std::make_unique<GraphicsFixtureState>();
  auto* fixture_ptr = fixture.get();
  g_fixture_states.emplace(state, std::move(fixture));
  return fixture_ptr;
}

GraphicsFixtureState* GetGraphicsFixtureState(
    darwin_art_graphics::GraphicsState* state) {
  if (state == nullptr) return nullptr;
  std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
  auto found = g_fixture_states.find(state);
  return found == g_fixture_states.end() ? nullptr : found->second.get();
}

GraphicsFixtureInvocation::GraphicsFixtureInvocation(
    darwin_art_graphics::GraphicsState* state, JNIEnv* env) {
  if (state == nullptr || env == nullptr) return;
  std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
  auto found = g_fixture_states.find(state);
  if (found == g_fixture_states.end()) {
    auto fixture = std::make_unique<GraphicsFixtureState>();
    found = g_fixture_states.emplace(state, std::move(fixture)).first;
  }
  if (found->second->retiring.load(std::memory_order_acquire)) return;
  ++found->second->active_invocations;
  state_ = state;
  fixture_ = found->second.get();
  env_ = env;
}

GraphicsFixtureInvocation::~GraphicsFixtureInvocation() {
  if (fixture_ == nullptr) return;
  bool finish_retirement = false;
  {
    std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
    --fixture_->active_invocations;
    finish_retirement = fixture_->active_invocations == 0 &&
        fixture_->retiring.load(std::memory_order_acquire);
  }
  if (finish_retirement) ClearProbeCanvasState(state_, env_);
}


void SetProbeCanvasClass(darwin_art_graphics::GraphicsState* state, JNIEnv* env,
                         jclass canvas_class) {
  if (env == nullptr) return;
  auto* fixture = EnsureGraphicsFixtureState(state);
  if (fixture == nullptr) return;
  if (fixture->probe_canvas_class != nullptr) {
    env->DeleteGlobalRef(fixture->probe_canvas_class);
    fixture->probe_canvas_class = nullptr;
  }
  if (canvas_class != nullptr) {
    fixture->probe_canvas_class =
        static_cast<jclass>(env->NewGlobalRef(canvas_class));
  }
}

jclass LookupProbeCanvasClass(darwin_art_graphics::GraphicsState* state) {
  auto* fixture = GetGraphicsFixtureState(state);
  return fixture == nullptr ? nullptr : fixture->probe_canvas_class;
}

bool retain_interactive_root(darwin_art_graphics::GraphicsState* state,
                             JNIEnv* env, jobject root, jint width,
                             jint height) {
  if (state == nullptr || env == nullptr || root == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  auto* fixture = EnsureGraphicsFixtureState(state);
  if (fixture == nullptr) return false;
  if (fixture->interactive_root != nullptr) {
    env->DeleteGlobalRef(fixture->interactive_root);
    fixture->interactive_root = nullptr;
  }
  fixture->interactive_root = env->NewGlobalRef(root);
  fixture->interactive_width = width;
  fixture->interactive_height = height;
  return fixture->interactive_root != nullptr && !env->ExceptionCheck();
}

bool retain_interactive_view_root(darwin_art_graphics::GraphicsState* state,
                                  JNIEnv* env, jobject view_root) {
  if (state == nullptr || env == nullptr || view_root == nullptr) return false;
  auto* fixture = EnsureGraphicsFixtureState(state);
  if (fixture == nullptr) return false;
  if (fixture->interactive_view_root != nullptr) {
    env->DeleteGlobalRef(fixture->interactive_view_root);
    fixture->interactive_view_root = nullptr;
  }
  fixture->interactive_view_root = env->NewGlobalRef(view_root);
  if (fixture->interactive_view_root == nullptr || env->ExceptionCheck()) {
    return false;
  }
  if (fixture->focused_view_root != nullptr) {
    set_view_root_focus(env, fixture->focused_view_root, false);
    env->DeleteGlobalRef(fixture->focused_view_root);
    fixture->focused_view_root = nullptr;
  }
  if (!set_view_root_focus(env, view_root, true)) return false;
  fixture->focused_view_root = env->NewGlobalRef(view_root);
  return fixture->focused_view_root != nullptr && !env->ExceptionCheck();
}

bool retain_hardware_context(darwin_art_graphics::GraphicsState* state,
                             JNIEnv* env, jobject context) {
  if (state == nullptr || env == nullptr || context == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  auto* fixture = EnsureGraphicsFixtureState(state);
  if (fixture == nullptr) return false;
  if (fixture->hardware_context != nullptr) {
    env->DeleteGlobalRef(fixture->hardware_context);
    fixture->hardware_context = nullptr;
  }
  fixture->hardware_context = env->NewGlobalRef(context);
  return fixture->hardware_context != nullptr && !env->ExceptionCheck();
}

bool install_surface_input_sink(darwin_art_graphics::GraphicsState* state) {
  if (state == nullptr) return false;
  auto* fixture = EnsureGraphicsFixtureState(state);
  if (fixture == nullptr || fixture->gpu_surface == nullptr) return false;
  if (fixture->input_sink_context == nullptr) {
    fixture->input_sink_context = new (std::nothrow) SurfaceInputSinkContext();
    if (fixture->input_sink_context == nullptr) return false;
  }
  const DarwinArtSurfaceInputSink sink{
      .version = 1,
      .size = static_cast<uint32_t>(sizeof(DarwinArtSurfaceInputSink)),
      .context = fixture->input_sink_context,
      .retain_context = &RetainSurfaceInputContext,
      .release_context = &ReleaseSurfaceInputContext,
      .pointer = &QueueSurfacePointer,
      .key = &QueueSurfaceKey,
  };
  return darwin_art_surface_set_input_sink(fixture->gpu_surface, &sink) ==
         DARWIN_ART_SURFACE_OK;
}

bool take_surface_input(darwin_art_graphics::GraphicsState* state,
                        FixtureQueuedInput* packet) {
  if (state == nullptr || packet == nullptr) return false;
  auto* fixture = GetGraphicsFixtureState(state);
  if (fixture == nullptr || fixture->input_sink_context == nullptr) return false;
  return fixture->input_sink_context->queue.take(packet);
}

bool acknowledge_surface_input_if_empty(
    darwin_art_graphics::GraphicsState* state) {
  if (state == nullptr) return false;
  auto* fixture = GetGraphicsFixtureState(state);
  if (fixture == nullptr || fixture->input_sink_context == nullptr) return false;
  return fixture->input_sink_context->queue.acknowledge_if_empty();
}

void ClearProbeCanvasState(darwin_art_graphics::GraphicsState* state,
                           JNIEnv* env) {
  if (state == nullptr) return;
  GraphicsFixtureState* retiring = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
    auto found = g_fixture_states.find(state);
    if (found == g_fixture_states.end()) return;
    retiring = found->second.get();
    retiring->retiring.store(true, std::memory_order_release);
    retiring->input_endpoints_retired = true;
    if (retiring->active_invocations != 0 || retiring->cleanup_started) return;
    retiring->cleanup_started = true;
  }
  // Keep a tombstone visible during Java cleanup: reentry cannot recreate a
  // fresh sidecar or admit a new invocation while this one is retiring.
  Cleanup(retiring, env);
  std::unique_ptr<GraphicsFixtureState> fixture;
  {
    std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
    auto found = g_fixture_states.find(state);
    if (found != g_fixture_states.end() && found->second.get() == retiring) {
      fixture = std::move(found->second);
      g_fixture_states.erase(found);
    }
  }
}

void ClearAllProbeCanvasState(JNIEnv* env) {
  if (env == nullptr) return;
  std::vector<darwin_art_graphics::GraphicsState*> states;
  {
    std::lock_guard<std::mutex> lock(g_fixture_states_mutex);
    states.reserve(g_fixture_states.size());
    for (auto& entry : g_fixture_states) {
      entry.second->retiring.store(true, std::memory_order_release);
      entry.second->input_endpoints_retired = true;
      states.emplace_back(entry.first);
    }
  }
  for (auto* state : states) ClearProbeCanvasState(state, env);
}

}  // namespace darwin_art_graphics_fixture
