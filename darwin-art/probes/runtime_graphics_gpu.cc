#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <memory>

#include "darwin_angle_egl.h"
#include "darwin_hwui_gpu_mode.h"
#include "darwin_surface_bridge.h"
#include "mirror/throwable.h"
#include "runtime_graphics_gpu.h"
#include "../runtime/embedding/graphics_state.h"
#include "graphics_fixture_state.h"
#include "runtime_frame_probe.h"
#include "runtime_hwui_probe.h"
#include "thread-current-inl.h"

#if defined(DARWIN_ART_REAL_GRAPHICS)
#ifdef HIDDEN
#undef HIDDEN
#endif
#define private public
#define protected public
#include "AnimationContext.h"
#include "Animator.h"
#include "AnimatorManager.h"
#include "renderthread/TimeLord.h"
#include "RenderNode.h"
#undef protected
#undef private
#endif

namespace darwin_art_graphics {

#if defined(DARWIN_ART_REAL_GRAPHICS)
namespace {

jobject view_root_render_node(JNIEnv* env, jobject view, bool* has_view_root,
                              bool* has_display_list) {
  if (has_view_root != nullptr) *has_view_root = false;
  if (has_display_list != nullptr) *has_display_list = false;
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_view_root =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getViewRootImpl",
                             "()Landroid/view/ViewRootImpl;");
  jobject view_root = get_view_root == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->CallObjectMethod(view, get_view_root);
  if (has_view_root != nullptr) *has_view_root = view_root != nullptr;
  jclass root_class =
      view_root == nullptr ? nullptr : env->GetObjectClass(view_root);
  jfieldID attach_info_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mAttachInfo",
                            "Landroid/view/View$AttachInfo;");
  jobject attach_info = attach_info_field == nullptr
                            ? nullptr
                            : env->GetObjectField(view_root, attach_info_field);
  jclass attach_class =
      attach_info == nullptr ? nullptr : env->GetObjectClass(attach_info);
  jfieldID renderer_field =
      attach_class == nullptr
          ? nullptr
          : env->GetFieldID(attach_class, "mThreadedRenderer",
                            "Landroid/view/ThreadedRenderer;");
  jobject renderer = renderer_field == nullptr
                         ? nullptr
                         : env->GetObjectField(attach_info, renderer_field);
  jclass renderer_class =
      renderer == nullptr ? nullptr : env->GetObjectClass(renderer);
  jmethodID get_root_node =
      renderer_class == nullptr
          ? nullptr
          : env->GetMethodID(renderer_class, "getRootNode",
                             "()Landroid/graphics/RenderNode;");
  jobject root_node = get_root_node == nullptr
                          ? nullptr
                          : env->CallObjectMethod(renderer, get_root_node);
  jclass render_node_class =
      root_node == nullptr ? nullptr : env->GetObjectClass(root_node);
  jmethodID has_list =
      render_node_class == nullptr
          ? nullptr
          : env->GetMethodID(render_node_class, "hasDisplayList", "()Z");
  if (has_display_list != nullptr && has_list != nullptr &&
      !env->ExceptionCheck()) {
    *has_display_list =
        env->CallBooleanMethod(root_node, has_list) == JNI_TRUE;
  }
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    if (root_node != nullptr) {
      env->DeleteLocalRef(root_node);
      root_node = nullptr;
    }
  }
  if (render_node_class != nullptr) env->DeleteLocalRef(render_node_class);
  if (renderer_class != nullptr) env->DeleteLocalRef(renderer_class);
  if (renderer != nullptr) env->DeleteLocalRef(renderer);
  if (attach_class != nullptr) env->DeleteLocalRef(attach_class);
  if (attach_info != nullptr) env->DeleteLocalRef(attach_info);
  if (root_class != nullptr) env->DeleteLocalRef(root_class);
  if (view_root != nullptr) env->DeleteLocalRef(view_root);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  return root_node;
}

bool view_subtree_needs_recording(JNIEnv* env, jobject view, jclass group_class,
                                  jmethodID is_dirty,
                                  jmethodID is_layout_requested,
                                  jmethodID get_visibility,
                                  jmethodID get_child_count,
                                  jmethodID get_child_at, int depth) {
  if (view == nullptr || depth > 32 || env->ExceptionCheck()) return true;
  // GONE and INVISIBLE descendants do not participate in ViewRoot's draw
  // traversal. In particular an uninflated ViewStub intentionally keeps its
  // dirty/layout bits forever; treating that placeholder as frame damage
  // forced large APK hierarchies such as Chromium to re-record at 60 Hz.
  if (depth > 0 && env->CallIntMethod(view, get_visibility) != 0) return false;
  const bool dirty = env->CallBooleanMethod(view, is_dirty) == JNI_TRUE;
  const bool layout_requested =
      env->CallBooleanMethod(view, is_layout_requested) == JNI_TRUE;
  if (dirty || layout_requested) {
    return true;
  }
  if (!env->IsInstanceOf(view, group_class)) return false;
  const jint count = env->CallIntMethod(view, get_child_count);
  for (jint index = 0; index < count && !env->ExceptionCheck(); ++index) {
    jobject child = env->CallObjectMethod(view, get_child_at, index);
    const bool child_needs_recording = view_subtree_needs_recording(
        env, child, group_class, is_dirty, is_layout_requested,
        get_visibility, get_child_count, get_child_at, depth + 1);
    if (child != nullptr) env->DeleteLocalRef(child);
    if (child_needs_recording) return true;
  }
  return env->ExceptionCheck();
}

// Returns 0/1 when the view belongs to a real ViewRootImpl, and -1 for the
// detached probe fallback. ViewRootImpl is Android's authoritative damage
// scheduler: invalidate(), requestLayout(), and animation callbacks all
// converge on mTraversalScheduled. Leaf View dirty bits are not equivalent;
// placeholders and manually flattened recordings may retain them indefinitely.
int view_root_traversal_pending(JNIEnv* env, jobject view) {
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_view_root =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getViewRootImpl",
                             "()Landroid/view/ViewRootImpl;");
  jobject view_root = get_view_root == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->CallObjectMethod(view, get_view_root);
  jclass root_class =
      view_root == nullptr ? nullptr : env->GetObjectClass(view_root);
  jfieldID traversal_scheduled =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mTraversalScheduled", "Z");
  int pending = -1;
  if (traversal_scheduled != nullptr && !env->ExceptionCheck()) {
    pending = env->GetBooleanField(view_root, traversal_scheduled) == JNI_TRUE
                  ? 1
                  : 0;
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (root_class != nullptr) env->DeleteLocalRef(root_class);
  if (view_root != nullptr) env->DeleteLocalRef(view_root);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  return pending;
}

int view_root_traversal_generation(JNIEnv* env, jobject view) {
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_view_root =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getViewRootImpl",
                             "()Landroid/view/ViewRootImpl;");
  jobject view_root = get_view_root == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->CallObjectMethod(view, get_view_root);
  jclass root_class =
      view_root == nullptr ? nullptr : env->GetObjectClass(view_root);
  jfieldID traversal_barrier =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mTraversalBarrier", "I");
  int generation = -1;
  if (traversal_barrier != nullptr && !env->ExceptionCheck()) {
    generation = env->GetIntField(view_root, traversal_barrier);
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (root_class != nullptr) env->DeleteLocalRef(root_class);
  if (view_root != nullptr) env->DeleteLocalRef(view_root);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  return generation;
}

