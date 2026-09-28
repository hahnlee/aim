#include <jni.h>

void register_libcore_io_AsynchronousCloseMonitor(JNIEnv* env);

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  JNIEnv* env = nullptr;
  if (vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  register_libcore_io_AsynchronousCloseMonitor(env);
  return env->ExceptionCheck() ? JNI_ERR : JNI_VERSION_1_6;
}
