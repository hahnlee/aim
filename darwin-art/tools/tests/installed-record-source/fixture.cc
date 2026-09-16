#include "../../../runtime/framework/pm/installed_record_source.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>

// Test-only implementation of the production runtime/profile FFI. This file
// must never be included in the runtime provider or adapter source manifests.
extern "C" int darwin_art_runtime_query_package_record(
    const char* socket, const char* package, uint8_t* output, size_t capacity,
    size_t* length) {
  if (socket == nullptr || package == nullptr || output == nullptr || length == nullptr) {
    return -9;
  }
  if (std::strcmp(package, "missing") == 0) return 1;
  if (std::strcmp(package, "error") == 0) return -7;

  std::string apk = std::strcmp(package, "unicode") == 0
                        ? "/packages/😀/base.apk"
                        : "/installed/base.apk";
  std::string record = "darwin-art-launch-v1\napk=" + apk + "\n";
  if (std::strcmp(package, "normal") == 0) record += "app_id=10042\n";
  record += "metadata=socket=";
  record += socket;
  record += " package=";
  record += package;
  record += '\n';
  if (record.size() > capacity) return -8;
  std::memcpy(output, record.data(), record.size());
  *length = record.size();
  return 0;
}

extern "C" JNIEXPORT jstring JNICALL
Java_InstalledRecordSourceTest_query(JNIEnv* env, jclass, jstring socket, jstring package) {
  const char* socket_utf = nullptr;
  if (socket != nullptr) {
    socket_utf = env->GetStringUTFChars(socket, nullptr);
    if (socket_utf == nullptr) return nullptr;
  }
  jstring result = darwin_art::framework::pm::QueryInstalledRecord(env, socket_utf, package);
  if (socket_utf != nullptr) env->ReleaseStringUTFChars(socket, socket_utf);
  return result;
}
