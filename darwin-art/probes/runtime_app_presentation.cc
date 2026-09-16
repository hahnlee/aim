#include "runtime_app_presentation.h"

#include "darwin_angle_egl.h"
#include "darwin_android_platform.h"

#include <cstdlib>
#include <cstring>
#include <iostream>
#include <unistd.h>

#include "runtime_graphics_phase.h"
#include "runtime_graphics_gpu.h"
#include "runtime_graphics_probe.h"
#include "runtime_graphics_state.h"
#include "runtime_jni_scope.h"
#include "runtime_app_resources.h"
#include "runtime_app_activity.h"
#include "darwin_binder_wire.h"
#include "runtime_process_state.h"
#include "mirror/throwable.h"
#include "obj_ptr-inl.h"
#include "thread-current-inl.h"

namespace darwin_art_presentation {

namespace {

void DescribeArtThrowableChain(art::Thread* self) {
  if (self == nullptr) return;
  art::ObjPtr<art::mirror::Throwable> current = self->GetException();
  for (int depth = 0; current != nullptr && depth < 8; ++depth) {
    std::cerr << "ART Android lifecycle cause[" << depth << "]: "
              << current->Dump() << "\n";
    current = current->GetCause();
  }
}

void EnsureJavaSurfaceValid(JNIEnv* env, jobject surface) {
  if (surface == nullptr) return;
  jclass surface_class = env->GetObjectClass(surface);
  jfieldID native_object =
      surface_class == nullptr
          ? nullptr
          : env->GetFieldID(surface_class, "mNativeObject", "J");
  if (native_object != nullptr &&
      env->GetLongField(surface, native_object) == 0) {
    // AOSP stores a strong native Surface producer in mNativeObject, not an
    // arbitrary identity token. Keep the same ownership shape: each Java
    // Surface owns one distinct managed producer reference, which native
    // lockCanvas and ANativeWindow_fromSurface can consume directly.
    const jint host_width = darwin_art::DarwinAngleHostSurfaceWidth();
    const jint host_height = darwin_art::DarwinAngleHostSurfaceHeight();
    void* producer = darwin_art_android_ANativeWindow_create(
        host_width > 0 ? host_width : 360,
        host_height > 0 ? host_height : 640,
        /*AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM=*/1);
    if (producer != nullptr) {
      env->SetLongField(surface, native_object,
                        reinterpret_cast<jlong>(producer));
      if (env->ExceptionCheck()) {
        darwin_art_android_ANativeWindow_release(producer);
      }
    }
  }
  if (surface_class != nullptr) env->DeleteLocalRef(surface_class);
}

bool VerifySoftwareSurfaceCanvas(JNIEnv* env) {
  jclass surface_class = env->FindClass("android/view/Surface");
  jmethodID surface_constructor =
      surface_class == nullptr
          ? nullptr
          : env->GetMethodID(surface_class, "<init>", "()V");
  jobject surface = surface_constructor == nullptr
                        ? nullptr
                        : env->NewObject(surface_class, surface_constructor);
  if (surface == nullptr || env->ExceptionCheck()) return false;
  EnsureJavaSurfaceValid(env, surface);
  jfieldID native_object =
      env->GetFieldID(surface_class, "mNativeObject", "J");
  const jlong native_handle = native_object == nullptr
                                  ? 0
                                  : env->GetLongField(surface, native_object);
  auto* producer = reinterpret_cast<void*>(
      static_cast<uintptr_t>(native_handle));
  const jint producer_width =
      darwin_art_android_ANativeWindow_getWidth(producer);
  const jint producer_height =
      darwin_art_android_ANativeWindow_getHeight(producer);
  bool passed = producer != nullptr &&
                darwin_art_android_ANativeWindow_is_managed(producer) &&
                producer_width >= 72 && producer_height >= 48 &&
                darwin_art_android_ANativeWindow_setBuffersGeometry(
                    producer, 0, 0, /*PixelFormat.OPAQUE=*/-1) == 0;

  jclass rect_class = env->FindClass("android/graphics/Rect");
  jmethodID rect_constructor =
      rect_class == nullptr
          ? nullptr
          : env->GetMethodID(rect_class, "<init>", "(IIII)V");
  jobject dirty = rect_constructor == nullptr
                      ? nullptr
                      : env->NewObject(rect_class, rect_constructor, -8, -4,
                                       72, 48);
  jmethodID lock_canvas = env->GetMethodID(
      surface_class, "lockCanvas",
      "(Landroid/graphics/Rect;)Landroid/graphics/Canvas;");
  jmethodID unlock_canvas = env->GetMethodID(
      surface_class, "unlockCanvasAndPost", "(Landroid/graphics/Canvas;)V");
  jmethodID release = env->GetMethodID(surface_class, "release", "()V");
  jobject canvas = !passed || lock_canvas == nullptr || dirty == nullptr
                       ? nullptr
                       : env->CallObjectMethod(surface, lock_canvas, dirty);
  passed = passed && canvas != nullptr && !env->ExceptionCheck();
  if (canvas != nullptr && !env->ExceptionCheck()) {
    jfieldID left = env->GetFieldID(rect_class, "left", "I");
    jfieldID top = env->GetFieldID(rect_class, "top", "I");
    jfieldID right = env->GetFieldID(rect_class, "right", "I");
    jfieldID bottom = env->GetFieldID(rect_class, "bottom", "I");
    jclass canvas_class = env->GetObjectClass(canvas);
    jmethodID get_width = canvas_class == nullptr
                              ? nullptr
                              : env->GetMethodID(canvas_class, "getWidth", "()I");
    jmethodID get_height = canvas_class == nullptr
                               ? nullptr
                               : env->GetMethodID(canvas_class, "getHeight", "()I");
    passed = passed && left != nullptr && top != nullptr && right != nullptr &&
             bottom != nullptr && get_width != nullptr && get_height != nullptr &&
             env->GetIntField(dirty, left) == 0 &&
             env->GetIntField(dirty, top) == 0 &&
             env->GetIntField(dirty, right) == 72 &&
             env->GetIntField(dirty, bottom) == 48 &&
             env->CallIntMethod(canvas, get_width) == producer_width &&
             env->CallIntMethod(canvas, get_height) == producer_height &&
             darwin_art_android_ANativeWindow_getFormat(producer) == 1 &&
             !env->ExceptionCheck();
    env->DeleteLocalRef(canvas_class);
  }
  if (canvas != nullptr && unlock_canvas != nullptr && !env->ExceptionCheck()) {
    env->CallVoidMethod(surface, unlock_canvas, canvas);
    passed = passed && !env->ExceptionCheck();
  }
  if (env->ExceptionCheck()) {
    env->ExceptionDescribe();
    env->ExceptionClear();
    passed = false;
  }
  if (release != nullptr) {
    env->CallVoidMethod(surface, release);
    passed = passed && !env->ExceptionCheck();
  }
  if (!passed && env->ExceptionCheck()) {
    env->ExceptionDescribe();
    env->ExceptionClear();
  }
  env->DeleteLocalRef(canvas);
  env->DeleteLocalRef(dirty);
  env->DeleteLocalRef(rect_class);
  env->DeleteLocalRef(surface);
  env->DeleteLocalRef(surface_class);
  if (passed) {
    std::cerr << "ART Android Surface: Java lockCanvas/unlockCanvasAndPost PASS "
              << "size=" << producer_width << "x" << producer_height
              << " dirty=0,0,72,48 format=RGBA_8888\n";
  }
  return passed;
}

void EnsureViewRootSurfaceValid(JNIEnv* env, jobject view) {
  if (view == nullptr) return;
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_view_root =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getViewRootImpl",
                             "()Landroid/view/ViewRootImpl;");
  jobject view_root = get_view_root == nullptr
                          ? nullptr
                          : env->CallObjectMethod(view, get_view_root);
  jclass view_root_class =
      view_root == nullptr ? nullptr : env->GetObjectClass(view_root);
  jfieldID root_surface_field =
      view_root_class == nullptr
          ? nullptr
          : env->GetFieldID(view_root_class, "mSurface",
                            "Landroid/view/Surface;");
  jobject root_surface = root_surface_field == nullptr
                             ? nullptr
                             : env->GetObjectField(view_root,
                                                   root_surface_field);
  EnsureJavaSurfaceValid(env, root_surface);
  jfieldID root_surface_control_field =
      view_root_class == nullptr
          ? nullptr
          : env->GetFieldID(view_root_class, "mSurfaceControl",
                            "Landroid/view/SurfaceControl;");
  jobject root_surface_control =
      root_surface_control_field == nullptr
          ? nullptr
          : env->GetObjectField(view_root, root_surface_control_field);
  jclass surface_control_class =
      root_surface_control == nullptr
          ? nullptr
          : env->GetObjectClass(root_surface_control);
  jfieldID surface_control_native =
      surface_control_class == nullptr
          ? nullptr
          : env->GetFieldID(surface_control_class, "mNativeObject", "J");
  if (surface_control_native != nullptr &&
      env->GetLongField(root_surface_control, surface_control_native) == 0) {
    // WindowSession.relayout() normally installs SurfaceFlinger's layer here.
    // The Darwin compositor owns the CAMetalLayer directly, but ViewRootImpl
    // must still observe a live layer identity for its transaction lifecycle.
    const jlong native_root = reinterpret_cast<jlong>(
        darwin_art_android_surface_control_create_root("ViewRoot"));
    jmethodID assign_native = env->GetMethodID(
        surface_control_class, "assignNativeObject", "(JLjava/lang/String;)V");
    jstring callsite = env->NewStringUTF("Darwin WMS relayout");
    if (assign_native != nullptr && callsite != nullptr &&
        !env->ExceptionCheck()) {
      env->CallVoidMethod(root_surface_control, assign_native, native_root,
                          callsite);
    } else {
      env->ExceptionClear();
      env->SetLongField(root_surface_control, surface_control_native,
                        native_root);
    }
    if (callsite != nullptr) env->DeleteLocalRef(callsite);
  }
  env->DeleteLocalRef(surface_control_class);
  env->DeleteLocalRef(root_surface_control);
  if (root_surface != nullptr) env->DeleteLocalRef(root_surface);
  if (view_root_class != nullptr) env->DeleteLocalRef(view_root_class);
  if (view_root != nullptr) env->DeleteLocalRef(view_root);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
}

bool CompleteDirectFixtureWindowVisibility(JNIEnv* env, jobject activity,
                                           jobject decor_view) {
  // This path is reachable only from DARWIN_ART_DIRECT_APK_FIXTURE. Installed
  // applications use ActivityThread transactions and never enter this probe.
  jclass activity_class = env->FindClass("android/app/Activity");
  jfieldID decor = activity_class == nullptr
                       ? nullptr
                       : env->GetFieldID(activity_class, "mDecor",
                                         "Lcom/android/internal/policy/DecorView;");
  jfieldID window_added = activity_class == nullptr
                              ? nullptr
                              : env->GetFieldID(activity_class, "mWindowAdded", "Z");
  jfieldID visible_server =
      activity_class == nullptr
          ? nullptr
          : env->GetFieldID(activity_class, "mVisibleFromServer", "Z");
  jfieldID visible_client =
      activity_class == nullptr
          ? nullptr
          : env->GetFieldID(activity_class, "mVisibleFromClient", "Z");
  jmethodID make_visible =
      activity_class == nullptr
          ? nullptr
          : env->GetMethodID(activity_class, "makeVisible", "()V");
  const bool valid = decor != nullptr && window_added != nullptr &&
                     visible_server != nullptr && visible_client != nullptr &&
                     make_visible != nullptr && !env->ExceptionCheck();
  if (valid) {
    env->SetObjectField(activity, decor, decor_view);
    env->SetBooleanField(activity, window_added, JNI_TRUE);
    env->SetBooleanField(activity, visible_server, JNI_TRUE);
    if (env->GetBooleanField(activity, visible_client) == JNI_TRUE) {
      env->CallVoidMethod(activity, make_visible);
    }
  }
  env->DeleteLocalRef(activity_class);
  return valid && !env->ExceptionCheck();
}

bool attach_android_window(JNIEnv* env, jobject activity, jobject window,
                           jobject decor_view, jobject window_attributes,
                           darwin_art_graphics::GraphicsState* graphics_state) {
  if (env == nullptr || activity == nullptr || window == nullptr ||
      decor_view == nullptr || window_attributes == nullptr ||
      graphics_state == nullptr) {
    return false;
  }
  jclass activity_class = env->GetObjectClass(activity);
  jmethodID get_display =
      activity_class == nullptr
          ? nullptr
          : env->GetMethodID(activity_class, "getDisplay",
                             "()Landroid/view/Display;");
  jobject display =
      get_display == nullptr ? nullptr : env->CallObjectMethod(activity, get_display);
  jclass params_class =
      env->FindClass("android/view/WindowManager$LayoutParams");
  jfieldID flags_field = params_class == nullptr
                             ? nullptr
                             : env->GetFieldID(params_class, "flags", "I");
  if (flags_field != nullptr && !env->ExceptionCheck()) {
    // Product windows follow Android's normal hardware-accelerated contract.
    // ViewRootImpl must create ThreadedRenderer and own the retained root
    // RenderNode; the Darwin Surface/SurfaceControl implementation routes its
    // buffers to IOSurface and the Metal composer. Clearing this flag forced a
    // host-owned View.draw() loop and severed Choreographer damage from HWUI.
    constexpr jint kFlagHardwareAccelerated = 0x01000000;
    const jint flags = env->GetIntField(window_attributes, flags_field);
    env->SetIntField(window_attributes, flags_field,
                     flags | kFlagHardwareAccelerated);
  }
  jclass global_class = env->FindClass("android/view/WindowManagerGlobal");
  jmethodID get_instance =
      global_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(global_class, "getInstance",
                                   "()Landroid/view/WindowManagerGlobal;");
  jobject global =
      get_instance == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(global_class, get_instance);
  jmethodID add_view =
      global_class == nullptr
          ? nullptr
          : env->GetMethodID(
                global_class, "addView",
                "(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;"
                "Landroid/view/Display;Landroid/view/Window;I)V");
  if (display == nullptr || global == nullptr || add_view == nullptr ||
      env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(global);
    env->DeleteLocalRef(global_class);
    env->DeleteLocalRef(params_class);
    env->DeleteLocalRef(display);
    env->DeleteLocalRef(activity_class);
    return false;
  }
  env->CallVoidMethod(global, add_view, decor_view, window_attributes, display,
                      window, static_cast<jint>(0));
  if (!CompleteDirectFixtureWindowVisibility(env, activity, decor_view)) {
    env->ExceptionClear();
    return false;
  }
  // ViewRootImpl normally receives a valid BufferQueue producer from
  // WindowManager before its first traversal.  The detached compositor owns
  // that producer, so publish the process-local token at the same boundary.
  // SurfaceView.updateSurface() checks the root Surface before creating child
  // surfaces and otherwise dispatches surfaceDestroyed() to app callbacks.
  EnsureViewRootSurfaceValid(env, decor_view);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android window: WindowManagerGlobal.addView failed\n";
    env->ExceptionDescribe();
    env->ExceptionClear();
    env->DeleteLocalRef(global);
    env->DeleteLocalRef(global_class);
    env->DeleteLocalRef(params_class);
    env->DeleteLocalRef(display);
    env->DeleteLocalRef(activity_class);
    return false;
  }

