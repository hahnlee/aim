#include <jni.h>
#include <stdarg.h>
#include <stddef.h>

extern JavaVM* darwin_art_jni_fixture_vm(void);

static jint NativePing(JNIEnv* env, jobject receiver) {
  (void)env;
  (void)receiver;
  return 42;
}

static JNINativeMethod kMethods[] = {
    {"nativePing", "()I", (void*)NativePing},
};

/* A ...V call made from Android code, with an Android va_list. */
static jfloat CallHalfFloatV(JNIEnv* env, jobject object, jclass clazz,
                             jmethodID method, ...) {
  va_list arguments;
  va_start(arguments, method);
  const jfloat result =
      (*env)->CallNonvirtualFloatMethodV(env, object, clazz, method, arguments);
  va_end(arguments);
  return result;
}

/* Every JNIEnv slot except the four reserved ones is populated, and the
 * variadic, va_list and nonvirtual entries reach the backend with their
 * Android-ABI arguments intact. */
static int CheckProxyTable(JNIEnv* env, jclass bridge) {
  void* const* slots = (void* const*)*env;
  const size_t count = sizeof(struct JNINativeInterface) / sizeof(void*);
  for (size_t index = 4; index < count; ++index) {
    if (slots[index] == NULL) return 0;
  }
  jobject receiver = (jobject)bridge;
  jmethodID sum_long = (*env)->GetMethodID(env, bridge, "sumLong", "(IJ)J");
  jmethodID mix_double =
      (*env)->GetStaticMethodID(env, bridge, "mixDouble", "(DI)D");
  jmethodID sum_int = (*env)->GetMethodID(env, bridge, "sumInt", "(II)I");
  jmethodID half_float = (*env)->GetMethodID(env, bridge, "halfFloat", "(D)F");
  if (sum_long == NULL || mix_double == NULL || sum_int == NULL ||
      half_float == NULL)
    return 0;
  if ((*env)->CallLongMethod(env, receiver, sum_long, (jint)7,
                             (jlong)0x100000000LL) != 0x100000007LL)
    return 0;
  if ((*env)->CallStaticDoubleMethod(env, bridge, mix_double, 1.5, (jint)2) !=
      3.5)
    return 0;
  if ((*env)->CallNonvirtualIntMethod(env, receiver, bridge, sum_int, (jint)5,
                                      (jint)6) != 11)
    return 0;
  if (CallHalfFloatV(env, receiver, bridge, half_float, 5.0) != 2.5f) return 0;
  return 1;
}

JNIEXPORT jint JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
  if ((*env)->GetVersion(env) != JNI_VERSION_1_6) return JNI_ERR;
  if ((*env)->ExceptionCheck(env)) return JNI_ERR;
  jclass bridge = (*env)->FindClass(env, "fixture/Bridge");
  if (bridge == NULL) return JNI_ERR;
  if ((*env)->RegisterNatives(env, bridge, kMethods, 1) != JNI_OK) return JNI_ERR;
  if (!CheckProxyTable(env, bridge)) return JNI_ERR;
  jclass exception = (*env)->FindClass(env, "java/lang/RuntimeException");
  if (exception == NULL) return JNI_ERR;
  if ((*env)->ThrowNew(env, exception, "proxy-fixture") != JNI_OK) return JNI_ERR;
  if (!(*env)->ExceptionCheck(env)) return JNI_ERR;
  return JNI_VERSION_1_6;
}

JNIEXPORT jint jni_proxy_fixture_run(void) {
  JavaVM* vm = darwin_art_jni_fixture_vm();
  return vm == NULL ? JNI_ERR : JNI_OnLoad(vm, NULL);
}