void debug_view_root_renderer(JNIEnv* env, jobject view, int traversal_pending,
                              int traversal_generation) {
  static int remaining = 300;
  if (remaining <= 0 ||
      std::getenv("DARWIN_ART_DEBUG_VIEW_ROOT_RENDERER") == nullptr) {
    return;
  }
  --remaining;
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_view_root =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getViewRootImpl",
                             "()Landroid/view/ViewRootImpl;");
  jobject view_root = get_view_root == nullptr
                          ? nullptr
                          : env->CallObjectMethod(view, get_view_root);
  jclass root_class =
      view_root == nullptr ? nullptr : env->GetObjectClass(view_root);
  jfieldID attach_info_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mAttachInfo",
                            "Landroid/view/View$AttachInfo;");
  jfieldID surface_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mSurface", "Landroid/view/Surface;");
  jfieldID choreographer_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mChoreographer",
                            "Landroid/view/Choreographer;");
  jfieldID full_redraw_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mFullRedrawNeeded", "Z");
  jfieldID stopped_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mStopped", "Z");
  jfieldID app_visible_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mAppVisible", "Z");
  jfieldID view_visibility_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mViewVisibility", "I");
  jfieldID root_width_field =
      root_class == nullptr ? nullptr
                            : env->GetFieldID(root_class, "mWidth", "I");
  jfieldID root_height_field =
      root_class == nullptr ? nullptr
                            : env->GetFieldID(root_class, "mHeight", "I");
  jfieldID traversal_skip_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mLastPerformTraversalsSkipDrawReason",
                            "Ljava/lang/String;");
  jfieldID draw_skip_field =
      root_class == nullptr
          ? nullptr
          : env->GetFieldID(root_class, "mLastPerformDrawSkippedReason",
                            "Ljava/lang/String;");
  jobject attach_info =
      attach_info_field == nullptr
          ? nullptr
          : env->GetObjectField(view_root, attach_info_field);
  jobject surface =
      surface_field == nullptr ? nullptr : env->GetObjectField(view_root, surface_field);
  jobject choreographer =
      choreographer_field == nullptr
          ? nullptr
          : env->GetObjectField(view_root, choreographer_field);
  jstring traversal_skip =
      traversal_skip_field == nullptr
          ? nullptr
          : static_cast<jstring>(
                env->GetObjectField(view_root, traversal_skip_field));
  jstring draw_skip =
      draw_skip_field == nullptr
          ? nullptr
          : static_cast<jstring>(env->GetObjectField(view_root, draw_skip_field));
  jclass choreographer_class =
      choreographer == nullptr ? nullptr : env->GetObjectClass(choreographer);
  jfieldID frame_scheduled_field =
      choreographer_class == nullptr
          ? nullptr
          : env->GetFieldID(choreographer_class, "mFrameScheduled", "Z");
  jfieldID display_receiver_field =
      choreographer_class == nullptr
          ? nullptr
          : env->GetFieldID(
                choreographer_class, "mDisplayEventReceiver",
                "Landroid/view/Choreographer$FrameDisplayEventReceiver;");
  jobject display_receiver =
      display_receiver_field == nullptr
          ? nullptr
          : env->GetObjectField(choreographer, display_receiver_field);
  jclass display_receiver_class =
      display_receiver == nullptr ? nullptr : env->GetObjectClass(display_receiver);
  jfieldID receiver_pointer_field =
      display_receiver_class == nullptr
          ? nullptr
          : env->GetFieldID(display_receiver_class, "mReceiverPtr", "J");
  jclass attach_class =
      attach_info == nullptr ? nullptr : env->GetObjectClass(attach_info);
  jfieldID renderer_field =
      attach_class == nullptr
          ? nullptr
          : env->GetFieldID(attach_class, "mThreadedRenderer",
                            "Landroid/view/ThreadedRenderer;");
  jobject renderer = renderer_field == nullptr
                         ? nullptr
                         : env->GetObjectField(attach_info, renderer_field);
  jclass renderer_class =
      renderer == nullptr ? nullptr : env->GetObjectClass(renderer);
  jmethodID is_enabled =
      renderer_class == nullptr
          ? nullptr
          : env->GetMethodID(renderer_class, "isEnabled", "()Z");
  jmethodID is_requested =
      renderer_class == nullptr
          ? nullptr
          : env->GetMethodID(renderer_class, "isRequested", "()Z");
  jmethodID get_root_node =
      renderer_class == nullptr
          ? nullptr
          : env->GetMethodID(renderer_class, "getRootNode",
                             "()Landroid/graphics/RenderNode;");
  jclass surface_class =
      surface == nullptr ? nullptr : env->GetObjectClass(surface);
  jmethodID surface_valid =
      surface_class == nullptr
          ? nullptr
          : env->GetMethodID(surface_class, "isValid", "()Z");
  jobject root_node =
      get_root_node == nullptr
          ? nullptr
          : env->CallObjectMethod(renderer, get_root_node);
  jclass render_node_class =
      root_node == nullptr ? nullptr : env->GetObjectClass(root_node);
  jmethodID has_display_list =
      render_node_class == nullptr
          ? nullptr
          : env->GetMethodID(render_node_class, "hasDisplayList", "()Z");
  if (!env->ExceptionCheck()) {
    const char* traversal_skip_utf =
        traversal_skip == nullptr
            ? nullptr
            : env->GetStringUTFChars(traversal_skip, nullptr);
    const char* draw_skip_utf =
        draw_skip == nullptr ? nullptr : env->GetStringUTFChars(draw_skip, nullptr);
    std::cerr << "ART ViewRoot renderer: root=" << view_root
              << " traversal_pending=" << traversal_pending
              << " traversal_generation=" << traversal_generation
              << " frame_scheduled="
              << (frame_scheduled_field != nullptr &&
                          env->GetBooleanField(choreographer,
                                               frame_scheduled_field) == JNI_TRUE)
              << " receiver_ptr="
              << (receiver_pointer_field == nullptr
                      ? 0
                      : env->GetLongField(display_receiver,
                                          receiver_pointer_field))
              << " surface_valid="
              << (surface_valid != nullptr &&
                          env->CallBooleanMethod(surface, surface_valid) == JNI_TRUE)
              << " renderer=" << renderer
              << " enabled="
              << (is_enabled != nullptr &&
                          env->CallBooleanMethod(renderer, is_enabled) == JNI_TRUE)
              << " requested="
              << (is_requested != nullptr &&
                          env->CallBooleanMethod(renderer, is_requested) == JNI_TRUE)
              << " render_node=" << root_node
              << " display_list="
              << (has_display_list != nullptr &&
                          env->CallBooleanMethod(root_node, has_display_list) == JNI_TRUE)
              << " size="
              << (root_width_field == nullptr
                      ? -1
                      : env->GetIntField(view_root, root_width_field))
              << "x"
              << (root_height_field == nullptr
                      ? -1
                      : env->GetIntField(view_root, root_height_field))
              << " visible="
              << (app_visible_field != nullptr &&
                          env->GetBooleanField(view_root, app_visible_field) == JNI_TRUE)
              << "/"
              << (view_visibility_field == nullptr
                      ? -1
                      : env->GetIntField(view_root, view_visibility_field))
              << " stopped="
              << (stopped_field != nullptr &&
                          env->GetBooleanField(view_root, stopped_field) == JNI_TRUE)
              << " full_redraw="
              << (full_redraw_field != nullptr &&
                          env->GetBooleanField(view_root, full_redraw_field) == JNI_TRUE)
              << " skip_traversal="
              << (traversal_skip_utf == nullptr ? "<none>" : traversal_skip_utf)
              << " skip_draw="
              << (draw_skip_utf == nullptr ? "<none>" : draw_skip_utf)
              << "\n";
    if (draw_skip_utf != nullptr) {
      env->ReleaseStringUTFChars(draw_skip, draw_skip_utf);
    }
    if (traversal_skip_utf != nullptr) {
      env->ReleaseStringUTFChars(traversal_skip, traversal_skip_utf);
    }
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (render_node_class != nullptr) env->DeleteLocalRef(render_node_class);
  if (root_node != nullptr) env->DeleteLocalRef(root_node);
  if (surface_class != nullptr) env->DeleteLocalRef(surface_class);
  if (renderer_class != nullptr) env->DeleteLocalRef(renderer_class);
  if (renderer != nullptr) env->DeleteLocalRef(renderer);
  if (display_receiver_class != nullptr) {
    env->DeleteLocalRef(display_receiver_class);
  }
  if (display_receiver != nullptr) env->DeleteLocalRef(display_receiver);
  if (choreographer_class != nullptr) env->DeleteLocalRef(choreographer_class);
  if (choreographer != nullptr) env->DeleteLocalRef(choreographer);
  if (draw_skip != nullptr) env->DeleteLocalRef(draw_skip);
  if (traversal_skip != nullptr) env->DeleteLocalRef(traversal_skip);
  if (attach_class != nullptr) env->DeleteLocalRef(attach_class);
  if (surface != nullptr) env->DeleteLocalRef(surface);
  if (attach_info != nullptr) env->DeleteLocalRef(attach_info);
  if (root_class != nullptr) env->DeleteLocalRef(root_class);
  if (view_root != nullptr) env->DeleteLocalRef(view_root);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
}

