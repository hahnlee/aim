#include "power_state_jni.h"

#include "power_state_platform.h"

#include <iterator>

namespace darwin_art::framework::power {
namespace {

jboolean NativeIsInteractive(JNIEnv*, jclass) {
  return PlatformIsInteractive() ? JNI_TRUE : JNI_FALSE;
}

// {present, level, scale, charging, charged, externalPower}, or null.
jintArray NativeBatteryState(JNIEnv* env, jclass) {
  BatteryState state;
  if (!PlatformBatteryState(&state)) return nullptr;
  const jint values[] = {state.present ? 1 : 0, state.level, state.scale,
                         state.charging ? 1 : 0, state.charged ? 1 : 0,
                         state.external_power ? 1 : 0};
  jintArray result = env->NewIntArray(std::size(values));
  if (result != nullptr) env->SetIntArrayRegion(result, 0, std::size(values), values);
  return result;
}

}  // namespace

bool RegisterBatteryStateProvider(JNIEnv* env, jclass provider_class) {
  if (env == nullptr || provider_class == nullptr || env->ExceptionCheck()) {
    return false;
  }
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeBatteryState"), const_cast<char*>("()[I"),
       reinterpret_cast<void*>(NativeBatteryState)},
  };
  return env->RegisterNatives(provider_class, methods, std::size(methods)) ==
      JNI_OK;
}

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
