#include "host_command_jni.h"

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

extern "C" void* darwin_art_runtime_host_command_connect();
extern "C" uint8_t* darwin_art_runtime_host_command_next(void* listener, size_t* length);
extern "C" void darwin_art_runtime_host_command_free(uint8_t* bytes, size_t length);
extern "C" int darwin_art_runtime_host_command_reply(void* listener, uint32_t status,
                                                     const uint8_t* output, size_t length);

namespace darwin_art::framework::system {
namespace {

void* Listener(jlong handle) { return reinterpret_cast<void*>(static_cast<intptr_t>(handle)); }

jlong Connect(JNIEnv*, jclass) {
  return static_cast<jlong>(reinterpret_cast<intptr_t>(darwin_art_runtime_host_command_connect()));
}

// The next command's arguments, or null once the daemon connection ends.
jobjectArray Next(JNIEnv* env, jclass, jlong handle) {
  size_t length = 0;
  uint8_t* bytes = darwin_art_runtime_host_command_next(Listener(handle), &length);
  if (bytes == nullptr) return nullptr;
  std::vector<std::string> arguments(1);
  for (size_t i = 0; i < length; ++i) {
    if (bytes[i] == 0) {
      arguments.emplace_back();
    } else {
      arguments.back().push_back(static_cast<char>(bytes[i]));
    }
  }
  darwin_art_runtime_host_command_free(bytes, length);
  jclass string_class = env->FindClass("java/lang/String");
  if (string_class == nullptr) return nullptr;
  jobjectArray result =
      env->NewObjectArray(static_cast<jsize>(arguments.size()), string_class, nullptr);
  env->DeleteLocalRef(string_class);
  for (size_t i = 0; result != nullptr && i < arguments.size(); ++i) {
    jstring argument = env->NewStringUTF(arguments[i].c_str());
    if (argument == nullptr) return nullptr;
    env->SetObjectArrayElement(result, static_cast<jsize>(i), argument);
    env->DeleteLocalRef(argument);
  }
  return result;
}

jboolean Reply(JNIEnv* env, jclass, jlong handle, jint status, jbyteArray output) {
  const jsize length = output == nullptr ? 0 : env->GetArrayLength(output);
  std::vector<uint8_t> bytes(static_cast<size_t>(length));
  if (length > 0) {
    env->GetByteArrayRegion(output, 0, length, reinterpret_cast<jbyte*>(bytes.data()));
  }
  return darwin_art_runtime_host_command_reply(Listener(handle), static_cast<uint32_t>(status),
                                               bytes.data(), bytes.size()) == 0
             ? JNI_TRUE
             : JNI_FALSE;
}

}  // namespace

bool RegisterHostCommandService(JNIEnv* env, jclass service) {
  if (service == nullptr) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("nativeConnect"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(Connect)},
      {const_cast<char*>("nativeNext"), const_cast<char*>("(J)[Ljava/lang/String;"),
       reinterpret_cast<void*>(Next)},
      {const_cast<char*>("nativeReply"), const_cast<char*>("(JI[B)Z"),
       reinterpret_cast<void*>(Reply)},
  };
  return env->RegisterNatives(service, methods, 3) == JNI_OK;
}

}  // namespace darwin_art::framework::system
