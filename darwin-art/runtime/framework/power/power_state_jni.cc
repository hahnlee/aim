#include "power_state_jni.h"

#include "power_state_platform.h"

#include <iterator>

namespace darwin_art::framework::power {
namespace {

jboolean NativeIsInteractive(JNIEnv*, jclass) {
  return PlatformIsInteractive() ? JNI_TRUE : JNI_FALSE;
}

}  // namespace

bool RegisterPowerStateProvider(JNIEnv* env, jclass provider_class) {
  if (env == nullptr || provider_class == nullptr || env->ExceptionCheck()) {
    return false;
  }
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeIsInteractive"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(NativeIsInteractive)},
  };
  return env->RegisterNatives(provider_class, methods, std::size(methods)) ==
      JNI_OK;
}

}  // namespace darwin_art::framework::power
