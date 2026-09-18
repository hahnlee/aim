#include "desktop_foreground_authority_jni.h"
#include "../../../compat/window/desktop_foreground_provider.h"

namespace darwin_art::framework::wm {
namespace {
jlongArray Capture(JNIEnv* env, jclass, jint pid) {
  if (env == nullptr || env->ExceptionCheck()) return nullptr;
  window::ProcessIdentity identity{};
  if (!window::CaptureProcessIdentity(pid, &identity)) return nullptr;
  jlongArray result = env->NewLongArray(2);
  if (result == nullptr || env->ExceptionCheck()) return nullptr;
  const jlong birth[] = {static_cast<jlong>(identity.start_seconds),
                        static_cast<jlong>(identity.start_microseconds)};
  env->SetLongArrayRegion(result, 0, 2, birth);
  return result;
}

jboolean Foreground(JNIEnv* env, jclass, jint pid, jlong seconds, jlong microseconds) {
  if (env == nullptr || env->ExceptionCheck() || pid <= 0 || seconds <= 0 ||
      microseconds < 0 || microseconds >= 1000000) return JNI_FALSE;
  const window::ProcessIdentity identity{pid, static_cast<uint64_t>(seconds),
                                       static_cast<uint64_t>(microseconds)};
  return window::IsProcessForeground(identity) ? JNI_TRUE : JNI_FALSE;
}
}

bool RegisterDesktopForegroundAuthority(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass loader_class = env->FindClass("java/lang/ClassLoader");
  if (loader_class == nullptr || env->ExceptionCheck()) return false;
  const auto get_loader = env->GetStaticMethodID(loader_class, "getSystemClassLoader",
                                                "()Ljava/lang/ClassLoader;");
  const auto load = get_loader == nullptr || env->ExceptionCheck() ? nullptr
      : env->GetMethodID(loader_class, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;");
  jobject loader = get_loader == nullptr || load == nullptr || env->ExceptionCheck()
      ? nullptr : env->CallStaticObjectMethod(loader_class, get_loader);
  jstring name = loader == nullptr || env->ExceptionCheck() ? nullptr
      : env->NewStringUTF("dev.darwinart.runtime.wm.DesktopForegroundAuthority");
  jclass type = name == nullptr || env->ExceptionCheck() ? nullptr
      : static_cast<jclass>(env->CallObjectMethod(loader, load, name));
  const JNINativeMethod methods[] = {
      {const_cast<char*>("nativeCaptureProcess"), const_cast<char*>("(I)[J"),
       reinterpret_cast<void*>(Capture)},
      {const_cast<char*>("nativeIsForeground"), const_cast<char*>("(IJJ)Z"),
       reinterpret_cast<void*>(Foreground)},
  };
  const bool registered = type != nullptr && !env->ExceptionCheck() &&
      env->RegisterNatives(type, methods, 2) == JNI_OK && !env->ExceptionCheck();
  if (type != nullptr) env->DeleteLocalRef(type);
  if (name != nullptr) env->DeleteLocalRef(name);
  if (loader != nullptr) env->DeleteLocalRef(loader);
  env->DeleteLocalRef(loader_class);
  return registered;
}
}