  jfieldID views_field =
      env->GetFieldID(global_class, "mViews", "Ljava/util/ArrayList;");
  jfieldID roots_field =
      env->GetFieldID(global_class, "mRoots", "Ljava/util/ArrayList;");
  jobject views = views_field == nullptr ? nullptr
                                         : env->GetObjectField(global, views_field);
  jobject roots = roots_field == nullptr ? nullptr
                                         : env->GetObjectField(global, roots_field);
  jclass list_class = env->FindClass("java/util/ArrayList");
  jmethodID size = list_class == nullptr
                       ? nullptr
                       : env->GetMethodID(list_class, "size", "()I");
  jmethodID get = list_class == nullptr
                      ? nullptr
                      : env->GetMethodID(list_class, "get",
                                         "(I)Ljava/lang/Object;");
  jobject view_root = nullptr;
  if (views != nullptr && roots != nullptr && size != nullptr && get != nullptr &&
      !env->ExceptionCheck()) {
    const jint count = env->CallIntMethod(views, size);
    for (jint index = count - 1; index >= 0 && !env->ExceptionCheck(); --index) {
      jobject candidate = env->CallObjectMethod(views, get, index);
      const bool matches = candidate != nullptr &&
                           env->IsSameObject(candidate, decor_view) == JNI_TRUE;
      env->DeleteLocalRef(candidate);
      if (matches) {
        view_root = env->CallObjectMethod(roots, get, index);
        break;
      }
    }
  }
  const bool retained =
      view_root != nullptr && !env->ExceptionCheck() &&
      darwin_art_graphics::retain_interactive_view_root(
          graphics_state, env, view_root);
  env->DeleteLocalRef(view_root);
  env->DeleteLocalRef(list_class);
  env->DeleteLocalRef(roots);
  env->DeleteLocalRef(views);
  env->DeleteLocalRef(global);
  env->DeleteLocalRef(global_class);
  env->DeleteLocalRef(params_class);
  env->DeleteLocalRef(display);
  env->DeleteLocalRef(activity_class);
  return retained && !env->ExceptionCheck();
}

}  // namespace

