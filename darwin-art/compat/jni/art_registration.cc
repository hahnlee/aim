#include "art_registration.h"

#include <jni.h>

#include <cstddef>
#include <new>
#include <vector>

namespace darwin_art::jni {
namespace {

void TranslateAllocationFailure(JNIEnv *env) noexcept {
  if (env == nullptr)
    return;

  // Never clear or replace an exception which ART has already installed.  The
  // caller supplies a real, fully initialized JNIEnv function table.
  if (env->ExceptionCheck()) {
    return;
  }
  jclass oom = env->FindClass("java/lang/OutOfMemoryError");
  if (oom == nullptr)
    return;
  env->ThrowNew(oom, "RegisterNatives method table allocation failed");
  env->DeleteLocalRef(oom);
}

} // namespace

jint ArtRegistration::Register(JNIEnv *env, jclass clazz,
                               const DarwinArtJniNativeMethod *methods,
                               jint count) noexcept {
  if (env == nullptr || clazz == nullptr || count < 0 ||
      (count != 0 && methods == nullptr)) {
    return JNI_ERR;
  }

  try {
    std::vector<JNINativeMethod> native_methods;
    native_methods.reserve(static_cast<std::size_t>(count));
    for (jint index = 0; index < count; ++index) {
      // Materialize the platform struct rather than aliasing the compatible-
      // looking DarwinArtJniNativeMethod layout.
      native_methods.push_back({const_cast<char *>(methods[index].name),
                                const_cast<char *>(methods[index].signature),
                                methods[index].function});
    }
    return env->RegisterNatives(
        clazz, count == 0 ? nullptr : native_methods.data(), count);
  } catch (const std::bad_alloc &) {
    TranslateAllocationFailure(env);
    return JNI_ERR;
  }
}

} // namespace darwin_art::jni
