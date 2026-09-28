#include <jni.h>
#include <stddef.h>

void register_java_io_FileInputStream(JNIEnv* env);

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  register_java_io_FileInputStream(env);
  if ((*env)->ExceptionCheck(env)) {
    return JNI_ERR;
  }
  return JNI_VERSION_1_6;
}
