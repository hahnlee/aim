#include <jni.h>

namespace android {
int register_android_util_Log(JNIEnv* env);
}

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  JNIEnv* env = nullptr;
  if (vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  return android::register_android_util_Log(env) == JNI_OK &&
                 !env->ExceptionCheck()
             ? JNI_VERSION_1_6
             : JNI_ERR;
}
