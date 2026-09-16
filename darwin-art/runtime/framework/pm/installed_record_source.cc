#include "installed_record_source.h"
#include <cstddef>
#include <cstdint>
#include <memory>
#include <new>

extern "C" int darwin_art_runtime_query_package_record(
    const char* socket, const char* package, uint8_t* output, size_t capacity, size_t* length);

namespace darwin_art::framework::pm {
namespace {
void Fail(JNIEnv* env, const char* type, const char* message) {
  if (env->ExceptionCheck()) return;
  jclass exception = env->FindClass(type);
  if (exception != nullptr) env->ThrowNew(exception, message);
  env->DeleteLocalRef(exception);
}
}

jstring QueryInstalledRecord(JNIEnv* env, const char* profile_socket, jstring package) {
  if (env == nullptr || env->ExceptionCheck()) return nullptr;
  if (profile_socket == nullptr || *profile_socket == '\0') {
    Fail(env, "java/lang/IllegalStateException", "Missing installed-package socket capability");
    return nullptr;
  }
  if (package == nullptr) {
    Fail(env, "java/lang/IllegalArgumentException", "Missing installed-package name");
    return nullptr;
  }
  constexpr size_t capacity = 60 * 1024;
  std::unique_ptr<uint8_t[]> record(new (std::nothrow) uint8_t[capacity]);
  if (record == nullptr) {
    Fail(env, "java/lang/OutOfMemoryError", "Installed-package record buffer allocation failed");
    return nullptr;
  }
  const char* name = env->GetStringUTFChars(package, nullptr);
  if (name == nullptr) return nullptr;
  size_t length = 0;
  const int status = darwin_art_runtime_query_package_record(
      profile_socket, name, record.get(), capacity, &length);
  env->ReleaseStringUTFChars(package, name);
  if (status == 1) return nullptr;
  if (status != 0 || length > capacity) {
    Fail(env, "java/io/IOException", "Installed-package registry query failed");
    return nullptr;
  }
  // Records contain ordinary UTF-8 filesystem paths, not JNI modified UTF-8.
  // Decode through String's real charset constructor, including non-BMP names.
  if (env->PushLocalFrame(4) < 0) return nullptr;
  jbyteArray bytes = env->NewByteArray(static_cast<jsize>(length));
  if (bytes != nullptr) env->SetByteArrayRegion(
      bytes, 0, static_cast<jsize>(length), reinterpret_cast<const jbyte*>(record.get()));
  jclass string_type = env->ExceptionCheck() ? nullptr : env->FindClass("java/lang/String");
  jmethodID decode = string_type == nullptr ? nullptr : env->GetMethodID(
      string_type, "<init>", "([BLjava/lang/String;)V");
  jstring charset = decode == nullptr ? nullptr : env->NewStringUTF("UTF-8");
  jobject result = charset == nullptr || bytes == nullptr || env->ExceptionCheck() ? nullptr
      : env->NewObject(string_type, decode, bytes, charset);
  return static_cast<jstring>(env->PopLocalFrame(result));
}
}
