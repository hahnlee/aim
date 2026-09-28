#include "darwin_icu_natives.h"
#include "aim/icu_jni.h"

namespace aim {

bool RegisterIcuCharsetNatives(JNIEnv* env) {
  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK || vm == nullptr) {
    return false;
  }
  return aim_icu_jni_on_load(vm, nullptr) == JNI_VERSION_1_6;
}

void ShutdownIcuCharsetNatives() {
  // The upstream unload path intentionally does not dereference JavaVM: ART
  // invokes it only after all runtime threads have been unregistered.
  aim_icu_jni_on_unload(nullptr, nullptr);
}

}  // namespace aim
