#include <jni.h>

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *reserved) {
  (void)vm;
  (void)reserved;
  return JNI_VERSION_1_6;
}

JNIEXPORT jint JNICALL
Java_dev_aim_managedload_ManagedNativeLoad_nativeProbe(JNIEnv *env,
                                                             jclass klass) {
  (void)env;
  (void)klass;
  return 42;
}
