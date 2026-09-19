#include "texture_view_jni.h"

#include "../darwin_angle_egl.h"
#include "../darwin_android_surface_texture.h"

#include <android/graphics/canvas.h>
#include <android/native_window.h>

#include <algorithm>
#include <cstdint>
#include <iterator>

namespace {

struct RectFields {
  jfieldID left = nullptr;
  jfieldID top = nullptr;
  jfieldID right = nullptr;
  jfieldID bottom = nullptr;
};

struct TextureViewFields {
  jfieldID native_window = nullptr;
};

RectFields g_rect_fields;
TextureViewFields g_texture_view_fields;

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  if (env == nullptr) return false;
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) return false;
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

bool ReadRect(JNIEnv* env, jobject object, ARect* rect) {
  if (object == nullptr || rect == nullptr) return false;
  rect->left = env->GetIntField(object, g_rect_fields.left);
  if (env->ExceptionCheck()) return false;
  rect->top = env->GetIntField(object, g_rect_fields.top);
  if (env->ExceptionCheck()) return false;
  rect->right = env->GetIntField(object, g_rect_fields.right);
  if (env->ExceptionCheck()) return false;
  rect->bottom = env->GetIntField(object, g_rect_fields.bottom);
  return !env->ExceptionCheck();
}

bool WriteRect(JNIEnv* env, jobject object, const ARect& rect) {
  env->SetIntField(object, g_rect_fields.left, rect.left);
  if (env->ExceptionCheck()) return false;
  env->SetIntField(object, g_rect_fields.top, rect.top);
  if (env->ExceptionCheck()) return false;
  env->SetIntField(object, g_rect_fields.right, rect.right);
  if (env->ExceptionCheck()) return false;
  env->SetIntField(object, g_rect_fields.bottom, rect.bottom);
  return !env->ExceptionCheck();
}

void TextureViewCreateNativeWindow(JNIEnv* env, jobject texture_view,
                                   jobject surface_texture) {
  if (env == nullptr || texture_view == nullptr) return;

  // Acquire first so a failed SurfaceTexture lookup leaves an existing valid
  // owner untouched.  This also makes repeated create/destroy cycles balance
  // exactly one producer reference per value stored in mNativeWindow.
  const jlong replacement =
      darwin_art_android_surface_texture_acquire_producer(env,
                                                           surface_texture);
  if (replacement == 0) return;

  const jlong previous = env->GetLongField(
      texture_view, g_texture_view_fields.native_window);
  if (env->ExceptionCheck()) {
    darwin_art_android_ANativeWindow_release_if_managed(
        reinterpret_cast<void*>(static_cast<uintptr_t>(replacement)));
    return;
  }
  env->SetLongField(texture_view, g_texture_view_fields.native_window,
                    replacement);
  if (env->ExceptionCheck()) {
    (void)darwin_art_android_ANativeWindow_release_if_managed(
        reinterpret_cast<void*>(static_cast<uintptr_t>(replacement)));
    return;
  }
  if (previous != 0) {
    (void)darwin_art_android_ANativeWindow_release_if_managed(
        reinterpret_cast<void*>(static_cast<uintptr_t>(previous)));
  }
}

void TextureViewDestroyNativeWindow(JNIEnv* env, jobject texture_view) {
  if (env == nullptr || texture_view == nullptr) return;
  const jlong native_window =
      env->GetLongField(texture_view, g_texture_view_fields.native_window);
  if (env->ExceptionCheck()) return;
  if (native_window != 0) {
    env->SetLongField(texture_view, g_texture_view_fields.native_window, 0);
    if (env->ExceptionCheck()) return;
    (void)darwin_art_android_ANativeWindow_release_if_managed(
        reinterpret_cast<void*>(static_cast<uintptr_t>(native_window)));
  }
}

