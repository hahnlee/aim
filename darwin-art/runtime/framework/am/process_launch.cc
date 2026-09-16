#include "process_launch.h"

#include <cstdint>
#include <cstdlib>

extern "C" int darwin_art_runtime_prepare_bound_service_process(
    const char* socket, const char* package, const char* process_name,
    uint32_t uid, int32_t isolated, uint64_t start_sequence,
    uint32_t* output_pid, uint64_t* output_handle);
extern "C" int darwin_art_runtime_activate_bound_service_process(
    const char* socket, uint64_t handle);
extern "C" int darwin_art_runtime_discard_bound_service_process(uint64_t handle);

namespace darwin_art::framework::am {
namespace {

void Error(JNIEnv* env, const char* message) {
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/IllegalStateException");
  if (type != nullptr) env->ThrowNew(type, message);
  env->DeleteLocalRef(type);
}

jlongArray Prepare(JNIEnv* env, jclass, jstring package, jstring process_name,
                   jint uid, jboolean isolated, jlong start_sequence) {
  const char* socket = std::getenv("DARWIN_ART_PROFILE_SOCKET");
  if (socket == nullptr || *socket == '\0' || package == nullptr ||
      process_name == nullptr || uid < 0 || start_sequence < 0) {
    Error(env, "Invalid bound-service process request");
    return nullptr;
  }
  const char* package_text = env->GetStringUTFChars(package, nullptr);
  if (package_text == nullptr) return nullptr;
  const char* process_text = env->GetStringUTFChars(process_name, nullptr);
  if (process_text == nullptr) {
    env->ReleaseStringUTFChars(package, package_text);
    return nullptr;
  }
  uint32_t pid = 0;
  uint64_t handle = 0;
  const int status = darwin_art_runtime_prepare_bound_service_process(
      socket, package_text, process_text, static_cast<uint32_t>(uid),
      isolated == JNI_TRUE ? 1 : 0, static_cast<uint64_t>(start_sequence),
      &pid, &handle);
  env->ReleaseStringUTFChars(process_name, process_text);
  env->ReleaseStringUTFChars(package, package_text);
  if (status != 0 || pid == 0 || handle == 0) {
    Error(env, "Profile daemon could not prepare bound-service process");
    return nullptr;
  }
  jlong values[2] = {static_cast<jlong>(pid), static_cast<jlong>(handle)};
  jlongArray result = env->NewLongArray(2);
  if (result == nullptr) {
    darwin_art_runtime_discard_bound_service_process(handle);
    return nullptr;
  }
  env->SetLongArrayRegion(result, 0, 2, values);
  if (env->ExceptionCheck()) {
    darwin_art_runtime_discard_bound_service_process(handle);
    return nullptr;
  }
  return result;
}

void Activate(JNIEnv* env, jclass, jlong raw_handle) {
  const char* socket = std::getenv("DARWIN_ART_PROFILE_SOCKET");
  const uint64_t handle = static_cast<uint64_t>(raw_handle);
  if (socket == nullptr || *socket == '\0' || handle == 0 ||
      darwin_art_runtime_activate_bound_service_process(socket, handle) != 0) {
    Error(env, "Profile daemon could not activate bound-service process");
  }
}

void Discard(JNIEnv*, jclass, jlong raw_handle) {
  const uint64_t handle = static_cast<uint64_t>(raw_handle);
  if (handle != 0) darwin_art_runtime_discard_bound_service_process(handle);
}

}  // namespace

bool RegisterProcessLauncher(JNIEnv* env, jclass endpoint) {
  if (endpoint == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativePrepareBoundServiceProcess"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;IZJ)[J"),
       reinterpret_cast<void*>(&Prepare)},
      {const_cast<char*>("nativeActivateBoundServiceProcess"),
       const_cast<char*>("(J)V"), reinterpret_cast<void*>(&Activate)},
      {const_cast<char*>("nativeDiscardBoundServiceProcess"),
       const_cast<char*>("(J)V"), reinterpret_cast<void*>(&Discard)},
  };
  return env->RegisterNatives(endpoint, methods, 3) == JNI_OK;
}

}  // namespace darwin_art::framework::am
