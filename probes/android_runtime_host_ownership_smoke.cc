#include "aim/android_runtime_host.h"

#include <android_runtime/AndroidRuntime.h>

#include <cstdio>

namespace {

JNIEnv* g_env_one = nullptr;
JNIEnv* g_env_two = nullptr;
JavaVM* g_vm_one = nullptr;
JavaVM* g_vm_two = nullptr;
bool g_vm_one_attached = true;

jint GetJavaVm(JNIEnv* env, JavaVM** vm) {
  if (env == g_env_one) {
    *vm = g_vm_one;
    return JNI_OK;
  }
  if (env == g_env_two) {
    *vm = g_vm_two;
    return JNI_OK;
  }
  *vm = nullptr;
  return JNI_ERR;
}

jint GetEnv(JavaVM* vm, void** env, jint version) {
  if (version != JNI_VERSION_1_6) {
    *env = nullptr;
    return JNI_EVERSION;
  }
  if (vm == g_vm_one) {
    if (!g_vm_one_attached) {
      *env = nullptr;
      return JNI_EDETACHED;
    }
    *env = g_env_one;
    return JNI_OK;
  }
  if (vm == g_vm_two) {
    *env = g_env_two;
    return JNI_OK;
  }
  *env = nullptr;
  return JNI_ERR;
}

bool Expect(bool condition, const char* message) {
  if (!condition) {
    std::fprintf(stderr, "android-runtime-host-smoke: %s\n", message);
  }
  return condition;
}

}  // namespace

int main() {
  JNINativeInterface native_interface{};
  native_interface.GetJavaVM = GetJavaVm;
  JNIEnv env_one{&native_interface};
  JNIEnv env_two{&native_interface};

  JNIInvokeInterface invoke_interface{};
  invoke_interface.GetEnv = GetEnv;
  JavaVM vm_one{&invoke_interface};
  JavaVM vm_two{&invoke_interface};

  g_env_one = &env_one;
  g_env_two = &env_two;
  g_vm_one = &vm_one;
  g_vm_two = &vm_two;

  bool ok = true;
  ok &= Expect(android::AndroidRuntime::getJavaVM() == nullptr,
               "VM visible before install");
  ok &= Expect(android::AndroidRuntime::getJNIEnv() == nullptr,
               "JNIEnv visible before install");
  ok &= Expect(aim_android_runtime_install(nullptr) ==
                   AIM_ANDROID_RUNTIME_INVALID_ENV,
               "null install accepted");
  ok &= Expect(aim_android_runtime_install(&env_one) ==
                   AIM_ANDROID_RUNTIME_OK,
               "install failed");
  ok &= Expect(android::AndroidRuntime::getJavaVM() == &vm_one,
               "installed VM mismatch");
  ok &= Expect(android::AndroidRuntime::getJNIEnv() == &env_one,
               "attached JNIEnv mismatch");
  ok &= Expect(aim_android_runtime_install(&env_one) ==
                   AIM_ANDROID_RUNTIME_ALREADY_INSTALLED,
               "duplicate install not diagnosed");
  ok &= Expect(aim_android_runtime_install(&env_two) ==
                   AIM_ANDROID_RUNTIME_DIFFERENT_VM,
               "different VM install accepted");

  g_vm_one_attached = false;
  ok &= Expect(android::AndroidRuntime::getJNIEnv() == nullptr,
               "detached thread received JNIEnv");
  ok &= Expect(aim_android_runtime_uninstall(&env_one) ==
                   AIM_ANDROID_RUNTIME_CURRENT_THREAD_NOT_ATTACHED,
               "detached uninstall accepted");
  g_vm_one_attached = true;

  ok &= Expect(aim_android_runtime_uninstall(&env_two) ==
                   AIM_ANDROID_RUNTIME_DIFFERENT_VM,
               "different VM uninstall accepted");
  ok &= Expect(aim_android_runtime_uninstall(&env_one) ==
                   AIM_ANDROID_RUNTIME_OK,
               "uninstall failed");
  ok &= Expect(android::AndroidRuntime::getJavaVM() == nullptr,
               "VM visible after uninstall");
  ok &= Expect(android::AndroidRuntime::getJNIEnv() == nullptr,
               "JNIEnv visible after uninstall");
  ok &= Expect(aim_android_runtime_uninstall(&env_one) ==
                   AIM_ANDROID_RUNTIME_ALREADY_UNINSTALLED,
               "duplicate uninstall not diagnosed");
  ok &= Expect(aim_android_runtime_install(&env_one) ==
                   AIM_ANDROID_RUNTIME_ALREADY_UNINSTALLED,
               "one-shot VM ownership was reusable");

  if (ok) {
    std::puts("android-runtime-host-smoke: install/getenv/detach/uninstall=pass");
  }
  return ok ? 0 : 1;
}
