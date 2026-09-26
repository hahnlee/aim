#include "security_log_jni.h"

#include <iterator>

// android_app_admin_SecurityLog.cpp over liblog's security buffer. Whether
// security logging is on is liblog's __android_log_security(): the
// persist.logd.security property, which device policy sets. This runtime has
// no logd security buffer, so with logging on, writes and reads fail instead
// of dropping or inventing events.
namespace darwin_art::security {
namespace {
jboolean IsLoggingEnabled(JNIEnv* env, jclass) {
  jclass properties = env->FindClass("android/os/SystemProperties");
  if (properties == nullptr) return JNI_FALSE;
  jmethodID get_boolean =
      env->GetStaticMethodID(properties, "getBoolean", "(Ljava/lang/String;Z)Z");
  jstring key = env->NewStringUTF("persist.logd.security");
  jboolean enabled = get_boolean == nullptr || key == nullptr
                         ? JNI_FALSE
                         : env->CallStaticBooleanMethod(properties, get_boolean, key, JNI_FALSE);
  env->DeleteLocalRef(key);
  env->DeleteLocalRef(properties);
  return enabled;
}

void Throw(JNIEnv* env, const char* type, const char* message) {
  jclass exception = env->FindClass(type);
  if (exception != nullptr) env->ThrowNew(exception, message);
}

jint WriteEvent(JNIEnv* env, jclass, jint, jobjectArray) {
  Throw(env, "java/lang/UnsupportedOperationException",
        "the security log buffer is not provided by this runtime");
  return -1;
}

void ReadEvents(JNIEnv* env, jclass, jobject) {
  Throw(env, "java/io/IOException", "the security log buffer is not provided by this runtime");
}

void ReadEventsSince(JNIEnv* env, jclass, jlong, jobject) {
  Throw(env, "java/io/IOException", "the security log buffer is not provided by this runtime");
}
}  // namespace

bool RegisterSecurityLogNatives(JNIEnv* env) {
  jclass security_log = env->FindClass("android/app/admin/SecurityLog");
  if (security_log == nullptr) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("isLoggingEnabled"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(IsLoggingEnabled)},
      {const_cast<char*>("writeEvent"), const_cast<char*>("(I[Ljava/lang/Object;)I"),
       reinterpret_cast<void*>(WriteEvent)},
      {const_cast<char*>("readEvents"), const_cast<char*>("(Ljava/util/Collection;)V"),
       reinterpret_cast<void*>(ReadEvents)},
      {const_cast<char*>("readPreviousEvents"), const_cast<char*>("(Ljava/util/Collection;)V"),
       reinterpret_cast<void*>(ReadEvents)},
      {const_cast<char*>("readEventsSince"), const_cast<char*>("(JLjava/util/Collection;)V"),
       reinterpret_cast<void*>(ReadEventsSince)},
      {const_cast<char*>("readEventsOnWrapping"),
       const_cast<char*>("(JLjava/util/Collection;)V"),
       reinterpret_cast<void*>(ReadEventsSince)},
  };
  const bool registered =
      env->RegisterNatives(security_log, methods, static_cast<jint>(std::size(methods))) == JNI_OK;
  env->DeleteLocalRef(security_log);
  return registered;
}
}  // namespace darwin_art::security
