#include <cstdlib>
#include <cstring>
#include <cstdint>

#include <jni.h>

extern void register_libcore_io_Memory(JNIEnv* env);

namespace {

jlong Allocate(JNIEnv* env, jclass, jint byte_count) {
  if (byte_count <= 0) {
    jclass exception = env->FindClass("java/lang/IllegalArgumentException");
    if (exception != nullptr) {
      env->ThrowNew(exception, "byte_count must be positive");
      env->DeleteLocalRef(exception);
    }
    return 0;
  }
  void* memory = std::calloc(1, static_cast<size_t>(byte_count));
  if (memory == nullptr) {
    jclass error = env->FindClass("java/lang/OutOfMemoryError");
    if (error != nullptr) {
      env->ThrowNew(error, "native allocation failed");
      env->DeleteLocalRef(error);
    }
    return 0;
  }
  return static_cast<jlong>(reinterpret_cast<uintptr_t>(memory));
}

void Free(JNIEnv*, jclass, jlong address) {
  std::free(reinterpret_cast<void*>(static_cast<uintptr_t>(address)));
}

JNINativeMethod kSmokeMethods[] = {
    {const_cast<char*>("allocate"), const_cast<char*>("(I)J"),
     reinterpret_cast<void*>(&Allocate)},
    {const_cast<char*>("free"), const_cast<char*>("(J)V"),
     reinterpret_cast<void*>(&Free)},
};

}  // namespace

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  JNIEnv* env = nullptr;
  if (vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  register_libcore_io_Memory(env);
  if (env->ExceptionCheck()) {
    return JNI_ERR;
  }
  jclass smoke = env->FindClass("dev/aim/probe/LibcoreMemorySmoke");
  if (smoke == nullptr ||
      env->RegisterNatives(
          smoke, kSmokeMethods,
          static_cast<jint>(sizeof(kSmokeMethods) /
                            sizeof(kSmokeMethods[0]))) != JNI_OK) {
    env->DeleteLocalRef(smoke);
    return JNI_ERR;
  }
  env->DeleteLocalRef(smoke);
  return JNI_VERSION_1_6;
}
