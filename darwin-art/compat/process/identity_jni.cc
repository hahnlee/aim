#include "identity_jni.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

// android_util_Process.cpp getUidForName/getGidForName resolve through
// bionic getpwnam/getgrnam, whose Android name space is the AID table plus
// OEM ranges and per-user application names (bionic grp_pwd.cpp).
namespace darwin_art::process {
namespace {
struct AndroidId {
  const char* name;
  int id;
};
constexpr AndroidId kAndroidIds[] = {
#include "android_ids.inc"
};
// android_filesystem_config.h ranges.
constexpr int kOemReserved[][2] = {{2900, 2999}, {5000, 5999}};
constexpr int kUserOffset = 100000;
constexpr int kAppStart = 10000;
constexpr int kAppEnd = 19999;
constexpr int kCacheGidStart = 20000;
constexpr int kIsolatedStart = 90000;
constexpr int kIsolatedEnd = 99999;

bool AllDigits(const char* text) {
  if (*text == '\0') return false;
  for (const char* cursor = text; *cursor != '\0'; ++cursor) {
    if (*cursor < '0' || *cursor > '9') return false;
  }
  return true;
}

// bionic app_id_from_name: u<user>_a<app>[_cache], u<user>_i<isolated>,
// all_a<app> (shared GIDs are not modelled here).
int AppId(const char* name, bool is_group) {
  unsigned user = 0;
  unsigned app = 0;
  char kind = 0;
  int consumed = 0;
  if (std::sscanf(name, "u%u_%c%u%n", &user, &kind, &app, &consumed) != 3) return -1;
  const char* rest = name + consumed;
  int id = -1;
  if (kind == 'a' && app <= static_cast<unsigned>(kAppEnd - kAppStart)) {
    id = kAppStart + static_cast<int>(app);
    if (is_group && std::strcmp(rest, "_cache") == 0) {
      id = kCacheGidStart + static_cast<int>(app);
      rest += 6;
    }
  } else if (kind == 'i' && app <= static_cast<unsigned>(kIsolatedEnd - kIsolatedStart)) {
    id = kIsolatedStart + static_cast<int>(app);
  }
  if (id < 0 || *rest != '\0' || user > 21473) return -1;
  return static_cast<int>(user) * kUserOffset + id;
}

int Resolve(JNIEnv* env, jstring java_name, bool is_group) {
  if (java_name == nullptr) return -1;
  const char* text = env->GetStringUTFChars(java_name, nullptr);
  if (text == nullptr) return -1;
  const std::string name(text);
  env->ReleaseStringUTFChars(java_name, text);
  if (name.empty()) return -1;
  if (AllDigits(name.c_str())) return std::atoi(name.c_str());
  for (const AndroidId& id : kAndroidIds) {
    if (name == id.name) return id.id;
  }
  unsigned oem = 0;
  int consumed = 0;
  if (std::sscanf(name.c_str(), "oem_%u%n", &oem, &consumed) == 1 &&
      static_cast<size_t>(consumed) == name.size()) {
    for (const auto& range : kOemReserved) {
      if (static_cast<int>(oem) >= range[0] && static_cast<int>(oem) <= range[1]) {
        return static_cast<int>(oem);
      }
    }
    return -1;
  }
  return AppId(name.c_str(), is_group);
}
}  // namespace

jint GetGidForName(JNIEnv* env, jclass, jstring name) { return Resolve(env, name, true); }
jint GetUidForName(JNIEnv* env, jclass, jstring name) { return Resolve(env, name, false); }
}  // namespace darwin_art::process