bool draw_window_manager_layers(JNIEnv* env, jobject canvas,
                                jclass view_class, jmethodID draw,
                                jclass animation_host_class,
                                jmethodID invalidate_view_tree) {
  jclass global_class = env->FindClass("android/view/WindowManagerGlobal");
  jmethodID get_instance =
      global_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(global_class, "getInstance",
                                   "()Landroid/view/WindowManagerGlobal;");
  jobject global = get_instance == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(global_class, get_instance);
  jfieldID views_field =
      global_class == nullptr
          ? nullptr
          : env->GetFieldID(global_class, "mViews", "Ljava/util/ArrayList;");
  jfieldID params_field =
      global_class == nullptr
          ? nullptr
          : env->GetFieldID(global_class, "mParams", "Ljava/util/ArrayList;");
  jobject views = global == nullptr || views_field == nullptr
                      ? nullptr
                      : env->GetObjectField(global, views_field);
  jobject params = global == nullptr || params_field == nullptr
                       ? nullptr
                       : env->GetObjectField(global, params_field);
  jclass list_class = env->FindClass("java/util/ArrayList");
  jmethodID list_size = list_class == nullptr
                            ? nullptr
                            : env->GetMethodID(list_class, "size", "()I");
  jmethodID list_get =
      list_class == nullptr
          ? nullptr
          : env->GetMethodID(list_class, "get", "(I)Ljava/lang/Object;");
  jclass params_class = env->FindClass("android/view/WindowManager$LayoutParams");
  jfieldID params_x = params_class == nullptr
                          ? nullptr
                          : env->GetFieldID(params_class, "x", "I");
  jfieldID params_y = params_class == nullptr
                          ? nullptr
                          : env->GetFieldID(params_class, "y", "I");
  jmethodID get_width = view_class == nullptr
                            ? nullptr
                            : env->GetMethodID(view_class, "getWidth", "()I");
  jmethodID get_height = view_class == nullptr
                             ? nullptr
                             : env->GetMethodID(view_class, "getHeight", "()I");
  jclass canvas_class = env->FindClass("android/graphics/Canvas");
  jmethodID save = canvas_class == nullptr
                       ? nullptr
                       : env->GetMethodID(canvas_class, "save", "()I");
  jmethodID translate =
      canvas_class == nullptr
          ? nullptr
          : env->GetMethodID(canvas_class, "translate", "(FF)V");
  jmethodID clip_rect =
      canvas_class == nullptr
          ? nullptr
          : env->GetMethodID(canvas_class, "clipRect", "(FFFF)Z");
  jmethodID restore =
      canvas_class == nullptr
          ? nullptr
          : env->GetMethodID(canvas_class, "restoreToCount", "(I)V");
  bool ok = global != nullptr && views != nullptr && params != nullptr &&
            list_size != nullptr && list_get != nullptr && params_x != nullptr &&
            params_y != nullptr && get_width != nullptr && get_height != nullptr &&
            save != nullptr && translate != nullptr && clip_rect != nullptr &&
            restore != nullptr && draw != nullptr && !env->ExceptionCheck();
  const jint count = ok ? env->CallIntMethod(views, list_size) : 0;
  for (jint index = 0; ok && index < count; ++index) {
    jobject layer = env->CallObjectMethod(views, list_get, index);
    jobject layout_params = env->CallObjectMethod(params, list_get, index);
    const jint layer_width = layer == nullptr ? 0 : env->CallIntMethod(layer, get_width);
    const jint layer_height = layer == nullptr ? 0 : env->CallIntMethod(layer, get_height);
    if (layer != nullptr && layout_params != nullptr && layer_width > 0 &&
        layer_height > 0 && !env->ExceptionCheck()) {
      if (invalidate_view_tree != nullptr && animation_host_class != nullptr) {
        env->CallStaticVoidMethod(animation_host_class, invalidate_view_tree, layer);
      }
      const jint restore_count = env->CallIntMethod(canvas, save);
      env->CallVoidMethod(canvas, translate,
                          static_cast<jfloat>(env->GetIntField(layout_params, params_x)),
                          static_cast<jfloat>(env->GetIntField(layout_params, params_y)));
      env->CallBooleanMethod(canvas, clip_rect, 0.0f, 0.0f,
                             static_cast<jfloat>(layer_width),
                             static_cast<jfloat>(layer_height));
      env->CallVoidMethod(layer, draw, canvas);
      env->CallVoidMethod(canvas, restore, restore_count);
    }
    if (layout_params != nullptr) env->DeleteLocalRef(layout_params);
    if (layer != nullptr) env->DeleteLocalRef(layer);
    ok = !env->ExceptionCheck();
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (canvas_class != nullptr) env->DeleteLocalRef(canvas_class);
  if (params_class != nullptr) env->DeleteLocalRef(params_class);
  if (list_class != nullptr) env->DeleteLocalRef(list_class);
  if (params != nullptr) env->DeleteLocalRef(params);
  if (views != nullptr) env->DeleteLocalRef(views);
  if (global != nullptr) env->DeleteLocalRef(global);
  if (global_class != nullptr) env->DeleteLocalRef(global_class);
  return ok;
}

}  // namespace

