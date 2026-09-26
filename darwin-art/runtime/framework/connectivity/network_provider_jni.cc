#include "network_provider_jni.h"

#include "network_path_abi.h"

#include <cstdio>
#include <cstdint>
#include <iterator>

namespace darwin_art::framework::connectivity {
namespace {

constexpr uint32_t kAbiVersion = 1;
constexpr uint32_t kStatusSatisfied = 1;
constexpr uint32_t kFlagExpensive = 1u << 0;
constexpr uint32_t kFlagConstrained = 1u << 1;

jlong NativeCreate(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(darwin_art_network_path_create());
}

jobjectArray NativeSnapshot(JNIEnv* env, jclass, jlong raw_handle) {
  DarwinArtNetworkPathSnapshot snapshot{};
  const int status = darwin_art_network_path_snapshot(
      reinterpret_cast<const void*>(raw_handle), &snapshot);
  if (status != 0 || snapshot.abi_version != kAbiVersion ||
      snapshot.struct_size != sizeof(snapshot)) {
    jclass error = env->FindClass("java/lang/IllegalStateException");
    if (error != nullptr) {
      env->ThrowNew(error, "macOS network path snapshot is unavailable");
    }
    return nullptr;
  }

  DarwinArtNetworkLinkFacts facts{};
  if (darwin_art_network_link_facts_snapshot(&facts) != 0
      || facts.abi_version != 1 || facts.struct_size != sizeof(facts)) {
    facts = {};
  }

  // The array is an owned, bounded copy across JNI. Java owns Android policy;
  // no Network.framework or SystemConfiguration object crosses this boundary.
  constexpr jsize kFieldCount = 6 + kDarwinArtNetworkMaxDnsServers;
  jclass string_class = env->FindClass("java/lang/String");
  if (string_class == nullptr) return nullptr;
  jobjectArray result = env->NewObjectArray(kFieldCount, string_class, nullptr);
  if (result == nullptr) return nullptr;
  char value[32];
  auto put_uint = [&](jsize index, uint64_t number) {
    std::snprintf(value, sizeof(value), "%llu",
                  static_cast<unsigned long long>(number));
    env->SetObjectArrayElement(result, index, env->NewStringUTF(value));
  };
  put_uint(0, snapshot.status);
  put_uint(1, snapshot.flags);
  put_uint(2, snapshot.interface_mask);
  put_uint(3, snapshot.generation);
  env->SetObjectArrayElement(result, 4, env->NewStringUTF(facts.interface_name));
  put_uint(5, facts.dns_count);
  for (uint32_t i = 0; i < kDarwinArtNetworkMaxDnsServers; ++i) {
    env->SetObjectArrayElement(result, 6 + static_cast<jsize>(i),
                               env->NewStringUTF(facts.dns_servers[i]));
  }
  return result;
}

// {kind, host, port, exclusions, pacUrl} of the host proxy configuration.
jobjectArray NativeHostProxy(JNIEnv* env, jclass) {
  DarwinArtNetworkProxyFacts facts{};
  if (darwin_art_network_proxy_snapshot(&facts) != 0 || facts.abi_version != 1 ||
      facts.struct_size != sizeof(facts)) {
    facts = {};
  }
  jclass string_class = env->FindClass("java/lang/String");
  if (string_class == nullptr) return nullptr;
  jobjectArray result = env->NewObjectArray(5, string_class, nullptr);
  if (result == nullptr) return nullptr;
  char number[16];
  std::snprintf(number, sizeof(number), "%u", facts.kind);
  env->SetObjectArrayElement(result, 0, env->NewStringUTF(number));
  env->SetObjectArrayElement(result, 1, env->NewStringUTF(facts.host));
  std::snprintf(number, sizeof(number), "%u", facts.port);
  env->SetObjectArrayElement(result, 2, env->NewStringUTF(number));
  env->SetObjectArrayElement(result, 3, env->NewStringUTF(facts.exclusions));
  env->SetObjectArrayElement(result, 4, env->NewStringUTF(facts.pac_url));
  return result;
}

void NativeDestroy(JNIEnv*, jclass, jlong raw_handle) {
  darwin_art_network_path_destroy(reinterpret_cast<void*>(raw_handle));
}

}  // namespace

bool RegisterNetworkPathProvider(JNIEnv* env, jclass provider_class) {
  if (env == nullptr || provider_class == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeCreate"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(NativeCreate)},
      {const_cast<char*>("nativeSnapshot"), const_cast<char*>("(J)[Ljava/lang/String;"),
       reinterpret_cast<void*>(NativeSnapshot)},
      {const_cast<char*>("nativeDestroy"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(NativeDestroy)},
      {const_cast<char*>("nativeHostProxy"), const_cast<char*>("()[Ljava/lang/String;"),
       reinterpret_cast<void*>(NativeHostProxy)},
  };
  return env->RegisterNatives(provider_class, methods, std::size(methods)) == JNI_OK;
}

}  // namespace darwin_art::framework::connectivity
