#include "desktop_window_metadata.h"

#include "../../../compat/darwin_surface_bridge.h"

#include <mutex>

namespace darwin_art::framework::wm {
namespace {

std::mutex g_client_lock;
jclass g_client_class = nullptr;
jmethodID g_register_method = nullptr;

jclass LoadClientClass(JNIEnv* env) {
  jclass loader_class = env->FindClass("java/lang/ClassLoader");
  jmethodID get_system_loader =
      loader_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(loader_class, "getSystemClassLoader",
                                   "()Ljava/lang/ClassLoader;");
  jobject loader =
      get_system_loader == nullptr
          ? nullptr
          : env->CallStaticObjectMethod(loader_class, get_system_loader);
  jmethodID load_class =
      loader_class == nullptr
          ? nullptr
          : env->GetMethodID(loader_class, "loadClass",
                             "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring name = env->NewStringUTF(
      "dev.darwinart.runtime.wm.DesktopWindowMetadataClient");
  jclass client =
      loader == nullptr || load_class == nullptr || name == nullptr
          ? nullptr
          : static_cast<jclass>(env->CallObjectMethod(loader, load_class, name));
  if (name != nullptr) env->DeleteLocalRef(name);
  if (loader != nullptr) env->DeleteLocalRef(loader);
  if (loader_class != nullptr) env->DeleteLocalRef(loader_class);
  return client;
}

jboolean SetTitle(JNIEnv* env, jclass, jstring title) {
  if (env == nullptr || title == nullptr || env->ExceptionCheck()) return JNI_FALSE;
  const jsize length = env->GetStringLength(title);
  const jchar* characters = env->GetStringChars(title, nullptr);
  if (characters == nullptr || env->ExceptionCheck()) return JNI_FALSE;
  const auto status = darwin_art_surface_set_active_title_utf16(
      reinterpret_cast<const uint16_t*>(characters), static_cast<size_t>(length));
  env->ReleaseStringChars(title, characters);
  return status == DARWIN_ART_SURFACE_OK ? JNI_TRUE : JNI_FALSE;
}

}  // namespace

bool RegisterDesktopWindowMetadataClient(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  std::lock_guard<std::mutex> guard(g_client_lock);
  if (g_client_class != nullptr && g_register_method != nullptr) return true;
  jclass client = LoadClientClass(env);
  if (client == nullptr) return false;
  JNINativeMethod method = {
      const_cast<char*>("nativeSetTitle"), const_cast<char*>("(Ljava/lang/String;)Z"),
      reinterpret_cast<void*>(&SetTitle)};
  const bool natives_registered =
      env->RegisterNatives(client, &method, 1) == JNI_OK &&
      !env->ExceptionCheck();
  jmethodID registration =
      natives_registered
          ? env->GetStaticMethodID(client, "register", "()V")
          : nullptr;
  jclass global = registration == nullptr || env->ExceptionCheck()
      ? nullptr
      : static_cast<jclass>(env->NewGlobalRef(client));
  env->DeleteLocalRef(client);
  if (global == nullptr || env->ExceptionCheck()) return false;
  g_client_class = global;
  g_register_method = registration;
  return true;
}

bool RegisterDesktopWindowMetadataReceiver(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass client = nullptr;
  jmethodID registration = nullptr;
  {
    std::lock_guard<std::mutex> guard(g_client_lock);
    if (g_client_class != nullptr) {
      client = static_cast<jclass>(env->NewLocalRef(g_client_class));
      registration = g_register_method;
    }
  }
  if (client == nullptr || registration == nullptr || env->ExceptionCheck())
    return false;
  env->CallStaticVoidMethod(client, registration);
  env->DeleteLocalRef(client);
  return !env->ExceptionCheck();
}

bool EnsureDesktopWindowMetadataReceiver(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass activity_thread = env->FindClass("android/app/ActivityThread");
  jmethodID current = activity_thread == nullptr
      ? nullptr
      : env->GetStaticMethodID(activity_thread, "currentActivityThread",
                               "()Landroid/app/ActivityThread;");
  jobject thread = current == nullptr
      ? nullptr
      : env->CallStaticObjectMethod(activity_thread, current);
  env->DeleteLocalRef(activity_thread);
  if (env->ExceptionCheck()) return false;
  if (thread == nullptr) return true;
  env->DeleteLocalRef(thread);
  return RegisterDesktopWindowMetadataReceiver(env);
}

}  // namespace darwin_art::framework::wm