void debug_product_view_root(JNIEnv* env, jobject view) {
  debug_view_root_renderer(env, view, -2, -2);
}

int prepare_gpu_surface(GraphicsState* state, jint width, jint height) {
  if (state == nullptr) return 1;
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (fixture->gpu_surface != nullptr) return 0;
  const bool run_apk_app = std::getenv("DARWIN_ART_APK_APP_PACKAGE") != nullptr;
  const char* app_label = std::getenv("DARWIN_ART_APK_WINDOW_TITLE");
  if (app_label == nullptr) app_label = std::getenv("DARWIN_ART_APK_APP_LABEL");
  DarwinArtSurfaceCreateInfo info{
      .width = static_cast<uint32_t>(width),
      .height = static_cast<uint32_t>(height),
      .title = run_apk_app && app_label != nullptr && app_label[0] != '\0'
                   ? app_label
                   : "Darwin ART · HWUI Metal",
      .visible = true,
      .scale_to_display = run_apk_app,
  };
  DarwinArtSurfaceResult result = DARWIN_ART_SURFACE_OK;
  fixture->gpu_surface = darwin_art_surface_create(&info, &result);
  if (fixture->gpu_surface == nullptr) {
    std::cerr << "ART HWUI GPU: surface initialization failed status="
              << result << "\n";
    return static_cast<int>(result);
  }
  // This fixture explicitly owns raw host-event queuing/manual dispatch.
  // Production surfaces install the Android ingress sink through the Rust
  // owner and never consume this retained fixture queue.
  if (!darwin_art_graphics_fixture::install_surface_input_sink(state)) {
    std::cerr << "ART HWUI GPU: fixture input sink installation failed\n";
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }
  darwin_art_surface_set_active_gpu(fixture->gpu_surface);
  // This is the display/HWC creation boundary. Configure the stable output
  // extent once here; individual BLAST queues retain their own dimensions.
  darwin_art::ConfigureDarwinAngleDisplayTarget(width, height);
  const uint32_t surface_id =
      darwin_art_surface_gpu_iosurface_id(fixture->gpu_surface);
  if (surface_id != 0) {
    const std::string encoded = std::to_string(surface_id);
    setenv("DARWIN_ART_HOST_IOSURFACE_ID", encoded.c_str(), 1);
    if (std::getenv("DARWIN_ART_DEBUG_ANGLE") != nullptr) {
      std::cerr << "ART HWUI GPU: exported host IOSurface id=" << surface_id
                << "\n";
    }
  }
  return 0;
}

int refresh_gpu_surface_identity(GraphicsState* state) {
  if (state == nullptr) return 1;
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (fixture->gpu_surface == nullptr) return 1;
  const char* app_label = std::getenv("DARWIN_ART_APK_WINDOW_TITLE");
  if (app_label == nullptr) app_label = std::getenv("DARWIN_ART_APK_APP_LABEL");
  if (app_label == nullptr || app_label[0] == '\0') return 0;
  return static_cast<int>(
      darwin_art_surface_set_title(fixture->gpu_surface, app_label));
}