bool verify_software_surface_canvas(JNIEnv* env) {
  return VerifySoftwareSurfaceCanvas(env);
}

int run(JNIEnv* env, art::Thread* self, jobject activity_instance,
         jclass probe_activity_class, jclass probe_context_class,
         jclass probe_resources_class, jclass probe_view_class,
         jclass probe_canvas_class, jclass content_root_class,
         jobject package_manager, bool run_apk_app,
         bool use_framework_resources, bool expect_apk_widgets,
         bool apk_native_loaded, bool run_framework_button, jint window_scale,
         const char* framework_res_apk, const char* apk_app_package,
         const char* apk_app_activity, const char* app_apk_path,
         darwin_art_graphics::GraphicsState* graphics_state) {
  // Every construction step below has multiple fail-fast returns.  Keep all
  // JNI locals created by the detached Activity/Window transaction in one
  // frame so an exception or an incomplete framework resource cannot leak a
  // local reference into the ART owner thread.
  darwin_art_jni_scope::ScopedLocalFrame local_frame(env);
  if (!local_frame.valid()) {
    std::cerr << "ART Android window: JNI local frame allocation failed\n";
    return 26;
  }
  constexpr jint kApkFrameWidth = 360;
  constexpr jint kApkFrameHeight = 640;
  darwin_art_app_resources::Bundle resources;
  if (darwin_art_app_resources::prepare(
          env, probe_resources_class, use_framework_resources, window_scale,
          framework_res_apk, app_apk_path, &resources) != 0) {
    std::cerr << "ART Android resources: bootstrap construction failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 27;
  }
  darwin_art_app_activity::Bundle activity;
  if (darwin_art_app_activity::prepare(
          env, self, &activity_instance, probe_activity_class,
          probe_context_class, &resources, package_manager, run_apk_app,
          use_framework_resources, apk_app_package, apk_app_activity,
          false, &activity) != 0) {
    return 27;
  }
  if (run_apk_app) {
    // ContextImpl normally carries the application's complete PathClassLoader.
    // Install that exact loader after Activity.attach() and before
    // PhoneWindow/LayoutInflater touches APK XML custom views.
    jclass activity_class = env->GetObjectClass(activity_instance);
    jmethodID get_base_context =
        activity_class == nullptr
            ? nullptr
            : env->GetMethodID(activity_class, "getBaseContext",
                               "()Landroid/content/Context;");
    jobject base_context =
        get_base_context == nullptr
            ? nullptr
            : env->CallObjectMethod(activity_instance, get_base_context);
    jclass base_context_class =
        base_context == nullptr ? nullptr : env->GetObjectClass(base_context);
    jmethodID get_class_loader =
        base_context_class == nullptr
            ? nullptr
            : env->GetMethodID(base_context_class, "getClassLoader",
                               "()Ljava/lang/ClassLoader;");
    jobject app_loader =
        get_class_loader == nullptr
            ? nullptr
            : env->CallObjectMethod(base_context, get_class_loader);
    jmethodID install_loader =
        env->GetStaticMethodID(probe_context_class,
                               "installApplicationClassLoader",
                               "(Ljava/lang/ClassLoader;)V");
    if (install_loader == nullptr || app_loader == nullptr ||
        env->ExceptionCheck()) {
      std::cerr << "ART Android APK: application ClassLoader bootstrap failed"
                << " activity=" << (activity_class != nullptr)
                << " base=" << (base_context != nullptr)
                << " base_class=" << (base_context_class != nullptr)
                << " get_loader=" << (get_class_loader != nullptr)
                << " app_loader=" << (app_loader != nullptr)
                << " install=" << (install_loader != nullptr) << "\n";
      if (env->ExceptionCheck()) {
        env->ExceptionDescribe();
        env->ExceptionClear();
      }
      return 27;
    }
    env->CallStaticVoidMethod(probe_context_class, install_loader, app_loader);
    env->DeleteLocalRef(app_loader);
    env->DeleteLocalRef(base_context_class);
    env->DeleteLocalRef(base_context);
    env->DeleteLocalRef(activity_class);
    if (env->ExceptionCheck()) {
      std::cerr << "ART Android APK: application ClassLoader install failed\n";
      env->ExceptionDescribe();
      env->ExceptionClear();
      return 27;
    }
  }
  jclass window_class = activity.window_class;
  jclass phone_window_class = activity.phone_window_class;
  jobject window = activity.window;
  jobject probe_theme = activity.probe_theme;
  jobject probe_resources = resources.probe_resources;

  // Construct the real framework singleton against the process-local
  // accessibility Binder. Popup ViewRoots register listeners in its internal
  // maps, so an AllocObject shell is not a valid ContextImpl substitute.
  jclass accessibility_class =
      env->FindClass("android/view/accessibility/AccessibilityManager");
  jmethodID get_accessibility_instance =
      accessibility_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                accessibility_class, "getInstance",
                "(Landroid/content/Context;)"
                "Landroid/view/accessibility/AccessibilityManager;");
  jobject accessibility =
      get_accessibility_instance == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(accessibility_class,
                                        get_accessibility_instance,
                                        activity.probe_context);
  if (accessibility == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android window: accessibility stub setup failed\n";
    return 31;
  }

  jmethodID get_window_attributes =
      window_class == nullptr
          ? nullptr
          : env->GetMethodID(window_class, "getAttributes",
                             "()Landroid/view/WindowManager$LayoutParams;");
  jobject window_attributes =
      get_window_attributes == nullptr
          ? nullptr
          : env->CallObjectMethod(window, get_window_attributes);
  jclass decor_view_class =
      env->FindClass("com/android/internal/policy/DecorView");
  jmethodID decor_view_constructor =
      decor_view_class == nullptr
          ? nullptr
          : env->GetMethodID(
                decor_view_class, "<init>",
                "(Landroid/content/Context;I"
                "Lcom/android/internal/policy/PhoneWindow;"
                "Landroid/view/WindowManager$LayoutParams;)V");
  jobject decor_view = nullptr;
  jobject content_root = nullptr;
  jmethodID get_decor_view = nullptr;
  jmethodID find_view_by_id = nullptr;
  jint framework_content_id = 0;
  if (run_apk_app) {
    // ActivityThread does not install an APK's decor before onCreate(). The
    // app's first setContentView() lets PhoneWindow select features and the
    // window background from the final Activity theme. Cache only the method
    // contract here; materialize the decor after performCreate() below.
    get_decor_view =
        window_class == nullptr
            ? nullptr
            : env->GetMethodID(window_class, "getDecorView",
                               "()Landroid/view/View;");
    find_view_by_id =
        window_class == nullptr
            ? nullptr
            : env->GetMethodID(window_class, "findViewById",
                               "(I)Landroid/view/View;");
    jclass framework_id_class = env->FindClass("android/R$id");
    jfieldID content_id =
        framework_id_class == nullptr
            ? nullptr
            : env->GetStaticFieldID(framework_id_class, "content", "I");
    if (content_id != nullptr && !env->ExceptionCheck()) {
      framework_content_id =
          env->GetStaticIntField(framework_id_class, content_id);
    }
    env->DeleteLocalRef(framework_id_class);
  } else {
    decor_view =
        decor_view_constructor == nullptr || window_attributes == nullptr
            ? nullptr
            : env->NewObject(decor_view_class, decor_view_constructor,
                             activity_instance, static_cast<jint>(-1), window,
                             window_attributes);
  }
  // PhoneWindow.installDecor() normally resolves windowBackground from the
  // active Theme and installs it on DecorView. The standalone launcher builds
  // the same objects directly, so preserve that framework-owned resource path
  // explicitly instead of substituting a host color.
  jobject window_background = nullptr;
  if (!run_apk_app && use_framework_resources && decor_view != nullptr) {
    jclass typed_value_class = env->FindClass("android/util/TypedValue");
    jmethodID typed_value_constructor =
        typed_value_class == nullptr
            ? nullptr
            : env->GetMethodID(typed_value_class, "<init>", "()V");
    jobject typed_value =
        typed_value_constructor == nullptr
            ? nullptr
            : env->NewObject(typed_value_class, typed_value_constructor);
    jclass theme_class = env->GetObjectClass(probe_theme);
    jmethodID resolve_attribute =
        theme_class == nullptr
            ? nullptr
            : env->GetMethodID(theme_class, "resolveAttribute",
                               "(ILandroid/util/TypedValue;Z)Z");
    jclass framework_attr_class = env->FindClass("android/R$attr");
    jfieldID window_background_attr =
        framework_attr_class == nullptr
            ? nullptr
            : env->GetStaticFieldID(framework_attr_class, "windowBackground",
                                    "I");
    jfieldID typed_value_resource_id =
        typed_value_class == nullptr
            ? nullptr
            : env->GetFieldID(typed_value_class, "resourceId", "I");
    jmethodID get_drawable =
        probe_resources_class == nullptr
            ? nullptr
            : env->GetMethodID(
                  probe_resources_class, "getDrawable",
                  "(ILandroid/content/res/Resources$Theme;)"
                  "Landroid/graphics/drawable/Drawable;");
    if (typed_value != nullptr && resolve_attribute != nullptr &&
        window_background_attr != nullptr &&
        typed_value_resource_id != nullptr && get_drawable != nullptr) {
      const jint attr = env->GetStaticIntField(framework_attr_class,
                                               window_background_attr);
      const jboolean resolved = env->CallBooleanMethod(
          probe_theme, resolve_attribute, attr, typed_value, JNI_TRUE);
      const jint resource_id =
          resolved == JNI_TRUE && !env->ExceptionCheck()
              ? env->GetIntField(typed_value, typed_value_resource_id)
              : 0;
      if (resource_id != 0) {
        window_background = env->CallObjectMethod(
            probe_resources, get_drawable, resource_id, probe_theme);
      }
    }
    env->DeleteLocalRef(framework_attr_class);
    env->DeleteLocalRef(theme_class);
    env->DeleteLocalRef(typed_value);
    env->DeleteLocalRef(typed_value_class);
  }
  jmethodID content_root_constructor =
      content_root_class == nullptr
          ? nullptr
          : env->GetMethodID(content_root_class, "<init>",
                             "(Landroid/content/Context;)V");
  if (!run_apk_app) {
    content_root =
        content_root_constructor == nullptr
            ? nullptr
            : env->NewObject(content_root_class, content_root_constructor,
                             activity_instance);
  }
  jmethodID add_view =
      decor_view_class == nullptr
          ? nullptr
          : env->GetMethodID(decor_view_class, "addView",
                             "(Landroid/view/View;)V");
  jfieldID phone_decor =
      phone_window_class == nullptr
          ? nullptr
          : env->GetFieldID(phone_window_class, "mDecor",
                            "Lcom/android/internal/policy/DecorView;");
  jfieldID phone_content_parent =
      phone_window_class == nullptr
          ? nullptr
          : env->GetFieldID(phone_window_class, "mContentParent",
                            "Landroid/view/ViewGroup;");
  const bool apk_decor_contract_missing =
      run_apk_app &&
      (get_decor_view == nullptr || find_view_by_id == nullptr ||
       framework_content_id == 0);
  const bool probe_decor_contract_missing =
      !run_apk_app &&
      (decor_view == nullptr || content_root == nullptr || add_view == nullptr ||
       phone_decor == nullptr || phone_content_parent == nullptr ||
       (use_framework_resources && window_background == nullptr));
  if (apk_decor_contract_missing || probe_decor_contract_missing ||
      env->ExceptionCheck()) {
    std::cerr << "ART Android window: DecorView setup failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 31;
  }
  if (!run_apk_app) {
    env->CallVoidMethod(decor_view, add_view, content_root);
    env->SetObjectField(window, phone_decor, decor_view);
    env->SetObjectField(window, phone_content_parent, content_root);
  }
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android window: DecorView attachment threw\n"
              << self->GetException()->Dump() << "\n";
    return 31;
  }

  jmethodID probe_on_create =
      run_apk_app
          ? env->GetMethodID(probe_activity_class, "performCreate",
                             "(Landroid/os/Bundle;)V")
          : env->GetMethodID(probe_activity_class, "probeOnCreate", "()I");
  jmethodID perform_start =
      run_apk_app
          ? env->GetMethodID(probe_activity_class, "performStart",
                             "(Ljava/lang/String;)V")
          : nullptr;
  jmethodID perform_resume =
      run_apk_app
          ? env->GetMethodID(probe_activity_class, "performResume",
                             "(ZLjava/lang/String;)V")
          : nullptr;
  jmethodID is_finishing =
      run_apk_app
          ? env->GetMethodID(probe_activity_class, "isFinishing", "()Z")
          : nullptr;
  jint lifecycle_result = -1;
  bool launcher_finishing = false;
  if (probe_on_create != nullptr) {
    if (run_apk_app) {
      // A device initializes Typeface's system font map during framework
      // startup.  The detached host has no Zygote/SystemServer phase, so run
      // the same pinned AOSP font bootstrap from the support DEX before an
      // arbitrary APK constructs TextView/Button objects.  The APK itself is
      // untouched; this is a runtime-owned framework initialization seam.
      // The support DEX is registered in the same PathClassLoader as the APK,
      // not in the boot class loader.  FindClass from this native frame would
      // therefore miss it for arbitrary APKs; resolve it through the actual
      // Activity class loader instead.
      jclass class_class = env->FindClass("java/lang/Class");
      jmethodID get_class_loader =
          class_class == nullptr
              ? nullptr
              : env->GetMethodID(
                    class_class, "getClassLoader",
                    "()Ljava/lang/ClassLoader;");
      jobject app_loader =
          get_class_loader == nullptr
              ? nullptr
              : env->CallObjectMethod(
                    reinterpret_cast<jobject>(probe_activity_class),
                    get_class_loader);
      jclass loader_class =
          app_loader == nullptr ? nullptr : env->GetObjectClass(app_loader);
      jmethodID load_class =
          loader_class == nullptr
              ? nullptr
              : env->GetMethodID(
                    loader_class, "loadClass",
                    "(Ljava/lang/String;)Ljava/lang/Class;");
      jstring bootstrap_name =
          env->NewStringUTF("dev.darwinart.probe.FontBootstrap");
      jobject bootstrap_class_object =
          load_class == nullptr || bootstrap_name == nullptr
              ? nullptr
              : env->CallObjectMethod(app_loader, load_class, bootstrap_name);
      jclass font_bootstrap =
          reinterpret_cast<jclass>(bootstrap_class_object);
      jmethodID install_fonts =
          font_bootstrap == nullptr
              ? nullptr
              : env->GetStaticMethodID(font_bootstrap, "install", "()V");
      if (install_fonts != nullptr) {
        env->CallStaticVoidMethod(font_bootstrap, install_fonts);
        if (env->ExceptionCheck()) {
          std::cerr << "ART Android framework: system font bootstrap unavailable;"
                    << " continuing with host font fallback\n";
          if (self->GetException() != nullptr) {
            std::cerr << self->GetException()->Dump() << "\n";
          }
          env->ExceptionDescribe();
          env->ExceptionClear();
        }
      }
      env->DeleteLocalRef(bootstrap_name);
      env->DeleteLocalRef(loader_class);
      env->DeleteLocalRef(app_loader);
      env->DeleteLocalRef(class_class);
      env->DeleteLocalRef(font_bootstrap);
      if (env->ExceptionCheck()) {
        std::cerr << "ART Android framework: system font bootstrap failed\n"
                  << self->GetException()->Dump() << "\n";
        return 28;
      }
    }
    if (run_apk_app) {
      env->CallVoidMethod(activity_instance, probe_on_create, nullptr);
      jstring lifecycle_reason = env->NewStringUTF("darwin-art launch");
      const bool finishing =
          !env->ExceptionCheck() && is_finishing != nullptr &&
          env->CallBooleanMethod(activity_instance, is_finishing) == JNI_TRUE;
      launcher_finishing = finishing;
      if (!finishing && !env->ExceptionCheck() && perform_start != nullptr) {
        env->CallVoidMethod(activity_instance, perform_start,
                            lifecycle_reason);
      }
      if (!finishing && !env->ExceptionCheck() && perform_resume != nullptr) {
        env->CallVoidMethod(activity_instance, perform_resume, JNI_FALSE,
                            lifecycle_reason);
      }
      env->DeleteLocalRef(lifecycle_reason);
      lifecycle_result = env->ExceptionCheck() ? -1 : 43;
    } else {
      lifecycle_result =
          env->CallIntMethod(activity_instance, probe_on_create);
    }
  }
  if (env->ExceptionCheck()) {
    DescribeArtThrowableChain(self);
    env->ExceptionDescribe();
    std::cerr << "ART Android lifecycle: Activity.onCreate() threw\n"
              << self->GetException()->Dump() << "\n";
    return 28;
  }
  if (run_apk_app) {
    // Match ActivityThread/PhoneWindow ordering: the Activity has now had the
    // opportunity to apply its theme, request window features, and install
    // content. Query the resulting framework-owned decor rather than freezing
    // the pre-onCreate theme into a host-created root.
    decor_view = env->CallObjectMethod(window, get_decor_view);
    if (!env->ExceptionCheck()) {
      content_root = env->CallObjectMethod(
          window, find_view_by_id, framework_content_id);
    }
    jobject current_window_attributes =
        env->ExceptionCheck() || get_window_attributes == nullptr
            ? nullptr
            : env->CallObjectMethod(window, get_window_attributes);
    if (current_window_attributes != nullptr && !env->ExceptionCheck()) {
      env->DeleteLocalRef(window_attributes);
      window_attributes = current_window_attributes;
    } else {
      env->DeleteLocalRef(current_window_attributes);
    }
    if (decor_view == nullptr || content_root == nullptr ||
        window_attributes == nullptr || env->ExceptionCheck()) {
      std::cerr << "ART Android window: post-onCreate DecorView setup failed\n";
      if (self->IsExceptionPending()) {
        std::cerr << self->GetException()->Dump() << "\n";
      }
      return 31;
    }
  }
  const bool expect_apk_native_answer =
      std::getenv("DARWIN_ART_APK_EXPECT_NATIVE_ANSWER") != nullptr &&
      std::strcmp(std::getenv("DARWIN_ART_APK_EXPECT_NATIVE_ANSWER"), "1") == 0;
  if (run_apk_app && apk_native_loaded && expect_apk_native_answer) {
    jmethodID native_answer = env->GetStaticMethodID(
        probe_activity_class, "nativeAnswer", "()I");
    const jint value = native_answer == nullptr
                           ? -1
                           : env->CallStaticIntMethod(probe_activity_class,
                                                       native_answer);
    if (native_answer == nullptr || env->ExceptionCheck() || value != 42) {
      std::cerr << "ART Android APK JNI: nativeAnswer() expected 42, got "
                << value << "\n";
      if (self->IsExceptionPending()) {
        std::cerr << self->GetException()->Dump() << "\n";
      }
      return 46;
    }
  }
  // PhoneWindow applies the resolved theme background while installing its
  // decor.  The standalone launcher supplies the decor before Activity's
  // setContentView(), so finish the same Android-owned operation after the
  // activity has installed its content.  Going through PhoneWindow keeps the
  // Drawable callback/window-background state in the framework path.
  if (!run_apk_app && use_framework_resources && window_background != nullptr) {
    jmethodID set_window_background =
        phone_window_class == nullptr
            ? nullptr
            : env->GetMethodID(
                  phone_window_class, "setBackgroundDrawable",
                  "(Landroid/graphics/drawable/Drawable;)V");
    if (set_window_background == nullptr || env->ExceptionCheck()) {
      std::cerr << "ART Android window: PhoneWindow background setup failed\n";
      env->ExceptionClear();
      return 31;
    }
    env->CallVoidMethod(window, set_window_background, window_background);
    if (env->ExceptionCheck()) {
      std::cerr << "ART Android window: PhoneWindow background setup threw\n"
                << self->GetException()->Dump() << "\n";
      return 31;
    }
  }
  if (run_apk_app &&
      !attach_android_window(env, activity_instance, window, decor_view,
                             window_attributes, graphics_state)) {
    std::cerr << "ART Android window: real ViewRoot attachment failed\n";
    return 31;
  }
  // A real ViewRootImpl performs the final DecorView layout before input
  // dispatch.  The detached launcher has no ViewRoot, so mirror that one
  // ownership step explicitly; without it DecorView remains 0x0 even though
  // its content display list has already been recorded, making all hit tests
  // fail closed.
  jmethodID layout = decor_view_class == nullptr
                         ? nullptr
                         : env->GetMethodID(decor_view_class, "layout",
                                            "(IIII)V");
  if (layout == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android input: DecorView layout lookup failed\n";
    env->ExceptionClear();
    return 31;
  }
  env->CallVoidMethod(decor_view, layout, 0, 0,
                      kApkFrameWidth * window_scale,
                      kApkFrameHeight * window_scale);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android input: DecorView layout failed\n"
              << self->GetException()->Dump() << "\n";
    return 31;
  }
  // We just installed this exact DecorView in PhoneWindow.mDecor above.  The
  // detached probe Window has no ViewRoot to lazily materialize a decor, so
  // relying on PhoneWindow.getDecorView() here can legitimately return null.
  // Keep the authoritative local object instead.
  jmethodID get_child_at =
      content_root_class == nullptr
          ? nullptr
          : env->GetMethodID(content_root_class, "getChildAt",
                             "(I)Landroid/view/View;");
  jobject probe_view =
      get_child_at == nullptr
          ? nullptr
          : env->CallObjectMethod(content_root, get_child_at,
                                  static_cast<jint>(0));
  if (graphics_state != nullptr &&
      !darwin_art_graphics::retain_hardware_context(graphics_state, env,
                                                    activity_instance)) {
    std::cerr << "ART Android graphics: activity context retention failed\n";
    return 33;
  }
  // This is the direct-APK fixture only. Production activity replacement is
  // scheduled by ActivityTaskManagerEndpoint through ActivityThread.
  if (graphics_state != nullptr && !launcher_finishing &&
      graphics_state->interactive_root == nullptr &&
      darwin_art_graphics_phase::present_and_retain(
          graphics_state, env, decor_view, content_root_class, content_root,
          probe_view_class, probe_view, run_apk_app, expect_apk_widgets,
          run_apk_app || run_framework_button, kApkFrameWidth * window_scale,
          kApkFrameHeight * window_scale) != 0) {
    return 33;
  }
  darwin_art_app_activity::release(env, &activity);
  darwin_art_app_resources::release(env, &resources);
  env->DeleteLocalRef(window_background);
  env->DeleteLocalRef(content_root);
  env->DeleteLocalRef(decor_view);
  env->DeleteLocalRef(decor_view_class);
  env->DeleteLocalRef(window_attributes);
  env->DeleteLocalRef(accessibility);
  env->DeleteLocalRef(accessibility_class);
  env->DeleteLocalRef(probe_view);
  if (lifecycle_result != 43) {
    std::cerr << "ART Android lifecycle: expected 43, got " << lifecycle_result
              << "\n";
    return 29;
  }

  return 0;
}

