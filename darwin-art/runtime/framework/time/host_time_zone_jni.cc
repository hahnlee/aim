#include "host_time_zone_jni.h"

#include <notify.h>
#include <unistd.h>

#include <cerrno>
#include <climits>
#include <cstdint>
#include <iterator>
#include <mutex>
#include <string>

namespace darwin_art::framework::time {
namespace {

// notify_keys.h kNotifyTimeZoneChange: posted when the host time zone changes.
constexpr char kTimeZoneChange[] = "com.apple.system.timezone";

// The IANA zone named by macOS's /etc/localtime link
// (/var/db/timezone/zoneinfo/Asia/Seoul), or null when the link is unusable.
jstring NativeHostTimeZone(JNIEnv* env, jclass) {
  char link[PATH_MAX];
  const ssize_t length = readlink("/etc/localtime", link, sizeof(link) - 1);
  if (length <= 0) return nullptr;
  const std::string path(link, static_cast<size_t>(length));
  const auto marker = path.rfind("/zoneinfo/");
  if (marker == std::string::npos) return nullptr;
  const std::string zone = path.substr(marker + 10);
  if (zone.empty() || zone.front() == '/' || zone.back() == '/') return nullptr;
  for (const char c : zone) {
    const bool allowed = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                         (c >= '0' && c <= '9') || c == '/' || c == '_' ||
                         c == '-' || c == '+';
    if (!allowed) return nullptr;
  }
  if (zone.find("..") != std::string::npos || zone.find("//") != std::string::npos) {
    return nullptr;
  }
  return env->NewStringUTF(zone.c_str());
}

// Blocks until the host posts a time zone change; false if the notification
// channel cannot be used, which ends the caller's watch.
jboolean NativeAwaitHostTimeZoneChange(JNIEnv*, jclass) {
  static std::once_flag once;
  static int descriptor = -1;
  std::call_once(once, [] {
    int token = 0;
    if (notify_register_file_descriptor(kTimeZoneChange, &descriptor, 0, &token) !=
        NOTIFY_STATUS_OK) {
      descriptor = -1;
    }
  });
  if (descriptor < 0) return JNI_FALSE;
  int32_t token = 0;
  ssize_t result;
  do {
    result = read(descriptor, &token, sizeof(token));
  } while (result < 0 && errno == EINTR);
  return result == sizeof(token) ? JNI_TRUE : JNI_FALSE;
}

}  // namespace

bool RegisterHostTimeZoneProvider(JNIEnv* env, jclass provider_class) {
  if (env == nullptr || provider_class == nullptr || env->ExceptionCheck()) {
    return false;
  }
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeHostTimeZone"), const_cast<char*>("()Ljava/lang/String;"),
       reinterpret_cast<void*>(NativeHostTimeZone)},
      {const_cast<char*>("nativeAwaitHostTimeZoneChange"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(NativeAwaitHostTimeZoneChange)},
  };
  return env->RegisterNatives(provider_class, methods, std::size(methods)) == JNI_OK;
}

}  // namespace darwin_art::framework::time