jboolean attach_hardware_hierarchy_on_owner(GraphicsState* state, JNIEnv* env,
                                            jobject view) {
  auto* fixture = state == nullptr
                      ? nullptr
                      : darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (state == nullptr || env == nullptr || view == nullptr ||
      fixture == nullptr || fixture->hardware_context == nullptr) {
    return JNI_FALSE;
  }
  std::cerr << "ART HWUI GPU: owner-thread attach begin\n";
  jclass thread_class = env->FindClass("java/lang/Thread");
  jmethodID current_thread = thread_class == nullptr
                                 ? nullptr
                                 : env->GetStaticMethodID(
                                       thread_class, "currentThread",
                                       "()Ljava/lang/Thread;");
  jobject thread = current_thread == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(thread_class, current_thread);
  jmethodID get_loader = thread_class == nullptr
                             ? nullptr
                             : env->GetMethodID(
                                   thread_class, "getContextClassLoader",
                                   "()Ljava/lang/ClassLoader;");
  jobject loader = get_loader == nullptr
                       ? nullptr
                       : env->CallObjectMethod(thread, get_loader);
  jclass loader_class = loader == nullptr ? nullptr : env->GetObjectClass(loader);
  jmethodID load_class = loader_class == nullptr
                             ? nullptr
                             : env->GetMethodID(
                                   loader_class, "loadClass",
                                   "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring helper_name = env->NewStringUTF("dev.darwinart.probe.ProbeAnimationHost");
  jclass helper = load_class == nullptr
                      ? nullptr
                      : static_cast<jclass>(env->CallObjectMethod(
                            loader, load_class, helper_name));
  jmethodID attach = helper == nullptr
                         ? nullptr
                         : env->GetStaticMethodID(
                               helper, "attachHardwareHierarchy",
                               "(Ljava/lang/Object;Ljava/lang/Object;)Z");
  jboolean result = (attach == nullptr || env->ExceptionCheck())
                        ? JNI_FALSE
                        : env->CallStaticBooleanMethod(
                              helper, attach, view, fixture->hardware_context);
  if (result == JNI_TRUE && !env->ExceptionCheck()) {
    jclass view_class = env->FindClass("android/view/View");
    jfieldID attach_info_field =
        view_class == nullptr
            ? nullptr
            : env->GetFieldID(view_class, "mAttachInfo",
                              "Landroid/view/View$AttachInfo;");
    jobject attach_info =
        attach_info_field == nullptr
            ? nullptr
            : env->GetObjectField(view, attach_info_field);
    jclass attach_info_class =
        attach_info == nullptr ? nullptr : env->GetObjectClass(attach_info);
    jfieldID window_token_field =
        attach_info_class == nullptr
            ? nullptr
            : env->GetFieldID(attach_info_class, "mWindowToken",
                              "Landroid/os/IBinder;");
    jclass binder_class = env->FindClass("android/os/Binder");
    jmethodID binder_constructor =
        binder_class == nullptr
            ? nullptr
            : env->GetMethodID(binder_class, "<init>", "()V");
    jobject window_token =
        binder_constructor == nullptr
            ? nullptr
            : env->NewObject(binder_class, binder_constructor);
    if (attach_info != nullptr && window_token_field != nullptr &&
        window_token != nullptr && !env->ExceptionCheck()) {
      env->SetObjectField(attach_info, window_token_field, window_token);
    } else {
      result = JNI_FALSE;
    }
    env->DeleteLocalRef(window_token);
    env->DeleteLocalRef(binder_class);
    env->DeleteLocalRef(attach_info_class);
    env->DeleteLocalRef(attach_info);
    env->DeleteLocalRef(view_class);
  }
  if (env->ExceptionCheck()) {
    std::cerr << "ART HWUI GPU: owner-thread ViewRoot attach failed\n";
    env->ExceptionDescribe();
    env->ExceptionClear();
    result = JNI_FALSE;
  }
  std::cerr << "ART HWUI GPU: owner-thread attach end result="
            << (result == JNI_TRUE ? 1 : 0) << "\n";
  env->DeleteLocalRef(helper_name);
  env->DeleteLocalRef(loader_class);
  env->DeleteLocalRef(loader);
  env->DeleteLocalRef(thread);
  env->DeleteLocalRef(thread_class);
  return result;
}

jboolean present_gpu_content(GraphicsState* state, JNIEnv* env, jobject view,
                                  jint width, jint height) {
  if (state == nullptr) return JNI_FALSE;
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (!darwin_art::hwui_gpu_enabled()) {
    return JNI_FALSE;
  }
  if (fixture->gpu_surface == nullptr) {
    if (prepare_gpu_surface(state, width, height) != 0) {
      return JNI_FALSE;
    }
  }
  // Keep retained replay for unchanged frames, but honor Android's normal
  // invalidation contract. App-side Handler/Choreographer work can dirty a
  // TextView or request layout without another pointer action (for example a
  // running stopwatch). Those frames must rebuild the display list; otherwise
  // state advances in Java while Metal keeps presenting stale pixels.
  const int pending_root_traversal = view_root_traversal_pending(env, view);
  const int traversal_generation = view_root_traversal_generation(env, view);
  debug_view_root_renderer(env, view, pending_root_traversal,
                           traversal_generation);
  bool has_real_view_root = false;
  bool root_has_display_list = false;
  jobject retained_root = view_root_render_node(
      env, view, &has_real_view_root, &root_has_display_list);
  if (has_real_view_root) {
    if (std::getenv("DARWIN_ART_APK_APP_PACKAGE") != nullptr) {
      // Product frames belong exclusively to ViewRootImpl/ThreadedRenderer.
      // The compatibility host may observe that a root exists, but must not
      // replay it or advance its animators independently of HWUI RenderThread.
      if (retained_root != nullptr) env->DeleteLocalRef(retained_root);
      darwin_art_frame_probe::record_dimensions(width, height);
      return JNI_TRUE;
    }
    // ViewRootImpl/ThreadedRenderer owns recording, invalidation propagation,
    // and child RenderNode retention. The host only consumes that root at the
    // compositor boundary; it must never issue a second View.draw traversal.
    if (retained_root == nullptr || !root_has_display_list) {
      if (retained_root != nullptr) env->DeleteLocalRef(retained_root);
      return JNI_TRUE;
    }
    if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node == nullptr ||
        !env->IsSameObject(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node, retained_root)) {
      if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node != nullptr) {
        env->DeleteGlobalRef(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node);
      }
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node = env->NewGlobalRef(retained_root);
    }
    jclass retained_class = env->FindClass("android/graphics/RenderNode");
    jfieldID native_field =
        retained_class == nullptr
            ? nullptr
            : env->GetFieldID(retained_class, "mNativeRenderNode", "J");
    auto* node = native_field == nullptr
                     ? nullptr
                     : reinterpret_cast<android::uirenderer::RenderNode*>(
                           static_cast<std::uintptr_t>(env->GetLongField(
                               retained_root, native_field)));
    if (node != nullptr && !env->ExceptionCheck()) {
      node->mValid = true;
      darwin_art_hwui::sync_recorded_render_node_tree(node);
      if (darwin_art_hwui::node_subtree_has_animators(node)) {
        if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context == nullptr) {
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord = std::make_unique<
              android::uirenderer::renderthread::TimeLord>();
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord->setFrameInterval(16666666);
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context =
              std::make_unique<android::uirenderer::AnimationContext>(
                  *darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord);
        }
        darwin_art_hwui::register_node_subtree_animators(
            node, *darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context);
      }
    }
    const bool presented = node != nullptr && !env->ExceptionCheck() &&
                           darwin_art_hwui::render_node_to_surface(
                               env, view, retained_root, fixture->gpu_surface,
                               width, height, darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_active,
                               darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_x,
                               darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_y,
                               darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_started);
    if (env->ExceptionCheck()) env->ExceptionClear();
    if (retained_class != nullptr) env->DeleteLocalRef(retained_class);
    env->DeleteLocalRef(retained_root);
    if (!presented) return JNI_FALSE;
    darwin_art_frame_probe::record_dimensions(width, height);
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded = true;
    if (traversal_generation >= 0) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_last_traversal_barrier = traversal_generation;
    }
    return JNI_TRUE;
  }
  if (retained_root != nullptr) env->DeleteLocalRef(retained_root);
  if (std::getenv("DARWIN_ART_APK_APP_PACKAGE") != nullptr) {
    // Product APK windows are always owned by WindowManagerGlobal. During the
    // short interval before addView() publishes ViewRootImpl there is nothing
    // valid to present; recording the DecorView here creates a second, host-
    // owned rendering lifecycle. Wait for ThreadedRenderer instead.
    darwin_art_frame_probe::record_dimensions(width, height);
    return JNI_TRUE;
  }
  const bool completed_traversal =
      traversal_generation >= 0 &&
      traversal_generation != darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_last_traversal_barrier;
  bool view_needs_recording =
      pending_root_traversal != 0 || completed_traversal;
  jclass dirty_view_class = env->FindClass("android/view/View");
  jclass dirty_group_class = env->FindClass("android/view/ViewGroup");
  jmethodID is_dirty = dirty_view_class == nullptr
                           ? nullptr
                           : env->GetMethodID(dirty_view_class, "isDirty", "()Z");
  jmethodID is_layout_requested =
      dirty_view_class == nullptr
          ? nullptr
          : env->GetMethodID(dirty_view_class, "isLayoutRequested", "()Z");
  jmethodID get_visibility =
      dirty_view_class == nullptr
          ? nullptr
          : env->GetMethodID(dirty_view_class, "getVisibility", "()I");
  jmethodID get_child_count =
      dirty_group_class == nullptr
          ? nullptr
          : env->GetMethodID(dirty_group_class, "getChildCount", "()I");
  jmethodID get_child_at =
      dirty_group_class == nullptr
          ? nullptr
          : env->GetMethodID(dirty_group_class, "getChildAt",
                             "(I)Landroid/view/View;");
  if (pending_root_traversal < 0 && is_dirty != nullptr &&
      is_layout_requested != nullptr &&
      get_visibility != nullptr &&
      get_child_count != nullptr && get_child_at != nullptr &&
      !env->ExceptionCheck()) {
    view_needs_recording = view_subtree_needs_recording(
        env, view, dirty_group_class, is_dirty,
        is_layout_requested, get_visibility, get_child_count, get_child_at, 0);
  }
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    view_needs_recording = true;
  }
  if (dirty_group_class != nullptr) env->DeleteLocalRef(dirty_group_class);
  if (dirty_view_class != nullptr) env->DeleteLocalRef(dirty_view_class);
  // ACTION_MOVE remains replay-only while the hierarchy is clean. Re-recording
  // every 16 ms would replace display-list-owned CanvasProperty references and
  // make native RenderNode animations appear static.
  if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded && darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_action == 0 &&
      !view_needs_recording) {
    return darwin_art_hwui::render_node_to_surface(
               env, view, fixture->gpu_render_node, fixture->gpu_surface, width, height,
               darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_active, darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_x,
               darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_y, darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_started)
               ? JNI_TRUE
               : JNI_FALSE;
  }

  jclass render_node_class = env->FindClass("android/graphics/RenderNode");
  jfieldID native_render_node =
      render_node_class == nullptr
          ? nullptr
          : env->GetFieldID(render_node_class, "mNativeRenderNode", "J");
  // The helper lives in the app DEX, so resolve it through the content
  // classloader rather than FindClass (which is rooted at boot on this
  // standalone ART thread).
  jclass animation_host_class = nullptr;
  jclass thread_class = env->FindClass("java/lang/Thread");
  jmethodID current_thread =
      thread_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(thread_class, "currentThread",
                                   "()Ljava/lang/Thread;");
  jobject thread = current_thread == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(thread_class, current_thread);
  jmethodID get_class_loader =
      thread_class == nullptr
          ? nullptr
          : env->GetMethodID(thread_class, "getContextClassLoader",
                             "()Ljava/lang/ClassLoader;");
  jobject class_loader = get_class_loader == nullptr
                             ? nullptr
                             : env->CallObjectMethod(thread, get_class_loader);
  jclass class_loader_class = class_loader == nullptr
                                  ? nullptr
                                  : env->GetObjectClass(class_loader);
  jmethodID load_class =
      class_loader_class == nullptr
          ? nullptr
          : env->GetMethodID(class_loader_class, "loadClass",
                             "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring helper_name = env->NewStringUTF("dev.darwinart.probe.ProbeAnimationHost");
  jobject helper_class = load_class == nullptr
                             ? nullptr
                             : env->CallObjectMethod(class_loader, load_class,
                                                     helper_name);
  if (!env->ExceptionCheck()) {
    animation_host_class = static_cast<jclass>(helper_class);
  } else {
    env->ExceptionClear();
  }
  env->DeleteLocalRef(helper_name);
  env->DeleteLocalRef(class_loader_class);
  env->DeleteLocalRef(class_loader);
  env->DeleteLocalRef(thread);
  env->DeleteLocalRef(thread_class);
  if (animation_host_class == nullptr) {
    std::cerr << "ART HWUI GPU: app AnimationHost helper class unavailable\n";
  }
  jclass animation_host_interface =
      env->FindClass("android/graphics/RenderNode$AnimationHost");
  jmethodID animation_host_create =
      animation_host_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(animation_host_class, "create",
                                   "(Ljava/lang/Class;)Ljava/lang/Object;");
  jmethodID prepare_view_pagers =
      animation_host_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(animation_host_class, "prepareViewPagers",
                                   "(Ljava/lang/Object;)V");
  jmethodID dispatch_pre_draw =
      animation_host_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(animation_host_class, "dispatchPreDraw",
                                   "(Ljava/lang/Object;)Z");
  jmethodID begin_host_traversal =
      animation_host_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(animation_host_class, "beginHostTraversal",
                                   "(Ljava/lang/Object;)V");
  jmethodID dispatch_on_draw =
      animation_host_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(animation_host_class, "dispatchOnDraw",
                                   "(Ljava/lang/Object;)V");
  jmethodID invalidate_view_tree =
      animation_host_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(animation_host_class, "invalidateViewTree",
                                   "(Ljava/lang/Object;)V");
  jmethodID render_node_create =
      render_node_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                render_node_class, "create",
                "(Ljava/lang/String;Landroid/graphics/RenderNode$AnimationHost;)"
                "Landroid/graphics/RenderNode;");
  jmethodID begin_recording =
      render_node_class == nullptr
          ? nullptr
          : env->GetMethodID(render_node_class, "beginRecording",
                             "(II)Landroid/graphics/RecordingCanvas;");
  jmethodID end_recording =
      render_node_class == nullptr
          ? nullptr
          : env->GetMethodID(render_node_class, "endRecording", "()V");
  jmethodID set_position =
      render_node_class == nullptr
          ? nullptr
          : env->GetMethodID(render_node_class, "setPosition", "(IIII)Z");
  if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node == nullptr && render_node_create != nullptr &&
      animation_host_create != nullptr &&
      animation_host_interface != nullptr &&
      !env->ExceptionCheck()) {
    jstring node_name = env->NewStringUTF("Darwin ART HWUI root");
    jobject host = env->CallStaticObjectMethod(
        animation_host_class, animation_host_create, animation_host_interface);
    std::cerr << "ART HWUI GPU: animation host=" << host << "\n";
    jobject node = env->CallStaticObjectMethod(render_node_class, render_node_create,
                                               node_name, host);
    std::cerr << "ART HWUI GPU: RenderNode.create node=" << node << "\n";
    if (node != nullptr && !env->ExceptionCheck()) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node = env->NewGlobalRef(node);
    }
    env->DeleteLocalRef(host);
    env->DeleteLocalRef(node);
    env->DeleteLocalRef(node_name);
  }
  // APK windows are attached by WindowManagerGlobal/ViewRootImpl and must
  // retain that authoritative AttachInfo. Only legacy standalone widget
  // probes use the bounded synthetic attachment helper.
  jclass attached_view_class = env->FindClass("android/view/View");
  jmethodID is_attached_to_window =
      attached_view_class == nullptr
          ? nullptr
          : env->GetMethodID(attached_view_class, "isAttachedToWindow", "()Z");
  const bool has_attached_view_root =
      is_attached_to_window != nullptr && !env->ExceptionCheck() &&
      env->CallBooleanMethod(view, is_attached_to_window) == JNI_TRUE;
  if (env->ExceptionCheck()) env->ExceptionClear();
  env->DeleteLocalRef(attached_view_class);
  if (fixture->gpu_render_node != nullptr && fixture->hardware_context != nullptr &&
      !darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded && !has_attached_view_root &&
      fixture->interactive_view_root == nullptr) {
    attach_hardware_hierarchy_on_owner(state, env, view);
  }
  jobject java_canvas =
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node == nullptr || begin_recording == nullptr ||
              env->ExceptionCheck()
          ? nullptr
          : env->CallObjectMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node, begin_recording, width,
                                  height);
  if (java_canvas == nullptr || native_render_node == nullptr ||
      end_recording == nullptr || set_position == nullptr ||
      env->ExceptionCheck()) {
    if (env->ExceptionCheck()) {
      art::Thread* self = art::Thread::Current();
      if (self != nullptr && self->IsExceptionPending()) {
        std::cerr << "ART HWUI GPU: RenderNode setup exception\n"
                  << self->GetException()->Dump() << "\n";
      }
    }
    if (env->ExceptionCheck()) {
      std::cerr << "ART HWUI GPU: RenderNode begin exception\n";
      art::Thread* self = art::Thread::Current();
      if (self != nullptr && self->IsExceptionPending()) {
        std::cerr << self->GetException()->Dump() << "\n";
      }
    }
    if (java_canvas != nullptr && darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node != nullptr &&
        end_recording != nullptr) {
      env->ExceptionClear();
      env->CallVoidMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node, end_recording);
      env->ExceptionClear();
    }
    env->ExceptionClear();
    env->DeleteLocalRef(animation_host_class);
    env->DeleteLocalRef(animation_host_interface);
    env->DeleteLocalRef(render_node_class);
    std::cerr << "ART HWUI GPU: RenderNode.beginRecording failed\n";
    return JNI_FALSE;
  }
  bool recording_ended = false;
  auto dump_pending_exception = [&](const char* phase) {
    if (!env->ExceptionCheck()) return;
    std::cerr << "ART HWUI GPU: managed exception during " << phase << "\n";
    art::Thread* self = art::Thread::Current();
    if (self != nullptr && self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
  };
  auto finish_recording = [&]() -> bool {
    if (recording_ended) return true;
    // endRecording is the Java-side promotion boundary. Always close a
    // recording, including failure paths, so RenderNode never remains in the
    // "recording in progress" state for the next frame.
    if (env->ExceptionCheck()) env->ExceptionClear();
    env->CallVoidMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node, end_recording);
    recording_ended = true;
    const bool ok = !env->ExceptionCheck();
    if (!ok) env->ExceptionClear();
    return ok;
  };
  env->CallBooleanMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node, set_position, 0, 0, width, height);
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    finish_recording();
    env->DeleteLocalRef(java_canvas);
    env->DeleteLocalRef(animation_host_class);
    env->DeleteLocalRef(animation_host_interface);
    env->DeleteLocalRef(render_node_class);
    std::cerr << "ART HWUI GPU: RenderNode.setPosition failed\n";
    return JNI_FALSE;
  }
  jclass view_class = env->FindClass("android/view/View");
  jmethodID draw = view_class == nullptr
                       ? nullptr
                       : env->GetMethodID(view_class, "draw",
                                          "(Landroid/graphics/Canvas;)V");
  jmethodID measure = view_class == nullptr
                          ? nullptr
                          : env->GetMethodID(view_class, "measure", "(II)V");
  jmethodID layout = view_class == nullptr
                         ? nullptr
                         : env->GetMethodID(view_class, "layout", "(IIII)V");
  jmethodID request_layout =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "requestLayout", "()V");
  jmethodID is_laid_out =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "isLaidOut", "()Z");
  jmethodID set_pressed = view_class == nullptr
                              ? nullptr
                              : env->GetMethodID(view_class, "setPressed", "(Z)V");
  jmethodID perform_click = view_class == nullptr
                                ? nullptr
                                : env->GetMethodID(view_class, "performClick", "()Z");
  jmethodID drawable_hotspot_changed =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "drawableHotspotChanged", "(FF)V");
  auto get_view_field = [&](const char* name) -> jfieldID {
    return view_class == nullptr || env->ExceptionCheck()
               ? nullptr
               : env->GetFieldID(view_class, name, "I");
  };
  jfieldID view_left = get_view_field("mLeft");
  jfieldID view_top = get_view_field("mTop");
  jfieldID view_right = get_view_field("mRight");
  jfieldID view_bottom = get_view_field("mBottom");
  if (draw == nullptr || measure == nullptr || layout == nullptr ||
      request_layout == nullptr ||
      is_laid_out == nullptr || is_layout_requested == nullptr ||
      view_left == nullptr || view_top == nullptr || view_right == nullptr ||
      view_bottom == nullptr ||
      env->ExceptionCheck()) {
    env->ExceptionClear();
    finish_recording();
    env->DeleteLocalRef(view_class);
    env->DeleteLocalRef(java_canvas);
    env->DeleteLocalRef(animation_host_class);
    env->DeleteLocalRef(animation_host_interface);
    env->DeleteLocalRef(render_node_class);
    return JNI_FALSE;
  }
  const bool first_recording = !darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded;
  // Recording a dirty display list is not itself a layout traversal. Android
  // ViewRootImpl measures/layouts only when the hierarchy requests it; doing
  // so on every Metal frame cancels ViewPager's active fake drag and its
  // ValueAnimator. Preserve the same distinction for the detached owner.
  const bool needs_layout =
      first_recording || env->CallBooleanMethod(view, is_laid_out) != JNI_TRUE ||
      env->CallBooleanMethod(view, is_layout_requested) == JNI_TRUE;
  // ViewPager populates its page children during Activity setup, after the
  // detached content root may already have received its one synthetic layout.
  // A real ViewRoot would schedule a second traversal; force that same initial
  // measure/layout before recording so weighted GridLayout pages get bounds.
  if (animation_host_class != nullptr && prepare_view_pagers != nullptr &&
      !env->ExceptionCheck()) {
    env->CallStaticVoidMethod(animation_host_class, prepare_view_pagers, view);
    if (env->ExceptionCheck()) {
      dump_pending_exception("ProbeAnimationHost.prepareViewPagers");
      env->ExceptionClear();
    }
  }
  if (needs_layout && !env->ExceptionCheck()) {
    constexpr jint kMeasureExactly = 0x40000000;
    const jint width_spec = kMeasureExactly | (width & 0x3fffffff);
    const jint height_spec = kMeasureExactly | (height & 0x3fffffff);
    // View.measure() may legitimately return early when the detached root
    // still carries the previous specs. requestLayout() sets the same force
    // bit that ViewRootImpl would set before its follow-up traversal, allowing
    // ViewPager/GridLayout descendants to recompute their page widths.
    env->CallVoidMethod(view, request_layout);
    env->CallVoidMethod(view, measure, width_spec, height_spec);
    dump_pending_exception("View.measure");
    env->SetIntField(view, view_left, 0);
    env->SetIntField(view, view_top, 0);
    env->SetIntField(view, view_right, width);
    env->SetIntField(view, view_bottom, height);
    env->CallVoidMethod(view, layout, 0, 0, width, height);
  }
  if (env->ExceptionCheck()) {
    dump_pending_exception("View.layout");
    env->ExceptionClear();
    finish_recording();
    env->DeleteLocalRef(view_class);
    env->DeleteLocalRef(java_canvas);
    env->DeleteLocalRef(animation_host_class);
    env->DeleteLocalRef(animation_host_interface);
    env->DeleteLocalRef(render_node_class);
    std::cerr << "ART HWUI GPU: View measure/layout failed\n";
    return JNI_FALSE;
  }
  // The first pass establishes the ViewPager client width. Populate once
  // more after that width is known, then repeat layout so its page adapter can
  // assign widthFactor and concrete bounds to GridLayout/button descendants.
  if (first_recording && animation_host_class != nullptr &&
      prepare_view_pagers != nullptr && !env->ExceptionCheck()) {
    env->CallStaticVoidMethod(animation_host_class, prepare_view_pagers, view);
    if (env->ExceptionCheck()) {
      dump_pending_exception("ProbeAnimationHost.prepareViewPagers(post-measure)");
      env->ExceptionClear();
    } else {
      env->CallVoidMethod(view, request_layout);
      env->CallVoidMethod(view, measure,
                          0x40000000 | (width & 0x3fffffff),
                          0x40000000 | (height & 0x3fffffff));
      env->CallVoidMethod(view, layout, 0, 0, width, height);
    }
  }
  if (std::getenv("DARWIN_ART_SKOTTIE_METAL") != nullptr) {
    darwin_art_hwui::hide_skottie_backing_views(env, view);
  }
  // A real ViewRoot performs the first ViewPager population while the
  // hierarchy is attached to a window.  This standalone probe has no
  // ViewRoot, so support-library ViewPager's drawing-order cache would remain
  // null and its getChildDrawingOrder() would throw during DecorView.draw().
  // Re-run the same public population hook on every pager in the hierarchy;
  // this changes no APK code and keeps the Android view traversal intact.
  // A real ViewRoot owns an app Looper and window-system thread. This
  // detached host intentionally does not synthesize one; the content root
  // is measured/layouted directly and recorded into the persistent RenderNode.
  if (pending_root_traversal < 0 && begin_host_traversal != nullptr &&
      !env->ExceptionCheck()) {
    env->CallStaticVoidMethod(animation_host_class, begin_host_traversal, view);
    if (env->ExceptionCheck()) {
      dump_pending_exception("ProbeAnimationHost.beginHostTraversal");
      env->ExceptionClear();
    }
  }
  if (dispatch_pre_draw != nullptr && !env->ExceptionCheck()) {
    env->CallStaticBooleanMethod(animation_host_class, dispatch_pre_draw, view);
    if (env->ExceptionCheck()) {
      dump_pending_exception("ProbeAnimationHost.dispatchPreDraw");
      env->ExceptionClear();
    }
  }
  // Preserve Android's RenderNode invalidation model. A dirty child rebuilds
  // its own display list and the retained root references that child node;
  // forcing the entire hierarchy dirty here flattened every frame into a new
  // root recording. Besides defeating HWUI's retained rendering, that made a
  // Chromium tab transition submit tens of thousands of display-list bytes
  // on every vsync. Detached legacy probes without hierarchy bookkeeping are
  // initialized dirty by their first recording and no longer need this
  // compatibility invalidation either.
  const uint32_t pending_pressed_action = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_action;
  const jfloat pending_pressed_x = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_x;
  const jfloat pending_pressed_y = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_y;
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_action = 0;
  if (pending_pressed_action != 0 && darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view != nullptr &&
      set_pressed != nullptr && !env->ExceptionCheck()) {
    if (drawable_hotspot_changed != nullptr) {
      env->CallVoidMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view, drawable_hotspot_changed,
                          pending_pressed_x, pending_pressed_y);
      dump_pending_exception("View.drawableHotspotChanged");
    }
    env->CallVoidMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view, set_pressed,
                        pending_pressed_action == 1 ? JNI_TRUE : JNI_FALSE);
    dump_pending_exception("View.setPressed");
  }
  if (dispatch_on_draw != nullptr && !env->ExceptionCheck()) {
    env->CallStaticVoidMethod(animation_host_class, dispatch_on_draw, view);
    if (env->ExceptionCheck()) {
      dump_pending_exception("ProbeAnimationHost.dispatchOnDraw");
      env->ExceptionClear();
    }
  }
  env->CallVoidMethod(view, draw, java_canvas);
  dump_pending_exception("View.draw");
  const bool layers_ok = !env->ExceptionCheck() &&
                         draw_window_manager_layers(
                             env, java_canvas, view_class, draw,
                             animation_host_class, invalidate_view_tree);
  dump_pending_exception("WindowManager layers");
  const bool draw_ok = layers_ok && !env->ExceptionCheck();
  const bool recording_ok = finish_recording();
  env->DeleteLocalRef(view_class);
  env->DeleteLocalRef(java_canvas);
  env->DeleteLocalRef(animation_host_class);
  env->DeleteLocalRef(animation_host_interface);
  env->DeleteLocalRef(render_node_class);
  if (!draw_ok || !recording_ok) {
    std::cerr << "ART HWUI GPU: View.draw failed\n";
    env->ExceptionClear();
    return JNI_FALSE;
  }

  auto* node = reinterpret_cast<android::uirenderer::RenderNode*>(
      static_cast<std::uintptr_t>(env->GetLongField(
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node, native_render_node)));
  if (node == nullptr) {
    std::cerr << "ART HWUI GPU: Java RenderNode native pointer missing\n";
    return JNI_FALSE;
  }
  node->mValid = true;
  const bool debug_display_lists =
      std::getenv("DARWIN_ART_DEBUG_HWUI_DISPLAY_LISTS") != nullptr;
  if (debug_display_lists) {
    std::cerr << "ART HWUI GPU: node staging needs="
              << node->mNeedsDisplayListSync
              << " stagingContent=" << node->mStagingDisplayList.hasContent()
              << " stagingSize=" << node->mStagingDisplayList.getUsedSize()
              << " activeContent=" << node->mDisplayList.hasContent() << "\n";
  }
  const size_t synchronized_nodes =
      darwin_art_hwui::sync_recorded_render_node_tree(node);
  if (debug_display_lists) {
    std::cerr << "ART HWUI GPU: synchronized RenderNodes="
              << synchronized_nodes
              << " activeSize=" << node->mDisplayList.getUsedSize()
              << " holePunches=" << node->hasHolePunches()
              << " childNodes="
              << (node->mDisplayList.asSkiaDl() == nullptr
                      ? 0
                      : node->mDisplayList.asSkiaDl()->mChildNodes.size())
              << "\n";
  }
  if (!node->mDisplayList || node->mDisplayList.isEmpty()) {
    std::cerr << "ART HWUI GPU: Java RenderNode produced empty display list\n";
    return JNI_FALSE;
  }
  if (darwin_art_hwui::node_subtree_has_animators(node)) {
    if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context == nullptr) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord = std::make_unique<
          android::uirenderer::renderthread::TimeLord>();
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord->setFrameInterval(16666666);
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context =
          std::make_unique<android::uirenderer::AnimationContext>(
              *darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord);
    }
    darwin_art_hwui::register_node_subtree_animators(
        node, *darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context);
  }

  if (!darwin_art_hwui::render_node_to_surface(
          env, view, fixture->gpu_render_node, fixture->gpu_surface, width, height,
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_active, darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_x,
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_y, darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_started)) {
    std::cerr << "ART HWUI GPU: drawable submit failed\n";
    return JNI_FALSE;
  }
  // Direct Metal presentation intentionally does not invoke the CPU frame
  // callback. Publish only the drawable dimensions so GPU-only APK
  // acceptance can validate the presented surface without a readback.
  darwin_art_frame_probe::record_dimensions(width, height);
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded = true;
  if (traversal_generation >= 0) {
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_last_traversal_barrier = traversal_generation;
  }
  return JNI_TRUE;
}
#else
int prepare_gpu_surface(GraphicsState*, jint, jint) { return 0; }
int refresh_gpu_surface_identity(GraphicsState*) { return 0; }
void debug_product_view_root(JNIEnv*, jobject) {}
#endif

#if !defined(DARWIN_ART_REAL_GRAPHICS)
jboolean present_gpu_content(GraphicsState*, JNIEnv*, jobject, jint, jint) {
  return JNI_FALSE;
}
#endif

}  // namespace darwin_art_graphics
