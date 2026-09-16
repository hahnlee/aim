#include "graphics_environment.h"

#include <atomic>
#include <cstdint>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

namespace darwin_art::graphics {
namespace {

struct State {
  std::string driver_path;
  std::string sphal_libraries;
  std::string angle_path;
  std::string angle_package;
  std::vector<std::string> angle_features;
  std::string layer_paths;
  std::string debug_layers;
  std::string debug_layers_gles;
  bool use_native_driver = true;
  bool angle_system_driver = false;
  uint64_t activity_launch_hints = 0;
};

std::atomic<bool> g_app_debuggable{false};
std::mutex g_mutex;
State g_state;

std::string CopyString(JNIEnv* env, jstring value) {
  if (value == nullptr) return {};
  const char* text = env->GetStringUTFChars(value, nullptr);
  if (text == nullptr) return {};
  std::string result(text);
  env->ReleaseStringUTFChars(value, text);
  return result;
}

jboolean IsDebuggable(JNIEnv*, jclass) {
  return g_app_debuggable.load(std::memory_order_acquire) ? JNI_TRUE : JNI_FALSE;
}

void SetDriverPath(JNIEnv* env, jclass, jstring path, jstring sphal) {
  const std::string copied_path = CopyString(env, path);
  const std::string copied_sphal = CopyString(env, sphal);
  if (env->ExceptionCheck()) return;
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state.driver_path = copied_path;
  g_state.sphal_libraries = copied_sphal;
}

void SetGpuStats(JNIEnv*, jclass, jstring, jstring, jlong, jlong, jstring, jint) {
  // Android forwards this accounting payload to IGpuService. The Darwin
  // compositor does not yet expose that service; setup remains successful.
}

jboolean SetInjectLayersPrSetDumpable(JNIEnv*, jclass) {
  // Linux makes an opted-in app dumpable before injecting layers. Darwin's
  // process specialization already owns this policy.
  return g_app_debuggable.load(std::memory_order_acquire) ? JNI_TRUE : JNI_FALSE;
}

void SetAngleInfo(JNIEnv* env, jclass, jstring path, jboolean use_native_driver,
                  jstring package, jobjectArray features) {
  State update;
  update.angle_path = CopyString(env, path);
  update.angle_package = CopyString(env, package);
  update.use_native_driver = use_native_driver == JNI_TRUE;
  if (features != nullptr && !env->ExceptionCheck()) {
    const jsize count = env->GetArrayLength(features);
    for (jsize index = 0; index < count && !env->ExceptionCheck(); ++index) {
      jstring feature =
          static_cast<jstring>(env->GetObjectArrayElement(features, index));
      if (feature != nullptr) update.angle_features.push_back(CopyString(env, feature));
      env->DeleteLocalRef(feature);
    }
  }
  if (env->ExceptionCheck()) return;
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state.angle_path = std::move(update.angle_path);
  g_state.angle_package = std::move(update.angle_package);
  g_state.angle_features = std::move(update.angle_features);
  g_state.use_native_driver = update.use_native_driver;
}

void SetLayerPaths(JNIEnv* env, jclass, jobject, jstring paths) {
  const std::string copied = CopyString(env, paths);
  if (env->ExceptionCheck()) return;
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state.layer_paths = copied;
}

void SetDebugLayers(JNIEnv* env, jclass, jstring layers) {
  const std::string copied = CopyString(env, layers);
  if (env->ExceptionCheck()) return;
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state.debug_layers = copied;
}

void SetDebugLayersGles(JNIEnv* env, jclass, jstring layers) {
  const std::string copied = CopyString(env, layers);
  if (env->ExceptionCheck()) return;
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state.debug_layers_gles = copied;
}

void HintActivityLaunch(JNIEnv*, jclass) {
  std::lock_guard<std::mutex> lock(g_mutex);
  ++g_state.activity_launch_hints;
}

void ToggleAngleSystemDriver(JNIEnv*, jclass, jboolean enabled) {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state.angle_system_driver = enabled == JNI_TRUE;
}

}

void ConfigureGraphicsEnvironment(bool app_debuggable) {
  g_app_debuggable.store(app_debuggable, std::memory_order_release);
}

void ResetGraphicsEnvironment() {
  g_app_debuggable.store(false, std::memory_order_release);
  std::lock_guard<std::mutex> lock(g_mutex);
  g_state = {};
}

int RegisterGraphicsEnvironment(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return JNI_ERR;
  jclass type = env->FindClass("android/os/GraphicsEnvironment");
  if (type == nullptr) return JNI_ERR;
  JNINativeMethod methods[] = {
      {const_cast<char*>("isDebuggable"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&IsDebuggable)},
      {const_cast<char*>("setDriverPathAndSphalLibraries"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&SetDriverPath)},
      {const_cast<char*>("setGpuStats"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;JJLjava/lang/String;I)V"),
       reinterpret_cast<void*>(&SetGpuStats)},
      {const_cast<char*>("setInjectLayersPrSetDumpable"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&SetInjectLayersPrSetDumpable)},
      {const_cast<char*>("nativeSetAngleInfo"),
       const_cast<char*>("(Ljava/lang/String;ZLjava/lang/String;[Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&SetAngleInfo)},
      {const_cast<char*>("setLayerPaths"),
       const_cast<char*>("(Ljava/lang/ClassLoader;Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&SetLayerPaths)},
      {const_cast<char*>("setDebugLayers"), const_cast<char*>("(Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&SetDebugLayers)},
      {const_cast<char*>("setDebugLayersGLES"),
       const_cast<char*>("(Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&SetDebugLayersGles)},
      {const_cast<char*>("hintActivityLaunch"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&HintActivityLaunch)},
      {const_cast<char*>("nativeToggleAngleAsSystemDriver"), const_cast<char*>("(Z)V"),
       reinterpret_cast<void*>(&ToggleAngleSystemDriver)},
  };
  const jint status = env->RegisterNatives(
      type, methods, static_cast<jint>(sizeof(methods) / sizeof(methods[0])));
  env->DeleteLocalRef(type);
  return status;
}

}
