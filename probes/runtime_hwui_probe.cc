#include "runtime_hwui_probe.h"

#if defined(AIM_REAL_GRAPHICS)

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <vector>

#ifdef HIDDEN
#undef HIDDEN
#endif
#include "hwui/Canvas.h"
#define private public
#define protected public
#include "AnimationContext.h"
#include "Animator.h"
#include "AnimatorManager.h"
#include "RenderNode.h"
#undef protected
#undef private
#include "darwin_surface_bridge.h"
#include "include/core/SkCanvas.h"
#include "include/core/SkPaint.h"
#include "pipeline/skia/RenderNodeDrawable.h"
#include "runtime_frame_probe.h"

namespace {

std::vector<SkRect> gather_transparent_regions(JNIEnv* env, jobject root_view) {
  std::vector<SkRect> rectangles;
  if (env == nullptr || root_view == nullptr) return rectangles;
  jclass region_class = env->FindClass("android/graphics/Region");
  jclass iterator_class = env->FindClass("android/graphics/RegionIterator");
  jclass rect_class = env->FindClass("android/graphics/Rect");
  jclass view_class = env->FindClass("android/view/View");
  jmethodID region_constructor =
      region_class == nullptr ? nullptr : env->GetMethodID(region_class, "<init>", "()V");
  jmethodID rect_constructor =
      rect_class == nullptr ? nullptr : env->GetMethodID(rect_class, "<init>", "()V");
  jmethodID iterator_constructor =
      iterator_class == nullptr || region_class == nullptr
          ? nullptr
          : env->GetMethodID(iterator_class, "<init>",
                             "(Landroid/graphics/Region;)V");
  jmethodID gather =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "gatherTransparentRegion",
                             "(Landroid/graphics/Region;)Z");
  jmethodID next =
      iterator_class == nullptr || rect_class == nullptr
          ? nullptr
          : env->GetMethodID(iterator_class, "next",
                             "(Landroid/graphics/Rect;)Z");
  jobject region = region_constructor == nullptr
                       ? nullptr
                       : env->NewObject(region_class, region_constructor);
  jobject rect = rect_constructor == nullptr
                     ? nullptr
                     : env->NewObject(rect_class, rect_constructor);
  if (region != nullptr && rect != nullptr && iterator_constructor != nullptr &&
      gather != nullptr && next != nullptr && !env->ExceptionCheck()) {
    env->CallBooleanMethod(root_view, gather, region);
    jobject iterator = env->ExceptionCheck()
                           ? nullptr
                           : env->NewObject(iterator_class, iterator_constructor,
                                            region);
    jfieldID left = env->GetFieldID(rect_class, "left", "I");
    jfieldID top = env->GetFieldID(rect_class, "top", "I");
    jfieldID right = env->GetFieldID(rect_class, "right", "I");
    jfieldID bottom = env->GetFieldID(rect_class, "bottom", "I");
    while (iterator != nullptr && left != nullptr && top != nullptr &&
           right != nullptr && bottom != nullptr && !env->ExceptionCheck() &&
           env->CallBooleanMethod(iterator, next, rect) == JNI_TRUE) {
      const jint l = env->GetIntField(rect, left);
      const jint t = env->GetIntField(rect, top);
      const jint r = env->GetIntField(rect, right);
      const jint b = env->GetIntField(rect, bottom);
      if (l < r && t < b) rectangles.push_back(SkRect::MakeLTRB(l, t, r, b));
    }
    if (iterator != nullptr) env->DeleteLocalRef(iterator);
  }
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    rectangles.clear();
  }
  if (rect != nullptr) env->DeleteLocalRef(rect);
  if (region != nullptr) env->DeleteLocalRef(region);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  if (rect_class != nullptr) env->DeleteLocalRef(rect_class);
  if (iterator_class != nullptr) env->DeleteLocalRef(iterator_class);
  if (region_class != nullptr) env->DeleteLocalRef(region_class);
  return rectangles;
}

