#include <jni.h>
#include <stdint.h>

extern int AimGenericChildValue(void);

static int g_generic_graph_value;

__attribute__((constructor)) static void GenericGraphInitialize(void) {
  g_generic_graph_value = AimGenericChildValue();
}

static jlong GenericNativeAdd(JNIEnv *env, jclass type, jint left, jlong middle,
                              jint right) {
  static const char expected_string[] = "generic-proxy";
  static const jbyte expected_bytes[] = {-128, -1, 0, 42, 127};
  if (env == 0 || type == 0) return -1;
  jstring string = (*env)->NewStringUTF(env, expected_string);
  jboolean is_copy = JNI_FALSE;
  const char *utf_bytes =
      string == 0 ? 0 : (*env)->GetStringUTFChars(env, string, &is_copy);
  int valid = string != 0 && utf_bytes != 0 &&
              (*env)->GetStringUTFLength(env, string) ==
                  (jsize)(sizeof(expected_string) - 1);
  for (size_t index = 0; valid && index < sizeof(expected_string); ++index) {
    valid = utf_bytes[index] == expected_string[index];
  }
  if (utf_bytes != 0) (*env)->ReleaseStringUTFChars(env, string, utf_bytes);
  if (string != 0) (*env)->DeleteLocalRef(env, string);

  jbyteArray byte_array =
      (*env)->NewByteArray(env, (jsize)sizeof(expected_bytes));
  valid = valid && byte_array != 0;
  if (byte_array != 0) {
    (*env)->SetByteArrayRegion(env, byte_array, 0,
                               (jsize)sizeof(expected_bytes), expected_bytes);
    valid = valid && !(*env)->ExceptionCheck(env) &&
            (*env)->GetArrayLength(env, byte_array) ==
                (jsize)sizeof(expected_bytes);

    jobject global = (*env)->NewGlobalRef(env, byte_array);
    (*env)->DeleteLocalRef(env, byte_array);
    jobject local = global == 0 ? 0 : (*env)->NewLocalRef(env, global);
    if (global != 0) (*env)->DeleteGlobalRef(env, global);
    valid = valid && global != 0 && local != 0;
    if (local != 0) {
      jbyte observed[sizeof(expected_bytes)] = {0};
      (*env)->GetByteArrayRegion(env, (jbyteArray)local, 0,
                                 (jsize)sizeof(observed), observed);
      for (size_t index = 0; valid && index < sizeof(observed); ++index) {
        valid = observed[index] == expected_bytes[index];
      }

      jbyte ignored = 0;
      (*env)->GetByteArrayRegion(env, (jbyteArray)local,
                                 (jsize)sizeof(observed), 1, &ignored);
      valid = valid && (*env)->ExceptionCheck(env);
      jthrowable exception = (*env)->ExceptionOccurred(env);
      valid = valid && exception != 0;
      (*env)->ExceptionClear(env);
      valid = valid && !(*env)->ExceptionCheck(env);
      if (exception != 0) (*env)->DeleteLocalRef(env, exception);
      (*env)->DeleteLocalRef(env, local);
    }
  }
  return valid ? (jlong)left + middle + (jlong)right : -1;
}

__attribute__((visibility("default"))) jint JNI_OnLoad(JavaVM *vm,
                                                       void *reserved) {
  JNIEnv *env = 0;
  if (vm == 0 || reserved != 0 || g_generic_graph_value != 20 ||
      (*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK || env == 0) {
    return JNI_ERR;
  }
  jclass fixture =
      (*env)->FindClass(env, "darwin/art/nativefixture/NativeFixture");
  JNINativeMethod method = {
      .name = "nativeAdd",
      .signature = "(IJI)J",
      .fnPtr = (void *)&GenericNativeAdd,
  };
  JNINativeMethod foreign = method;
  foreign.fnPtr = (void *)(uintptr_t)1;
  if (fixture == 0 ||
      (*env)->RegisterNatives(env, fixture, &foreign, 1) == JNI_OK ||
      (*env)->RegisterNatives(env, fixture, &method, 1) != JNI_OK) {
    return JNI_ERR;
  }
  return JNI_VERSION_1_6;
}
