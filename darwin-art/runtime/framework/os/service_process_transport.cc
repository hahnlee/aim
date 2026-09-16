#include "service_process_transport.h"

#include "../../../compat/darwin_binder_wire.h"
#include "../../../probes/runtime_process_state.h"

#include <chrono>
#include <cstdlib>
#include <iostream>
#include <iterator>
#include <string>
#include <unistd.h>

namespace darwin_art::framework::os {
namespace {

std::string JavaString(JNIEnv* env, jstring value) {
  if (env == nullptr || value == nullptr) return {};
  const char* utf = env->GetStringUTFChars(value, nullptr);
  if (utf == nullptr) return {};
  std::string result(utf);
  env->ReleaseStringUTFChars(value, utf);
  return result;
}

class ScopedSpawn final {
 public:
  ScopedSpawn(JNIEnv* env, int32_t host_pid, int32_t control_fd)
      : env_(env), host_pid_(host_pid), control_fd_(control_fd) {}
  ScopedSpawn(const ScopedSpawn&) = delete;
  ScopedSpawn& operator=(const ScopedSpawn&) = delete;

  ~ScopedSpawn() { Rollback(); }

  void Commit() {
    committed_ = true;
    host_pid_ = -1;
    control_fd_ = -1;
    process_owned_ = false;
  }

 private:
  void Rollback() noexcept {
    if (committed_) return;
    // SendServiceBindIntent can populate wire state before dispatcher startup;
    // channel close is idempotent and must precede closing the descriptor.
    if (env_ != nullptr && control_fd_ >= 0) {
      darwin_art::CloseRemoteBinderChannel(env_, control_fd_);
    }
    if (control_fd_ >= 0) {
      close(control_fd_);
      control_fd_ = -1;
    }
    // A successful spawn owns one process lease; release a valid pid exactly
    // once. The spawn ABI uses -1 as its invalid/unset pid sentinel.
    if (process_owned_) {
      process_owned_ = false;
      if (host_pid_ >= 0) {
        darwin_art_process::release_service_process(host_pid_);
      }
    }
  }

  JNIEnv* env_;
  int32_t host_pid_;
  int32_t control_fd_;
  bool process_owned_ = true;
  bool committed_ = false;
};

jintArray SpawnService(JNIEnv* env, jclass, jstring component,
                       jstring instance_name, jstring process_name,
                       jboolean isolated, jobject intent) {
  const bool debug_timing =
      std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr;
  const auto started = std::chrono::steady_clock::now();
  const auto log_stage = [&](const char* stage) {
    if (!debug_timing) return;
    const auto elapsed = std::chrono::duration_cast<std::chrono::microseconds>(
                             std::chrono::steady_clock::now() - started)
                             .count();
    std::cerr << "DARWIN_ART service-spawn stage=" << stage
              << " elapsed_us=" << elapsed << "\n";
  };
  if (env == nullptr || env->ExceptionCheck()) return nullptr;
  const std::string component_utf = JavaString(env, component);
  if (component_utf.empty() || env->ExceptionCheck()) return nullptr;
  const std::string instance_utf = JavaString(env, instance_name);
  if (env->ExceptionCheck()) return nullptr;
  const std::string process_utf = JavaString(env, process_name);
  if (process_utf.empty() || env->ExceptionCheck()) {
    return nullptr;
  }

  int32_t host_pid = -1;
  int32_t control_fd = -1;
  const int32_t spawn_status = darwin_art_process::spawn_service_process(
      component_utf.c_str(), instance_utf.c_str(), process_utf.c_str(),
      isolated == JNI_TRUE, &host_pid, &control_fd);
  log_stage("spawn");
  if (spawn_status != 0) return nullptr;
  ScopedSpawn spawned(env, host_pid, control_fd);

  const bool intent_sent =
      intent != nullptr && darwin_art::SendServiceBindIntent(env, control_fd, intent);
  log_stage("bind-intent");
  const bool dispatcher_started =
      intent_sent && darwin_art::StartRemoteBinderDispatcher(env, control_fd);
  log_stage("dispatcher");
  if (!dispatcher_started) return nullptr;

  const jint values[2] = {host_pid, control_fd};
  jintArray result = env->NewIntArray(2);
  if (result == nullptr || env->ExceptionCheck()) return nullptr;
  env->SetIntArrayRegion(result, 0, 2, values);
  if (env->ExceptionCheck()) return nullptr;

  // The Java result now owns the process/fd pair. Every JNI failure above
  // leaves ScopedSpawn responsible for both resources.
  spawned.Commit();
  return result;
}

jint ReleaseRemoteService(JNIEnv* env, jclass, jint host_pid, jint control_fd) {
  if (env == nullptr) return -1;
  darwin_art::CloseRemoteBinderChannel(env, control_fd);
  const int close_status = control_fd < 0 ? -1 : close(control_fd);
  const int32_t release_status =
      darwin_art_process::release_service_process(host_pid);
  return close_status == 0 && release_status == 0 ? 0 : -1;
}

}  // namespace

bool RegisterSystemServiceClientTransport(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass remote = env->FindClass("dev/darwinart/runtime/os/RemoteBinder");
  if (remote == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(remote);
    return false;
  }
  const bool transport = darwin_art::RegisterRemoteBinderNatives(env, remote);
  env->DeleteLocalRef(remote);
  if (!transport || env->ExceptionCheck()) return false;

  jclass services = env->FindClass("dev/darwinart/runtime/os/SystemServices");
  if (services == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(services);
    return false;
  }
  const bool system_transport =
      darwin_art::RegisterSystemServicesNatives(env, services);
  env->DeleteLocalRef(services);
  return system_transport && !env->ExceptionCheck();
}

bool RegisterServiceProcessTransport(JNIEnv* env, jclass endpoint) {
  if (env == nullptr || endpoint == nullptr || env->ExceptionCheck() ||
      !RegisterSystemServiceClientTransport(env)) {
    return false;
  }
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeSpawnService"),
       const_cast<char*>(
           "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;"
           "ZLandroid/content/Intent;)[I"),
       reinterpret_cast<void*>(&SpawnService)},
      {const_cast<char*>("nativeReleaseRemoteService"),
       const_cast<char*>("(II)I"),
       reinterpret_cast<void*>(&ReleaseRemoteService)},
  };

  return env->RegisterNatives(endpoint, methods,
                              static_cast<jint>(std::size(methods))) == JNI_OK;
}

}  // namespace darwin_art::framework::os