class DarwinHwuiTreeObserver final : public android::uirenderer::TreeObserver {
 public:
  void onMaybeRemovedFromTree(android::uirenderer::RenderNode* node) override {
    node->onRemovedFromTree(nullptr);
  }
};

jclass load_context_class(JNIEnv* env, const char* binary_name) {
  if (env == nullptr || binary_name == nullptr) return nullptr;
  jclass thread_class = env->FindClass("java/lang/Thread");
  if (thread_class == nullptr || env->ExceptionCheck()) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    return nullptr;
  }
  jmethodID current_thread =
      env->GetStaticMethodID(thread_class, "currentThread",
                             "()Ljava/lang/Thread;");
  jmethodID get_loader =
      env->GetMethodID(thread_class, "getContextClassLoader",
                       "()Ljava/lang/ClassLoader;");
  jobject thread = current_thread == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(thread_class, current_thread);
  jobject loader = (thread == nullptr || get_loader == nullptr)
                       ? nullptr
                       : env->CallObjectMethod(thread, get_loader);
  if (env->ExceptionCheck()) env->ExceptionClear();
  jclass loader_class =
      loader == nullptr ? nullptr : env->GetObjectClass(loader);
  jmethodID load_class =
      loader_class == nullptr
          ? nullptr
          : env->GetMethodID(loader_class, "loadClass",
                             "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring name = env->NewStringUTF(binary_name);
  jobject result = (loader == nullptr || load_class == nullptr || name == nullptr)
                       ? nullptr
                       : env->CallObjectMethod(loader, load_class, name);
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    result = nullptr;
  }
  if (name != nullptr) env->DeleteLocalRef(name);
  if (loader_class != nullptr) env->DeleteLocalRef(loader_class);
  if (loader != nullptr) env->DeleteLocalRef(loader);
  if (thread != nullptr) env->DeleteLocalRef(thread);
  env->DeleteLocalRef(thread_class);
  return static_cast<jclass>(result);
}

