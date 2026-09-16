#include "classloader_identity.h"
#include <mutex>
#include <vector>

namespace darwin_art::loader {
namespace {
struct Entry { jweak loader; uint64_t identity; };
std::mutex mutex;
std::vector<Entry> entries;
uint64_t next_identity = 1;

uint64_t FindLocked(JNIEnv* env, jobject loader) {
  for (auto it = entries.begin(); it != entries.end();) {
    if (env->IsSameObject(it->loader, nullptr)) {
      env->DeleteWeakGlobalRef(it->loader);
      it = entries.erase(it);
    } else {
      if (env->IsSameObject(it->loader, loader)) return it->identity;
      ++it;
    }
  }
  return 0;
}
}
uint64_t FindClassLoaderIdentity(JNIEnv* env, jobject loader) {
  if (env == nullptr || loader == nullptr || env->ExceptionCheck()) return 0;
  std::lock_guard<std::mutex> lock(mutex);
  return FindLocked(env, loader);
}
uint64_t EnsureClassLoaderIdentity(JNIEnv* env, jobject loader) {
  if (env == nullptr || loader == nullptr || env->ExceptionCheck()) return 0;
  std::lock_guard<std::mutex> lock(mutex);
  if (uint64_t existing = FindLocked(env, loader)) return existing;
  if (next_identity == 0) return 0; // Never wrap into a reused identity.
  jweak weak = env->NewWeakGlobalRef(loader);
  if (weak == nullptr) return 0;
  const uint64_t identity = next_identity;
  try {
    entries.push_back({weak, identity});
  } catch (...) {
    env->DeleteWeakGlobalRef(weak);
    return 0;
  }
  ++next_identity;
  return identity;
}
void ReleaseClassLoaderIdentities(JNIEnv* env) {
  if (env == nullptr) return;
  std::lock_guard<std::mutex> lock(mutex);
  for (const auto& entry : entries) env->DeleteWeakGlobalRef(entry.loader);
  entries.clear();
  // Do not reuse IDs even if a runtime teardown is followed by reinitialization.
}
}
