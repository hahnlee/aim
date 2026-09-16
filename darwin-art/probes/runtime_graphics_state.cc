#include "runtime_graphics_state.h"

#include "darwin_framework_natives.h"
#include "darwin_surface_bridge.h"

#if defined(DARWIN_ART_REAL_GRAPHICS)
#define private public
#define protected public
#include "AnimationContext.h"
#include "renderthread/TimeLord.h"
#undef protected
#undef private
#endif

namespace darwin_art_graphics {

GraphicsState::GraphicsState() = default;
GraphicsState::~GraphicsState() = default;

#if defined(DARWIN_ART_REAL_GRAPHICS)
android::uirenderer::renderthread::TimeLord* hwui_time_lord_for_state(
    GraphicsState* state) {
  return state == nullptr ? nullptr : state->hwui_time_lord.get();
}

android::uirenderer::AnimationContext* hwui_animation_context_for_state(
    GraphicsState* state) {
  return state == nullptr ? nullptr : state->hwui_animation_context.get();
}
#endif

void set_probe_canvas_class(GraphicsState* state, JNIEnv* env,
                            jclass canvas_class) {
  if (state == nullptr || env == nullptr) return;
  if (state->probe_canvas_class != nullptr) {
    env->DeleteGlobalRef(state->probe_canvas_class);
    state->probe_canvas_class = nullptr;
  }
  if (canvas_class != nullptr) {
    state->probe_canvas_class =
        static_cast<jclass>(env->NewGlobalRef(canvas_class));
  }
}

bool retain_interactive_root(GraphicsState* state, JNIEnv* env, jobject root,
                             jint width, jint height) {
  if (state == nullptr || env == nullptr || root == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  if (state->interactive_root != nullptr) {
    env->DeleteGlobalRef(state->interactive_root);
    state->interactive_root = nullptr;
  }
  state->interactive_root = env->NewGlobalRef(root);
  state->interactive_width = width;
  state->interactive_height = height;
  return state->interactive_root != nullptr && !env->ExceptionCheck();
}

bool retain_interactive_view_root(GraphicsState* state, JNIEnv* env,
                                  jobject view_root) {
  if (state == nullptr || env == nullptr || view_root == nullptr) return false;
  if (state->interactive_view_root != nullptr) {
    env->DeleteGlobalRef(state->interactive_view_root);
    state->interactive_view_root = nullptr;
  }
  state->interactive_view_root = env->NewGlobalRef(view_root);
  if (state->interactive_view_root == nullptr || env->ExceptionCheck()) {
    return false;
  }
  if (state->focused_view_root != nullptr) {
    darwin_art::SetFrameworkViewRootFocus(env, state->focused_view_root, false);
    env->DeleteGlobalRef(state->focused_view_root);
    state->focused_view_root = nullptr;
  }
  if (!darwin_art::FocusFrameworkViewRoot(env, view_root)) return false;
  state->focused_view_root = env->NewGlobalRef(view_root);
  return state->focused_view_root != nullptr && !env->ExceptionCheck();
}

bool retain_hardware_context(GraphicsState* state, JNIEnv* env, jobject context) {
  if (state == nullptr || env == nullptr || context == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  if (state->hardware_context != nullptr) {
    env->DeleteGlobalRef(state->hardware_context);
    state->hardware_context = nullptr;
  }
  state->hardware_context = env->NewGlobalRef(context);
  return state->hardware_context != nullptr && !env->ExceptionCheck();
}

void shutdown(GraphicsState* state, JNIEnv* env) {
  if (state == nullptr || env == nullptr) return;
  if (state->gpu_surface != nullptr && state->owner_wake_bound) {
    // A window-close callback can destroy the active surface before ART's
    // owner-thread teardown runs. Consult the lock-free active-surface slot
    // before touching the opaque handle; a stale GraphicsState borrow must
    // fail closed instead of dereferencing freed Objective-C state.
    if (darwin_art_surface_active_gpu() == state->gpu_surface) {
      (void)darwin_art_surface_set_owner_wake(state->gpu_surface, nullptr,
                                               nullptr);
    }
    state->owner_wake_bound = false;
  }
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
  state->main_looper = {};
#if defined(DARWIN_ART_REAL_GRAPHICS)
  if (state->hwui_animation_context != nullptr) {
    state->hwui_animation_context->destroy();
    state->hwui_animation_context.reset();
  }
  state->hwui_time_lord.reset();
#endif
  if (state->gpu_render_node != nullptr) {
    env->DeleteGlobalRef(state->gpu_render_node);
    state->gpu_render_node = nullptr;
  }
  if (state->pressed_view != nullptr) {
    env->DeleteGlobalRef(state->pressed_view);
    state->pressed_view = nullptr;
  }
  if (state->interactive_view_root != nullptr) {
    env->DeleteGlobalRef(state->interactive_view_root);
    state->interactive_view_root = nullptr;
  }
  if (state->pointer_dispatch_root != nullptr) {
    env->DeleteGlobalRef(state->pointer_dispatch_root);
    state->pointer_dispatch_root = nullptr;
  }
  if (state->pointer_dispatch_view_root != nullptr) {
    env->DeleteGlobalRef(state->pointer_dispatch_view_root);
    state->pointer_dispatch_view_root = nullptr;
  }
  if (state->focused_view_root != nullptr) {
    env->DeleteGlobalRef(state->focused_view_root);
    state->focused_view_root = nullptr;
  }
  state->pointer_dispatch_offset_x = 0.0f;
  state->pointer_dispatch_offset_y = 0.0f;
  state->pointer_dispatch_is_window = false;
  state->pointer_dispatch_outside_only = false;
  state->pointer_down_x = 0.0f;
  state->pointer_down_y = 0.0f;
  state->pointer_touch_slop = 8;
  state->pointer_click_candidate = false;
  if (state->interactive_root != nullptr) {
    env->DeleteGlobalRef(state->interactive_root);
    state->interactive_root = nullptr;
  }
  if (state->hardware_context != nullptr) {
    env->DeleteGlobalRef(state->hardware_context);
    state->hardware_context = nullptr;
  }
  if (state->probe_canvas_class != nullptr) {
    env->DeleteGlobalRef(state->probe_canvas_class);
    state->probe_canvas_class = nullptr;
  }
  state->gpu_render_node_recorded = false;
  state->gpu_ripple_overlay_active = false;
  state->gpu_ripple_overlay_x = 0.0f;
  state->gpu_ripple_overlay_y = 0.0f;
  state->interactive_width = 0;
  state->interactive_height = 0;
  state->pending_pressed_action = 0;
  state->pending_pressed_x = 0.0f;
  state->pending_pressed_y = 0.0f;
  // SurfaceSession owns this handle. GraphicsState only borrows it.
  state->gpu_surface = nullptr;
}

}  // namespace darwin_art_graphics
