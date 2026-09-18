#include <cstdlib>

#include <jni.h>

#include "runtime_graphics_probe.h"

// Optional fixture diagnostic. DisplayEventReceiver may weak-link this symbol,
// but the production graphics-session owner does not depend on View-tree
// reflection or diagnostic state.
extern "C" void darwin_art_graphics_debug_vsync(JNIEnv* env) {
  if (env == nullptr || std::getenv("DARWIN_ART_DEBUG_VIEW_TEXT") == nullptr) {
    return;
  }
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
  jobject views = views_field == nullptr
                      ? nullptr
                      : env->GetObjectField(global, views_field);
  jclass list_class = env->FindClass("java/util/ArrayList");
  jmethodID size = list_class == nullptr
                       ? nullptr
                       : env->GetMethodID(list_class, "size", "()I");
  jmethodID get = list_class == nullptr
                      ? nullptr
                      : env->GetMethodID(list_class, "get",
                                         "(I)Ljava/lang/Object;");
  jobject root = nullptr;
  if (views != nullptr && size != nullptr && get != nullptr &&
      !env->ExceptionCheck()) {
    const jint count = env->CallIntMethod(views, size);
    if (count > 0 && !env->ExceptionCheck()) {
      root = env->CallObjectMethod(views, get, count - 1);
    }
  }
  darwin_art_graphics::debug_view_text_state(env, root);
  env->DeleteLocalRef(root);
  env->DeleteLocalRef(list_class);
  env->DeleteLocalRef(views);
  env->DeleteLocalRef(global);
  env->DeleteLocalRef(global_class);
  if (env->ExceptionCheck()) env->ExceptionClear();
}
