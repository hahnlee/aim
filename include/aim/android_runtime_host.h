#ifndef AIM_ANDROID_RUNTIME_HOST_H_
#define AIM_ANDROID_RUNTIME_HOST_H_

#include <jni.h>

#if defined(__cplusplus)
extern "C" {
#endif

enum aim_android_runtime_status {
  AIM_ANDROID_RUNTIME_OK = 0,
  AIM_ANDROID_RUNTIME_INVALID_ENV = 1,
  AIM_ANDROID_RUNTIME_GET_VM_FAILED = 2,
  AIM_ANDROID_RUNTIME_CURRENT_THREAD_NOT_ATTACHED = 3,
  AIM_ANDROID_RUNTIME_ENV_MISMATCH = 4,
  AIM_ANDROID_RUNTIME_ALREADY_INSTALLED = 5,
  AIM_ANDROID_RUNTIME_DIFFERENT_VM = 6,
  AIM_ANDROID_RUNTIME_NOT_INSTALLED = 7,
  AIM_ANDROID_RUNTIME_ALREADY_UNINSTALLED = 8,
};

// Installs the process JavaVM exactly once. The supplied JNIEnv must belong to
// the current attached thread and to the VM returned by JNIEnv::GetJavaVM.
int aim_android_runtime_install(JNIEnv* env);

// Clears the process JavaVM before DestroyJavaVM. All consumers of the published VM must
// already be quiescent, and this must run on an attached thread of the same VM.
int aim_android_runtime_uninstall(JNIEnv* env);

#if defined(__cplusplus)
}  // extern "C"
#endif

#endif  // AIM_ANDROID_RUNTIME_HOST_H_
