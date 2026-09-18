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
extern "C" int darwin_art_runtime_cancel_bound_service_process(
    const char* socket, uint64_t handle);
extern "C" int darwin_art_runtime_abort_prepared_bound_service_process(
    const char* socket, uint64_t handle);

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
  // Allocate the Java result before creating any daemon-owned child.
  jlongArray result = env->NewLongArray(2);
  if (result == nullptr) {
    env->ReleaseStringUTFChars(process_name, process_text);
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
  if (status != 0 || pid == 0 || pid > INT32_MAX || handle == 0) {
    if (handle != 0) darwin_art_runtime_abort_prepared_bound_service_process(socket, handle);
    Error(env, "Profile daemon could not prepare bound-service process");
    return nullptr;
  }
  jlong values[2] = {static_cast<jlong>(pid), static_cast<jlong>(handle)};
  env->SetLongArrayRegion(result, 0, 2, values);
  if (env->ExceptionCheck()) {
    darwin_art_runtime_abort_prepared_bound_service_process(socket, handle);
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

void Discard(JNIEnv* env, jclass, jlong raw_handle) {
  const uint64_t handle = static_cast<uint64_t>(raw_handle);
  if (handle == 0 || darwin_art_runtime_discard_bound_service_process(handle) != 0) {
    Error(env, "Could not release bound-service launch capability");
  }
}

void Cancel(JNIEnv* env, jclass, jlong raw_handle) {
  const char* socket = std::getenv("DARWIN_ART_PROFILE_SOCKET");
  const uint64_t handle = static_cast<uint64_t>(raw_handle);
  if (socket == nullptr || *socket == '\0' || handle == 0 ||
      darwin_art_runtime_cancel_bound_service_process(socket, handle) != 0) {
    Error(env, "Profile daemon could not admit bound-service cancellation");
  }
}

void AbortPrepared(JNIEnv* env, jclass, jlong raw_handle) {
  const char* socket = std::getenv("DARWIN_ART_PROFILE_SOCKET");
  const uint64_t handle = static_cast<uint64_t>(raw_handle);
  if (socket == nullptr || *socket == '\0' || handle == 0) {
    Error(env, "Invalid unpublished bound-service launch capability");
    return;
  }
  const int status = darwin_art_runtime_abort_prepared_bound_service_process(socket, handle);
  // A failed transport (-2) is retained by the Rust unpublished ledger. The
  // caller is relinquishing local publication, not claiming completed reaping.
  if (status != 0 && status != -2) {
    Error(env, "Could not transfer unpublished bound-service cleanup ownership");
  }
}

}  // namespace

bool RegisterProcessLauncher(JNIEnv* env) {
  if (env->ExceptionCheck()) return false;
  jclass launcher = env->FindClass("dev/darwinart/runtime/am/BoundServiceProcessLauncher");
  if (launcher == nullptr) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativePrepareBoundServiceProcess"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;IZJ)[J"),
       reinterpret_cast<void*>(&Prepare)},
      {const_cast<char*>("nativeActivateBoundServiceProcess"),
       const_cast<char*>("(J)V"), reinterpret_cast<void*>(&Activate)},
      {const_cast<char*>("nativeCancelBoundServiceProcess"),
       const_cast<char*>("(J)V"), reinterpret_cast<void*>(&Cancel)},
      {const_cast<char*>("nativeForgetBoundServiceProcess"),
       const_cast<char*>("(J)V"), reinterpret_cast<void*>(&Discard)},
      {const_cast<char*>("nativeAbortPreparedBoundServiceProcess"),
       const_cast<char*>("(J)V"), reinterpret_cast<void*>(&AbortPrepared)},
  };
  const bool registered = env->RegisterNatives(launcher, methods, 5) == JNI_OK;
  env->DeleteLocalRef(launcher);
  return registered;
}

}  // namespace darwin_art::framework::am
