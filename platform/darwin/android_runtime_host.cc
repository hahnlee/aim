#include "aim/android_runtime_host.h"

#include <android_runtime/AndroidRuntime.h>

#include <mutex>

namespace {

enum class ProcessVmState : unsigned char {
  kFresh,
  kInstalled,
  kUninstalled,
};

std::mutex g_process_vm_mutex;
JavaVM* g_process_java_vm = nullptr;
ProcessVmState g_process_vm_state = ProcessVmState::kFresh;

int GetAndValidateJavaVm(JNIEnv* env, JavaVM** vm_out) {
  if (env == nullptr || vm_out == nullptr) {
    return AIM_ANDROID_RUNTIME_INVALID_ENV;
  }

  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK || vm == nullptr) {
    return AIM_ANDROID_RUNTIME_GET_VM_FAILED;
  }

  void* current_env = nullptr;
  if (vm->GetEnv(&current_env, JNI_VERSION_1_6) != JNI_OK) {
    return AIM_ANDROID_RUNTIME_CURRENT_THREAD_NOT_ATTACHED;
  }
  if (current_env != env) {
    return AIM_ANDROID_RUNTIME_ENV_MISMATCH;
  }

  *vm_out = vm;
  return AIM_ANDROID_RUNTIME_OK;
}

}  // namespace

extern "C" int aim_android_runtime_install(JNIEnv* env) {
  JavaVM* vm = nullptr;
  const int validation = GetAndValidateJavaVm(env, &vm);
  if (validation != AIM_ANDROID_RUNTIME_OK) {
    return validation;
  }

  const std::lock_guard lock(g_process_vm_mutex);
  if (g_process_vm_state != ProcessVmState::kFresh) {
    if (g_process_vm_state == ProcessVmState::kUninstalled) {
      return AIM_ANDROID_RUNTIME_ALREADY_UNINSTALLED;
    }
    return g_process_java_vm == vm
               ? AIM_ANDROID_RUNTIME_ALREADY_INSTALLED
               : AIM_ANDROID_RUNTIME_DIFFERENT_VM;
  }

  g_process_java_vm = vm;
  g_process_vm_state = ProcessVmState::kInstalled;
  return AIM_ANDROID_RUNTIME_OK;
}

extern "C" int aim_android_runtime_uninstall(JNIEnv* env) {
  JavaVM* vm = nullptr;
  const int validation = GetAndValidateJavaVm(env, &vm);
  if (validation != AIM_ANDROID_RUNTIME_OK) {
    return validation;
  }

  const std::lock_guard lock(g_process_vm_mutex);
  if (g_process_vm_state == ProcessVmState::kFresh) {
    return AIM_ANDROID_RUNTIME_NOT_INSTALLED;
  }
  if (g_process_vm_state == ProcessVmState::kUninstalled) {
    return AIM_ANDROID_RUNTIME_ALREADY_UNINSTALLED;
  }
  if (g_process_java_vm != vm) {
    return AIM_ANDROID_RUNTIME_DIFFERENT_VM;
  }

  g_process_vm_state = ProcessVmState::kUninstalled;
  g_process_java_vm = nullptr;
  return AIM_ANDROID_RUNTIME_OK;
}

namespace android {

JavaVM* AndroidRuntime::getJavaVM() {
  const std::lock_guard lock(g_process_vm_mutex);
  if (g_process_vm_state != ProcessVmState::kInstalled) {
    return nullptr;
  }
  return g_process_java_vm;
}

JNIEnv* AndroidRuntime::getJNIEnv() {
  JavaVM* vm = getJavaVM();
  if (vm == nullptr) {
    return nullptr;
  }

  void* env = nullptr;
  if (vm->GetEnv(&env, JNI_VERSION_1_6) != JNI_OK) {
    return nullptr;
  }
  return static_cast<JNIEnv*>(env);
}

}  // namespace android
