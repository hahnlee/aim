#include "../darwin_binder_wire.h"
#include "context_manager.h"

namespace darwin_art {
namespace {
jobject Connect(JNIEnv* env, jclass) {
  return GetSystemContextObject(env);
}
jboolean Transact(JNIEnv* env, jclass, jint fd, jint target, jint code,
                  jobject data, jobject reply, jint flags) {
  return TransactRemoteBinder(env, fd, target, code, data, reply, flags);
}
}

bool RegisterRemoteBinderNatives(JNIEnv* env, jclass endpoint) {
  if (env == nullptr || endpoint == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {{
      const_cast<char*>("nativeTransact"),
      const_cast<char*>("(IIILandroid/os/Parcel;Landroid/os/Parcel;I)Z"),
      reinterpret_cast<void*>(&Transact)}};
  return env->RegisterNatives(endpoint, methods, 1) == JNI_OK;
}
bool RegisterSystemServicesNatives(JNIEnv* env, jclass endpoint) {
  if (env == nullptr || endpoint == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {{const_cast<char*>("nativeConnect"),
      const_cast<char*>("()Landroid/os/IBinder;"), reinterpret_cast<void*>(&Connect)}};
  return env->RegisterNatives(endpoint, methods, 1) == JNI_OK;
}
}  // namespace darwin_art