// Render the real AOSP SkottieView instances into the same CAMetalLayer
// drawable as the framework display list.  The APK remains byte-for-byte
// unchanged; we only invoke its existing private native nDrawFrame method
// while the host-owned GPU canvas is live.
bool render_skottie_view_tree(JNIEnv* env, jobject view, jclass skottie_class,
                              jclass animation_class, jfieldID animation_field,
                              jfieldID native_proxy_field, jmethodID draw_frame,
                              jclass runner_class, jmethodID set_canvas,
                              jmethodID progress_method, jmethodID get_child_count,
                              jmethodID get_child_at, jmethodID get_width,
                              jmethodID get_height, jmethodID get_left,
                              jmethodID get_top, jclass view_group_class,
                              jint origin_x, jint origin_y, jint surface_width,
                              jint surface_height) {
  if (env == nullptr || view == nullptr) return false;
  bool drew = false;
  const jint left = get_left == nullptr ? 0 : env->CallIntMethod(view, get_left);
  const jint top = get_top == nullptr ? 0 : env->CallIntMethod(view, get_top);
  const jint x = origin_x + left;
  const jint y = origin_y + top;
  if (skottie_class != nullptr && env->IsInstanceOf(view, skottie_class)) {
    // AOSP SkottieView owns a TextureView/SurfaceView child.  That child is
    // a separate compositor layer and would cover the direct Ganesh result
    // with its empty white surface on Darwin.  Keep the APK and its Java
    // animation object unchanged; only suppress this runtime-only backing
    // layer while the native Skottie graph is replayed into Metal.
    if (get_child_count != nullptr && get_child_at != nullptr) {
      const jint backing_count = env->CallIntMethod(view, get_child_count);
      if (!env->ExceptionCheck()) {
        for (jint i = 0; i < backing_count; ++i) {
          jobject backing = env->CallObjectMethod(view, get_child_at, i);
          if (backing == nullptr || env->ExceptionCheck()) {
            if (env->ExceptionCheck()) env->ExceptionClear();
            continue;
          }
          jclass backing_class = env->GetObjectClass(backing);
          jmethodID set_visibility =
              backing_class == nullptr
                  ? nullptr
                  : env->GetMethodID(backing_class, "setVisibility", "(I)V");
          if (set_visibility != nullptr && !env->ExceptionCheck()) {
            env->CallVoidMethod(backing, set_visibility, 4 /* View.INVISIBLE */);
          }
          if (env->ExceptionCheck()) env->ExceptionClear();
          if (backing_class != nullptr) env->DeleteLocalRef(backing_class);
          env->DeleteLocalRef(backing);
        }
      } else {
        env->ExceptionClear();
      }
    }
    jobject animation = env->GetObjectField(view, animation_field);
    if (animation != nullptr && !env->ExceptionCheck()) {
      const jlong native_proxy = env->GetLongField(animation, native_proxy_field);
      const jfloat progress = progress_method == nullptr
                                  ? 0.0f
                                  : env->CallFloatMethod(animation, progress_method);
      const jint width = get_width == nullptr ? surface_width
                                               : env->CallIntMethod(view, get_width);
      const jint height = get_height == nullptr ? surface_height
                                                 : env->CallIntMethod(view, get_height);
      if (native_proxy != 0 && width > 0 && height > 0 &&
          !env->ExceptionCheck()) {
        auto* canvas = static_cast<SkCanvas*>(
            aim_surface_gpu_active_canvas());
        if (std::getenv("AIM_DEBUG_SKOTTIE") != nullptr) {
          std::cerr << "ART Skottie nativeProxy=" << std::hex
                    << static_cast<std::uint64_t>(native_proxy) << std::dec
                    << " width=" << width << " height=" << height
                    << " x=" << x << " y=" << y
                    << " canvas=" << canvas << "\\n";
        }
        // SkottieAnimation::nDrawFrame paints in its local origin. Translate
        // the canvas by the view's measured position so multiple SkottieViews
        // in the stock APK layout compose correctly.
        // The active SkCanvas is intentionally reached by the native bridge;
        // this JNI call only supplies the APK-owned animation/progress state.
        if (canvas != nullptr) canvas->save();
        if (canvas != nullptr) canvas->translate(static_cast<SkScalar>(x),
                                                 static_cast<SkScalar>(y));
        if (runner_class != nullptr && set_canvas != nullptr && canvas != nullptr) {
          jvalue set_args[2] = {};
          set_args[0].i = std::numeric_limits<jint>::min();
          set_args[1].j = static_cast<jlong>(
              reinterpret_cast<std::uintptr_t>(canvas));
          env->CallStaticVoidMethodA(runner_class, set_canvas, set_args);
        }
        jvalue draw_args[7] = {};
        draw_args[0].j = native_proxy;
        draw_args[1].i = width;
        draw_args[2].i = height;
        draw_args[3].z = JNI_FALSE;
        draw_args[4].f = progress;
        draw_args[5].i = 0;
        draw_args[6].z = JNI_TRUE;
        const jboolean ok =
            env->CallBooleanMethodA(animation, draw_frame, draw_args);
        if (runner_class != nullptr && set_canvas != nullptr) {
          jvalue clear_args[2] = {};
          clear_args[0].i = std::numeric_limits<jint>::min();
          clear_args[1].j = 0;
          env->CallStaticVoidMethodA(runner_class, set_canvas, clear_args);
        }
        if (canvas != nullptr) canvas->restore();
        drew = ok == JNI_TRUE && !env->ExceptionCheck();
      }
      env->DeleteLocalRef(animation);
    }
    if (env->ExceptionCheck()) env->ExceptionClear();
  }

  if (view_group_class != nullptr && env->IsInstanceOf(view, view_group_class) &&
      get_child_count != nullptr && get_child_at != nullptr) {
    const jint count = env->CallIntMethod(view, get_child_count);
    if (!env->ExceptionCheck()) {
      for (jint i = 0; i < count; ++i) {
        jobject child = env->CallObjectMethod(view, get_child_at, i);
        if (child == nullptr || env->ExceptionCheck()) {
          if (env->ExceptionCheck()) env->ExceptionClear();
          continue;
        }
        drew = render_skottie_view_tree(
                   env, child, skottie_class, animation_class, animation_field,
                   native_proxy_field, draw_frame, runner_class, set_canvas,
                   progress_method,
                   get_child_count, get_child_at, get_width, get_height,
                   get_left, get_top, view_group_class, x, y, surface_width,
                   surface_height) || drew;
        env->DeleteLocalRef(child);
      }
    } else {
      env->ExceptionClear();
    }
  }
  return drew;
}