int run_service(JNIEnv* env, art::Thread* self, jclass service_class,
                jclass probe_context_class, jclass probe_resources_class,
                jobject package_manager, bool use_framework_resources,
                jint window_scale, const char* framework_res_apk,
                const char* apk_app_package, const char* service_class_name,
                const char* app_apk_path, jint control_fd) {
  darwin_art_jni_scope::ScopedLocalFrame local_frame(env);
  if (!local_frame.valid() || env == nullptr || self == nullptr ||
      service_class == nullptr || control_fd < 0) {
    return 26;
  }
  darwin_art_app_resources::Bundle resources;
  if (darwin_art_app_resources::prepare(
          env, probe_resources_class, use_framework_resources, window_scale,
          framework_res_apk, app_apk_path, &resources) != 0) {
    std::cerr << "ART Android service: resource bootstrap failed\n";
    return 27;
  }
  jobject no_activity = nullptr;
  darwin_art_app_activity::Bundle application;
  if (darwin_art_app_activity::prepare(
          env, self, &no_activity, service_class, probe_context_class,
          &resources, package_manager, true, use_framework_resources,
          apk_app_package, service_class_name, true, &application) != 0 ||
      application.probe_context == nullptr || application.application == nullptr) {
    std::cerr << "ART Android service: Application bootstrap failed\n";
    return 27;
  }
  jmethodID service_constructor = env->GetMethodID(service_class, "<init>", "()V");
  jobject service = service_constructor == nullptr
                        ? nullptr
                        : env->NewObject(service_class, service_constructor);
  jclass service_base = env->FindClass("android/app/Service");
  jclass activity_thread_class = env->FindClass("android/app/ActivityThread");
  jfieldID current_thread =
      activity_thread_class == nullptr
          ? nullptr
          : env->GetStaticFieldID(activity_thread_class, "sCurrentActivityThread",
                                  "Landroid/app/ActivityThread;");
  jobject activity_thread = current_thread == nullptr
                                ? nullptr
                                : env->GetStaticObjectField(activity_thread_class,
                                                            current_thread);
  jclass binder_class = env->FindClass("android/os/Binder");
  jmethodID binder_constructor = binder_class == nullptr
                                     ? nullptr
                                     : env->GetMethodID(binder_class, "<init>", "()V");
  jobject token = binder_constructor == nullptr
                      ? nullptr
                      : env->NewObject(binder_class, binder_constructor);
  jmethodID attach =
      service_base == nullptr
          ? nullptr
          : env->GetMethodID(
        service_base, "attach",
        "(Landroid/content/Context;Landroid/app/ActivityThread;"
        "Ljava/lang/String;Landroid/os/IBinder;Landroid/app/Application;"
        "Ljava/lang/Object;)V");
  jstring service_name = env->NewStringUTF(service_class_name);
  if (service == nullptr || activity_thread == nullptr || token == nullptr ||
      attach == nullptr || service_name == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android service: Service.attach lookup failed\n";
    if (env->ExceptionCheck()) env->ExceptionDescribe();
    return 27;
  }
  env->CallVoidMethod(service, attach, application.probe_context,
                      activity_thread, service_name, token,
                      application.application, nullptr);
  jmethodID on_create = env->GetMethodID(service_base, "onCreate", "()V");
  jmethodID on_bind = env->GetMethodID(
      service_base, "onBind", "(Landroid/content/Intent;)Landroid/os/IBinder;");
  jmethodID on_destroy = env->GetMethodID(service_base, "onDestroy", "()V");
  jobject intent = darwin_art::ReceiveServiceBindIntent(env, control_fd);
  if (intent != nullptr && !env->ExceptionCheck()) {
    jclass class_class = env->FindClass("java/lang/Class");
    jmethodID get_class_loader = class_class == nullptr
                                     ? nullptr
                                     : env->GetMethodID(
                                           class_class, "getClassLoader",
                                           "()Ljava/lang/ClassLoader;");
    jobject class_loader = get_class_loader == nullptr
                               ? nullptr
                               : env->CallObjectMethod(service_class,
                                                       get_class_loader);
    jclass intent_class = env->FindClass("android/content/Intent");
    jmethodID set_extras_class_loader =
        intent_class == nullptr
            ? nullptr
            : env->GetMethodID(intent_class, "setExtrasClassLoader",
                               "(Ljava/lang/ClassLoader;)V");
    if (set_extras_class_loader != nullptr && !env->ExceptionCheck()) {
      env->CallVoidMethod(intent, set_extras_class_loader, class_loader);
    }
    env->DeleteLocalRef(intent_class);
    env->DeleteLocalRef(class_loader);
    env->DeleteLocalRef(class_class);
  }
  if (!env->ExceptionCheck() && on_create != nullptr) {
    env->CallVoidMethod(service, on_create);
  }
  jobject binder = on_bind == nullptr || intent == nullptr || env->ExceptionCheck()
                       ? nullptr
                       : env->CallObjectMethod(service, on_bind, intent);
  if (binder == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android service: onCreate/onBind failed\n";
    if (self->IsExceptionPending()) {
      std::cerr << self->GetException()->Dump() << "\n";
    }
    return 27;
  }
  std::cerr << "ART Android service: attached " << service_class_name
            << " pid=" << getpid() << " binder=ready\n";
  if (!darwin_art::StartServingRemoteBinder(env, control_fd, binder)) {
    std::cerr << "ART Android service: Binder dispatcher failed\n";
    return 27;
  }
  jclass looper_class = env->FindClass("android/os/Looper");
  jmethodID my_looper = looper_class == nullptr
                            ? nullptr
                            : env->GetStaticMethodID(
                                  looper_class, "myLooper",
                                  "()Landroid/os/Looper;");
  jmethodID loop = looper_class == nullptr
                       ? nullptr
                       : env->GetStaticMethodID(looper_class, "loop", "()V");
  jobject owner_looper = my_looper == nullptr
                             ? nullptr
                             : env->CallStaticObjectMethod(looper_class,
                                                           my_looper);
  if (owner_looper == nullptr || loop == nullptr || env->ExceptionCheck()) {
    std::cerr << "ART Android service: owner Looper unavailable\n";
    if (env->ExceptionCheck()) env->ExceptionDescribe();
    return 27;
  }
  std::cerr << "ART Android service: entering owner Looper "
            << service_class_name << " pid=" << getpid() << "\n";
  env->CallStaticVoidMethod(looper_class, loop);
  const bool loop_failed = env->ExceptionCheck();
  if (loop_failed) {
    std::cerr << "ART Android service: owner Looper failed\n";
    env->ExceptionDescribe();
  }
  env->DeleteLocalRef(owner_looper);
  env->DeleteLocalRef(looper_class);
  if (on_destroy != nullptr && !env->ExceptionCheck()) {
    env->CallVoidMethod(service, on_destroy);
  }
  return loop_failed ? 27 : 0;
}

}  // namespace darwin_art_presentation