jboolean TextureViewLockCanvas(JNIEnv* env, jobject, jlong native_window,
                                jobject canvas_object, jobject dirty_object) {
  if (env == nullptr || native_window == 0 || canvas_object == nullptr ||
      !darwin_art_android_ANativeWindow_is_managed(
          reinterpret_cast<void*>(static_cast<uintptr_t>(native_window)))) {
    return JNI_FALSE;
  }

  ARect dirty{0, 0, 0x3FFF, 0x3FFF};
  // Match AOSP's large sentinel for an omitted dirty region.  The first
  // coordinate is explicitly zero; the lock path clips the sentinel to the
  // actual buffer dimensions below.
  dirty.left = 0;
  if (dirty_object != nullptr && !ReadRect(env, dirty_object, &dirty)) {
    return JNI_FALSE;
  }

  auto* window = reinterpret_cast<void*>(static_cast<uintptr_t>(native_window));
  ANativeWindow_Buffer buffer{};
  if (darwin_art_android_ANativeWindow_lock(window, &buffer, &dirty) != 0) {
    return JNI_FALSE;
  }

  ACanvas* canvas = ACanvas_getNativeHandleFromJava(env, canvas_object);
  const int32_t dataspace = ANativeWindow_getBuffersDataSpace(
      reinterpret_cast<ANativeWindow*>(window));
  if (canvas == nullptr || !ACanvas_setBuffer(canvas, &buffer, dataspace)) {
    (void)darwin_art_android_ANativeWindow_cancel_locked_buffer(window);
    return JNI_FALSE;
  }

  dirty.left = std::clamp(dirty.left, 0, buffer.width);
  dirty.top = std::clamp(dirty.top, 0, buffer.height);
  dirty.right = std::clamp(dirty.right, dirty.left, buffer.width);
  dirty.bottom = std::clamp(dirty.bottom, dirty.top, buffer.height);
  ACanvas_clipRect(canvas, &dirty, false);
  if (dirty_object != nullptr && !WriteRect(env, dirty_object, dirty)) {
    (void)ACanvas_setBuffer(canvas, nullptr, 0);
    (void)darwin_art_android_ANativeWindow_cancel_locked_buffer(window);
    return JNI_FALSE;
  }
  return JNI_TRUE;
}

void TextureViewUnlockCanvasAndPost(JNIEnv* env, jobject, jlong native_window,
                                    jobject canvas_object) {
  if (env == nullptr || canvas_object == nullptr) return;
  ACanvas* canvas = ACanvas_getNativeHandleFromJava(env, canvas_object);
  if (canvas == nullptr) return;

  // TextureView does not have Surface.java's mLockedObject.  Its
  // mNativeWindow reference remains the sole owner, so do not acquire another
  // reference here.  Always detach the buffer before posting it.
  (void)ACanvas_setBuffer(canvas, nullptr, 0);
  if (native_window == 0) return;
  auto* window = reinterpret_cast<void*>(static_cast<uintptr_t>(native_window));
  if (darwin_art_android_ANativeWindow_is_managed(window)) {
    (void)darwin_art_android_ANativeWindow_unlockAndPost(window);
  }
}

}  // namespace

namespace darwin_art::window {

bool RegisterTextureViewNatives(JNIEnv* env) {
  if (env == nullptr) return false;

  jclass rect = env->FindClass("android/graphics/Rect");
  if (rect == nullptr) return false;
  g_rect_fields.left = env->GetFieldID(rect, "left", "I");
  g_rect_fields.top = env->GetFieldID(rect, "top", "I");
  g_rect_fields.right = env->GetFieldID(rect, "right", "I");
  g_rect_fields.bottom = env->GetFieldID(rect, "bottom", "I");
  env->DeleteLocalRef(rect);
  if (env->ExceptionCheck() || g_rect_fields.left == nullptr ||
      g_rect_fields.top == nullptr || g_rect_fields.right == nullptr ||
      g_rect_fields.bottom == nullptr) {
    return false;
  }

  jclass texture_view = env->FindClass("android/view/TextureView");
  if (texture_view == nullptr) return false;
  g_texture_view_fields.native_window =
      env->GetFieldID(texture_view, "mNativeWindow", "J");
  env->DeleteLocalRef(texture_view);
  if (env->ExceptionCheck() ||
      g_texture_view_fields.native_window == nullptr) {
    return false;
  }

  JNINativeMethod methods[] = {
      {const_cast<char*>("nCreateNativeWindow"),
       const_cast<char*>("(Landroid/graphics/SurfaceTexture;)V"),
       reinterpret_cast<void*>(&TextureViewCreateNativeWindow)},
      {const_cast<char*>("nDestroyNativeWindow"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&TextureViewDestroyNativeWindow)},
      {const_cast<char*>("nLockCanvas"),
       const_cast<char*>("(JLandroid/graphics/Canvas;Landroid/graphics/Rect;)Z"),
       reinterpret_cast<void*>(&TextureViewLockCanvas)},
      {const_cast<char*>("nUnlockCanvasAndPost"),
       const_cast<char*>("(JLandroid/graphics/Canvas;)V"),
       reinterpret_cast<void*>(&TextureViewUnlockCanvasAndPost)},
  };
  return Register(env, "android/view/TextureView", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::window