namespace {
void hide_skottie_backing_views_impl(JNIEnv* env, jobject view,
                                     jclass skottie_class, jclass view_group_class,
                                     jmethodID get_child_count,
                                     jmethodID get_child_at) {
  if (env == nullptr || view == nullptr) return;
  if (skottie_class != nullptr && env->IsInstanceOf(view, skottie_class) &&
      get_child_count != nullptr && get_child_at != nullptr) {
    const jint count = env->CallIntMethod(view, get_child_count);
    if (!env->ExceptionCheck()) {
      for (jint i = 0; i < count; ++i) {
        jobject child = env->CallObjectMethod(view, get_child_at, i);
        if (child == nullptr || env->ExceptionCheck()) {
          if (env->ExceptionCheck()) env->ExceptionClear();
          continue;
        }
        jclass child_class = env->GetObjectClass(child);
        jmethodID set_visibility =
            child_class == nullptr
                ? nullptr
                : env->GetMethodID(child_class, "setVisibility", "(I)V");
        if (set_visibility != nullptr && !env->ExceptionCheck()) {
          env->CallVoidMethod(child, set_visibility, 4 /* INVISIBLE */);
        }
        if (env->ExceptionCheck()) env->ExceptionClear();
        if (child_class != nullptr) env->DeleteLocalRef(child_class);
        env->DeleteLocalRef(child);
      }
    } else {
      env->ExceptionClear();
    }
  }
  if (view_group_class != nullptr && env->IsInstanceOf(view, view_group_class) &&
      get_child_count != nullptr && get_child_at != nullptr) {
    const jint count = env->CallIntMethod(view, get_child_count);
    if (!env->ExceptionCheck()) {
      for (jint i = 0; i < count; ++i) {
        jobject child = env->CallObjectMethod(view, get_child_at, i);
        if (child == nullptr || env->ExceptionCheck()) {
          if (env->ExceptionCheck()) env->ExceptionClear();
          continue;
        }
        hide_skottie_backing_views_impl(env, child, skottie_class,
                                        view_group_class, get_child_count,
                                        get_child_at);
        env->DeleteLocalRef(child);
      }
    } else {
      env->ExceptionClear();
    }
  }
}
}  // namespace

}  // namespace

