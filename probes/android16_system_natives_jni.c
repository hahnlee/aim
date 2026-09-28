#include <jni.h>
#include <stdint.h>
#include <string.h>

#ifndef AIM_SYSTEM_SOURCE
#error "AIM_SYSTEM_SOURCE must name the patched Android 16 System.c"
#endif

// The managed acceptance intentionally shares a translation unit with the
// pinned upstream source so it can exercise its private table implementations
// without registering Android-only methods into the host JDK's System class.
#include AIM_SYSTEM_SOURCE

static jint Smoke_run(JNIEnv* env, jclass ignored, jthrowable throwable) {
  (void)ignored;
  jclass system = (*env)->FindClass(env, "java/lang/System");
  if (system == NULL) return -1;

  jfieldID in_id = (*env)->GetStaticFieldID(
      env, system, "in", "Ljava/io/InputStream;");
  jfieldID out_id = (*env)->GetStaticFieldID(
      env, system, "out", "Ljava/io/PrintStream;");
  jfieldID err_id = (*env)->GetStaticFieldID(
      env, system, "err", "Ljava/io/PrintStream;");
  if (in_id == NULL || out_id == NULL || err_id == NULL) return -2;
  jobject in = (*env)->GetStaticObjectField(env, system, in_id);
  jobject out = (*env)->GetStaticObjectField(env, system, out_id);
  jobject err = (*env)->GetStaticObjectField(env, system, err_id);
  System_setIn0(env, system, in);
  System_setOut0(env, system, out);
  System_setErr0(env, system, err);
  if ((*env)->ExceptionCheck(env)) return -3;

  jstring library = (*env)->NewStringUTF(env, "aim_probe");
  jstring mapped = System_mapLibraryName(env, system, library);
  if (mapped == NULL) return -4;
  const char* mapped_chars = (*env)->GetStringUTFChars(env, mapped, NULL);
  if (mapped_chars == NULL) return -5;
  const int valid_mapping = strstr(mapped_chars, "aim_probe") != NULL &&
                            strstr(mapped_chars, ".so") != NULL;
  (*env)->ReleaseStringUTFChars(env, mapped, mapped_chars);
  if (!valid_mapping) return -6;

  jobjectArray properties = System_specialProperties(env, system);
  if (properties == NULL || (*env)->GetArrayLength(env, properties) != 4)
    return -7;
  for (jsize index = 0; index < 4; ++index) {
    if ((*env)->GetObjectArrayElement(env, properties, index) == NULL) return -8;
  }

  const jlong wall = System_currentTimeMillis();
  const jlong monotonic = System_nanoTime();
  if (wall <= 0 || monotonic <= 0) return -9;

  jstring message = (*env)->NewStringUTF(
      env, "aim Android 16 java.lang.System native acceptance");
  if (message == NULL) return -10;
  System_log(env, system, 'I', message, NULL);
  System_log(env, system, 'W', message, throwable);
  if ((*env)->ExceptionCheck(env)) return -11;
  return 0xff;
}

static JNINativeMethod kMethods[] = {
    {"run", "(Ljava/lang/Throwable;)I", (void*)Smoke_run},
};

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
  jclass smoke = (*env)->FindClass(
      env, "dev/aim/probe/SystemNativesDarwinSmoke");
  if (smoke == NULL ||
      (*env)->RegisterNatives(env, smoke, kMethods,
                             sizeof(kMethods) / sizeof(kMethods[0])) != JNI_OK)
    return JNI_ERR;
  return JNI_VERSION_1_6;
}
