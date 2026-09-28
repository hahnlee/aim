#include <jni.h>

#include <algorithm>
#include <array>
#include <cassert>
#include <cstring>
#include <iostream>

extern "C" void register_java_lang_Runtime(JNIEnv *env);

namespace {

std::array<JNINativeMethod, 6> g_methods{};
int g_registration_count = 0;

} // namespace

extern "C" int jniRegisterNativeMethods(JNIEnv *, const char *class_name,
                                        const JNINativeMethod *methods,
                                        int method_count) {
  assert(std::strcmp(class_name, "java/lang/Runtime") == 0);
  assert(method_count == static_cast<int>(g_methods.size()));
  std::copy(methods, methods + method_count, g_methods.begin());
  ++g_registration_count;
  return 0;
}

extern "C" jlong JVM_FreeMemory() { return 1; }
extern "C" jlong JVM_TotalMemory() { return 2; }
extern "C" jlong JVM_MaxMemory() { return 3; }
extern "C" void JVM_GC() {}
extern "C" void JVM_Exit(jint) { __builtin_trap(); }
extern "C" jstring JVM_NativeLoad(JNIEnv *, jstring, jobject, jclass) {
  return reinterpret_cast<jstring>(0x42);
}

int main() {
  register_java_lang_Runtime(nullptr);
  assert(g_registration_count == 1);
  const char *const expected[][2] = {
      {"freeMemory", "()J"},
      {"totalMemory", "()J"},
      {"maxMemory", "()J"},
      {"nativeGc", "()V"},
      {"nativeExit", "(I)V"},
      {"nativeLoad",
       "(Ljava/lang/String;Ljava/lang/ClassLoader;Ljava/lang/Class;)"
       "Ljava/lang/String;"},
  };
  for (size_t index = 0; index < g_methods.size(); ++index) {
    assert(std::strcmp(g_methods[index].name, expected[index][0]) == 0);
    assert(std::strcmp(g_methods[index].signature, expected[index][1]) == 0);
    assert(g_methods[index].fnPtr != nullptr);
  }
  std::cout
      << "android-managed-native-load: Runtime registrar table=6 atomic\n";
  return 0;
}