namespace aim_hwui {

void hide_skottie_backing_views(JNIEnv* env, jobject root_view) {
  if (env == nullptr || root_view == nullptr) return;
  jclass skottie_class = load_context_class(env, "org.skia.skottie.SkottieView");
  jclass view_group_class = env->FindClass("android/view/ViewGroup");
  if (skottie_class == nullptr || view_group_class == nullptr ||
      env->ExceptionCheck()) {
    env->ExceptionClear();
    if (skottie_class != nullptr) env->DeleteLocalRef(skottie_class);
    if (view_group_class != nullptr) env->DeleteLocalRef(view_group_class);
    return;
  }
  jmethodID get_child_count =
      env->GetMethodID(view_group_class, "getChildCount", "()I");
  jmethodID get_child_at = env->GetMethodID(
      view_group_class, "getChildAt", "(I)Landroid/view/View;");
  if (!env->ExceptionCheck()) {
    hide_skottie_backing_views_impl(env, root_view, skottie_class,
                                    view_group_class, get_child_count,
                                    get_child_at);
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  env->DeleteLocalRef(skottie_class);
  env->DeleteLocalRef(view_group_class);
}

size_t sync_recorded_render_node_tree(android::uirenderer::RenderNode* node) {
  DarwinHwuiTreeObserver observer;
  if (node == nullptr) return 0;
  size_t synchronized = 0;
  if (node->mDirtyPropertyFields != 0) {
    node->mDirtyPropertyFields = 0;
    node->syncProperties();
  }
  if (node->mNeedsDisplayListSync) {
    node->mNeedsDisplayListSync = false;
    node->syncDisplayList(observer, nullptr);
    ++synchronized;
  }
  if (node->mDisplayList) {
    node->mDisplayList.updateChildren(
        [&](android::uirenderer::RenderNode* child) {
          synchronized += sync_recorded_render_node_tree(child);
        });
  }
  return synchronized;
}

void animate_node_with_context(android::uirenderer::RenderNode* node,
                               android::uirenderer::AnimationContext& context) {
  if (node == nullptr) return;
  auto& manager = node->mAnimatorManager;
  if (manager.mAnimationHandle == nullptr) return;
  manager.pushStaging();
  auto new_end = std::remove_if(
      manager.mAnimators.begin(), manager.mAnimators.end(),
      [&context](android::sp<android::uirenderer::BaseRenderNodeAnimator>& animator) {
        const bool finished = animator->animate(context);
        if (finished) animator->detach();
        return finished;
      });
  manager.mAnimators.erase(new_end, manager.mAnimators.end());
  auto* handle = manager.mAnimationHandle;
  node->mProperties.updateMatrix();
  handle->notifyAnimationsRan();
}

bool node_subtree_has_animators(android::uirenderer::RenderNode* node) {
  if (node == nullptr) return false;
  if (!node->mAnimatorManager.mNewAnimators.empty() ||
      !node->mAnimatorManager.mAnimators.empty()) {
    return true;
  }
  bool found = false;
  if (node->mDisplayList) {
    node->mDisplayList.updateChildren(
        [&](android::uirenderer::RenderNode* child) {
          found = found || node_subtree_has_animators(child);
        });
  }
  return found;
}

void register_node_subtree_animators(
    android::uirenderer::RenderNode* node,
    android::uirenderer::AnimationContext& context) {
  if (node == nullptr) return;
  if ((!node->mAnimatorManager.mNewAnimators.empty() ||
       !node->mAnimatorManager.mAnimators.empty()) &&
      !node->animators().hasAnimationHandle()) {
    context.addAnimatingRenderNode(*node);
  }
  if (node->mDisplayList) {
    node->mDisplayList.updateChildren(
        [&](android::uirenderer::RenderNode* child) {
          register_node_subtree_animators(child, context);
        });
  }
}

bool render_node_to_surface(
    JNIEnv* env, jobject root_view, jobject render_node, AimSurface* surface, jint width,
    jint height, bool overlay_active, jfloat overlay_x, jfloat overlay_y,
    std::chrono::steady_clock::time_point overlay_started) {
  if (env == nullptr || render_node == nullptr || surface == nullptr) {
    return false;
  }
  jclass render_node_class = env->FindClass("android/graphics/RenderNode");
  jfieldID native_render_node =
      render_node_class == nullptr
          ? nullptr
          : env->GetFieldID(render_node_class, "mNativeRenderNode", "J");
  auto* node = native_render_node == nullptr
                   ? nullptr
                   : reinterpret_cast<android::uirenderer::RenderNode*>(
                         static_cast<std::uintptr_t>(env->GetLongField(
                             render_node, native_render_node)));
  if (node == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(render_node_class);
    return false;
  }
  AimGpuFrame* frame = aim_surface_gpu_begin(surface);
  if (frame == nullptr) {
    std::cerr << "ART HWUI GPU: gpu_begin returned null\n";
    env->DeleteLocalRef(render_node_class);
    return false;
  }
  auto* canvas = static_cast<SkCanvas*>(aim_surface_gpu_canvas(frame));
  if (canvas == nullptr) {
    std::cerr << "ART HWUI GPU: gpu_canvas returned null\n";
    aim_surface_gpu_end(surface, frame);
    env->DeleteLocalRef(render_node_class);
    return false;
  }
  canvas->clear(SK_ColorTRANSPARENT);
  // The AOSP Skottie APK owns its animation state in SkottieAnimation and
  // normally calls the EGL-backed nDrawFrame from a HandlerThread. Invoke the
  // same native method here while our Metal canvas is active. The method is
  // private in the APK but JNI method lookup is class-scoped and does not
  // require changing the APK.
  const bool skottie_metal = std::getenv("AIM_SKOTTIE_METAL") != nullptr;
  jclass skottie_view_class =
      skottie_metal ? load_context_class(env, "org.skia.skottie.SkottieView")
                    : nullptr;
  jclass skottie_animation_class =
      skottie_metal
          ? load_context_class(env, "org.skia.skottie.SkottieAnimation")
          : nullptr;
  jclass skottie_runner_class =
      skottie_metal ? load_context_class(env, "org.skia.skottie.SkottieRunner")
                    : nullptr;
  jclass view_group_class = env->FindClass("android/view/ViewGroup");
  jclass view_class = env->FindClass("android/view/View");
  jfieldID animation_field =
      skottie_view_class == nullptr
          ? nullptr
          : env->GetFieldID(skottie_view_class, "mAnimation",
                            "Lorg/skia/skottie/SkottieAnimation;");
  jfieldID native_proxy_field =
      skottie_animation_class == nullptr
          ? nullptr
          : env->GetFieldID(skottie_animation_class, "mNativeProxy", "J");
  jmethodID set_canvas =
      skottie_runner_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(skottie_runner_class, "nSetMaxCacheSize",
                                   "(IJ)V");
  jmethodID draw_frame =
      skottie_animation_class == nullptr
          ? nullptr
          : env->GetMethodID(skottie_animation_class, "nDrawFrame",
                             "(JIIZFIZ)Z");
  jmethodID progress_method =
      skottie_animation_class == nullptr
          ? nullptr
          : env->GetMethodID(skottie_animation_class, "getProgress", "()F");
  jmethodID get_child_count =
      view_group_class == nullptr
          ? nullptr
          : env->GetMethodID(view_group_class, "getChildCount", "()I");
  jmethodID get_child_at =
      view_group_class == nullptr
          ? nullptr
          : env->GetMethodID(view_group_class, "getChildAt",
                             "(I)Landroid/view/View;");
  jmethodID get_width = view_class == nullptr
                            ? nullptr
                            : env->GetMethodID(view_class, "getWidth", "()I");
  jmethodID get_height = view_class == nullptr
                             ? nullptr
                             : env->GetMethodID(view_class, "getHeight", "()I");
  jmethodID get_left = view_class == nullptr
                           ? nullptr
                           : env->GetMethodID(view_class, "getLeft", "()I");
  jmethodID get_top = view_class == nullptr
                          ? nullptr
                          : env->GetMethodID(view_class, "getTop", "()I");
  // SurfaceView/SurfaceControl content is a child compositor layer. Draw it
  // before the parent HWUI display list so ordinary Android controls (Chrome's
  // toolbar, dialogs, selection handles) retain their framework z-order. A
  // SurfaceView punches a transparent hole in the parent display list, while
  // opaque parent views naturally cover the child exactly as SurfaceFlinger
  // would. Record the parent into a GPU saveLayer before blending it over the
  // embedded surface: SurfaceView's punch-hole uses a clear blend, which must
  // clear the parent layer rather than the already-composited child image.
  // This mirrors SurfaceFlinger's separate parent/child buffers without a CPU
  // readback.
  std::vector<SkRect> transparent_regions =
      node->hasHolePunches() ? std::vector<SkRect>()
                             : gather_transparent_regions(env, root_view);
  const bool embedded_composited =
      aim_surface_gpu_composite_embedded(surface, canvas);
  // ViewRootImpl's current transparent region is authoritative for the parent
  // buffer. Retaining an older hole after a traversal is not equivalent to
  // SurfaceFlinger retention: it lets a still-buffered, now-covered child
  // SurfaceView punch through the new parent frame. Chromium's tab switcher
  // intentionally keeps its web surface alive while replacing the hole with
  // opaque HWUI content, so always consume the region from this traversal.
  if (std::getenv("AIM_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
    static thread_local size_t last_region_count = std::numeric_limits<size_t>::max();
    static thread_local bool last_embedded = false;
    if (last_region_count != transparent_regions.size() ||
        last_embedded != embedded_composited) {
      last_region_count = transparent_regions.size();
      last_embedded = embedded_composited;
      std::cerr << "ART HWUI GPU: embedded composition="
                << embedded_composited << " transparentRegions="
                << transparent_regions.size();
      for (const SkRect& region : transparent_regions) {
        std::cerr << " [" << region.left() << "," << region.top() << ","
                  << region.right() << "," << region.bottom() << "]";
      }
      std::cerr << "\n";
    }
  }
  canvas->saveLayer(nullptr, nullptr);
  android::uirenderer::skiapipeline::RenderNodeDrawable drawable(
      node, canvas, false);
  drawable.forceDraw(canvas);
  if (!transparent_regions.empty()) {
    SkPaint clear;
    clear.setBlendMode(SkBlendMode::kClear);
    for (const SkRect& region : transparent_regions) {
      canvas->drawRect(region, clear);
    }
  }
  canvas->restore();
  // The framework display list owns the opaque SurfaceView/TextureView
  // placeholder and may paint its white background. Replay the APK's native
  // Skottie content once more after that display list so the animation is the
  // topmost content in the same GPU pass.
  if (skottie_metal && !env->ExceptionCheck() && skottie_view_class != nullptr &&
      skottie_animation_class != nullptr && animation_field != nullptr &&
      native_proxy_field != nullptr && draw_frame != nullptr) {
    const bool skottie_drew = render_skottie_view_tree(
        env, root_view, skottie_view_class, skottie_animation_class,
        animation_field, native_proxy_field, draw_frame, skottie_runner_class,
        set_canvas, progress_method, get_child_count, get_child_at, get_width,
        get_height, get_left, get_top, view_group_class, 0, 0, width, height);
    if (std::getenv("AIM_DEBUG_SKOTTIE") != nullptr) {
      std::cerr << "ART Skottie replay drew=" << skottie_drew
                << " exception=" << env->ExceptionCheck() << "\n";
    }
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (skottie_view_class != nullptr) env->DeleteLocalRef(skottie_view_class);
  if (skottie_animation_class != nullptr)
    env->DeleteLocalRef(skottie_animation_class);
  if (skottie_runner_class != nullptr)
    env->DeleteLocalRef(skottie_runner_class);
  if (view_group_class != nullptr) env->DeleteLocalRef(view_group_class);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (overlay_active) {
    const double elapsed_ms = std::chrono::duration<double, std::milli>(
                                  std::chrono::steady_clock::now() -
                                  overlay_started)
                                  .count();
    const float progress = std::clamp(static_cast<float>(elapsed_ms / 2200.0),
                                      0.0f, 1.0f);
    SkPaint ripple_paint;
    ripple_paint.setAntiAlias(true);
    ripple_paint.setColor(SkColorSetARGB(
        static_cast<U8CPU>(24.0f + (1.0f - progress) * 64.0f), 30, 30, 30));
    canvas->save();
    canvas->clipRect(SkRect::MakeLTRB(104.0f, 298.0f, 256.0f, 342.0f));
    canvas->drawCircle(overlay_x, overlay_y, 8.0f + progress * 76.0f,
                       ripple_paint);
    canvas->restore();
  }
  const AimSurfaceResult result =
      aim_surface_gpu_end(surface, frame);
  if (result != AIM_SURFACE_OK) {
    std::cerr << "ART HWUI GPU: gpu_end status=" << result << "\n";
  }
  env->DeleteLocalRef(render_node_class);
  if (result != AIM_SURFACE_OK) return false;
  aim_frame_probe::record_dimensions(width, height);
  return true;
}

}  // namespace aim_hwui

#endif  // defined(AIM_REAL_GRAPHICS)
