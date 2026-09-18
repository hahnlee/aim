#include "connection_death_jni.h"
#include "../../../compat/binder/proxy_death_recipient.h"

namespace darwin_art::framework::am {
namespace {
jint Presence(JNIEnv* env, jclass, jobject binder, jobject recipient) {
  return static_cast<jint>(android::QueryProxyDeathRecipient(env, binder, recipient));
}
}  // namespace

bool RegisterConnectionDeathResources(JNIEnv* env) {
  if (env->ExceptionCheck()) return false;
  jclass owner = env->FindClass("dev/darwinart/runtime/am/ServiceConnectionDeathRegistration");
  if (owner == nullptr) return false;
  JNINativeMethod method = {
      const_cast<char*>("nativeQueryRecipientPresence"),
      const_cast<char*>("(Landroid/os/IBinder;Landroid/os/IBinder$DeathRecipient;)I"),
      reinterpret_cast<void*>(&Presence)};
  const bool registered = env->RegisterNatives(owner, &method, 1) == JNI_OK;
  env->DeleteLocalRef(owner);
  return registered;
}
}  // namespace darwin_art::framework::am
