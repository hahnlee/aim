#include <android/file_descriptor_jni.h>

#include <jni.h>

#include <stdbool.h>
#include <stdio.h>
#include <string.h>

static bool g_saw_descriptor;
static bool g_saw_host_fd;
static bool g_saw_set_int;
static bool g_called_set_int;

static jclass FindClass(JNIEnv* env, const char* name) {
  (void)env;
  (void)name;
  return (jclass)(uintptr_t)0x10;
}

static jobject NewGlobalRef(JNIEnv* env, jobject object) {
  (void)env;
  return object;
}

static jmethodID GetMethodID(JNIEnv* env, jclass klass, const char* name,
                             const char* signature) {
  (void)env;
  (void)klass;
  (void)signature;
  if (strcmp(name, "setInt$") == 0) {
    g_saw_set_int = true;
  }
  return (jmethodID)(uintptr_t)0x20;
}

static jmethodID GetStaticMethodID(JNIEnv* env, jclass klass, const char* name,
                                   const char* signature) {
  return GetMethodID(env, klass, name, signature);
}

static jfieldID GetFieldID(JNIEnv* env, jclass klass, const char* name,
                           const char* signature) {
  (void)env;
  (void)klass;
  (void)signature;
  if (strcmp(name, "descriptor") == 0) {
    g_saw_descriptor = true;
  }
  if (strcmp(name, "fd") == 0) {
    g_saw_host_fd = true;
  }
  return (jfieldID)(uintptr_t)0x30;
}

static jboolean IsInstanceOf(JNIEnv* env, jobject object, jclass klass) {
  (void)env;
  (void)object;
  (void)klass;
  return JNI_TRUE;
}

static jint GetIntField(JNIEnv* env, jobject object, jfieldID field) {
  (void)env;
  (void)object;
  (void)field;
  return 73;
}

static void CallVoidMethod(JNIEnv* env, jobject object, jmethodID method, ...) {
  (void)env;
  (void)object;
  (void)method;
  g_called_set_int = true;
}

int main(void) {
  struct JNINativeInterface table = {0};
  table.FindClass = FindClass;
  table.NewGlobalRef = NewGlobalRef;
  table.GetMethodID = GetMethodID;
  table.GetStaticMethodID = GetStaticMethodID;
  table.GetFieldID = GetFieldID;
  table.IsInstanceOf = IsInstanceOf;
  table.GetIntField = GetIntField;
  table.CallVoidMethod = CallVoidMethod;
  JNIEnv env = &table;
  jobject descriptor = (jobject)(uintptr_t)0x40;

  const int value = AFileDescriptor_getFd(&env, descriptor);
  AFileDescriptor_setFd(&env, descriptor, 91);
  if (value != 73 || !g_saw_descriptor || g_saw_host_fd || !g_saw_set_int ||
      !g_called_set_int) {
    fprintf(stderr,
            "nativehelper-device-smoke: value=%d descriptor=%d fd=%d "
            "setInt=%d called=%d\n",
            value, g_saw_descriptor, g_saw_host_fd, g_saw_set_int,
            g_called_set_int);
    return 1;
  }
  puts("nativehelper-device-smoke: descriptor:I get=set-field-method=pass "
       "host-fd:I=absent");
  return 0;
}
