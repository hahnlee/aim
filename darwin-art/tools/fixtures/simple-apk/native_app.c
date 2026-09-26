#include <jni.h>
#include <stddef.h>

JNIIMPORT jint JNICALL DarwinArtChildAnswer(void);

static jint NativeAnswer(JNIEnv* env, jclass clazz) {
  (void)env;
  (void)clazz;
  return DarwinArtChildAnswer();
}

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK || env == NULL) {
    return JNI_ERR;
  }
  jclass activity = (*env)->FindClass(env, "dev/darwinart/simple/MainActivity");
  if (activity == NULL) return JNI_ERR;
  JNINativeMethod methods[] = {
      {"nativeAnswer", "()I", (void*)NativeAnswer},
  };
  const jint status = (*env)->RegisterNatives(
      env, activity, methods, (jint)(sizeof(methods) / sizeof(methods[0])));
  (*env)->DeleteLocalRef(env, activity);
  if (status != JNI_OK) return JNI_ERR;
  return JNI_VERSION_1_6;
}
