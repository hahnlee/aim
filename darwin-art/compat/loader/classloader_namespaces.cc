#include "classloader_namespaces.h"

#include "classloader_identity.h"
#include "library_search.h"

#include <mutex>
#include <unordered_map>

namespace darwin_art::loader {
namespace {

struct Namespace {
  int32_t target_sdk_version;
  bool is_shared;
  std::string dex_path;
  std::string library_search_path;
  std::string library_permitted_path;
  std::string uses_library_list;
};

std::mutex g_mutex;
bool g_initialized = false;
std::unordered_map<uint64_t, Namespace> g_namespaces;

bool CopyString(JNIEnv* env, jstring value, std::string* output) {
  output->clear();
  if (value == nullptr) return true;
  const char* chars = env->GetStringUTFChars(value, nullptr);
  if (chars == nullptr) return false;
  *output = chars;
  env->ReleaseStringUTFChars(value, chars);
  return !env->ExceptionCheck();
}

bool SameNamespace(const Namespace& left, const Namespace& right) {
  return left.target_sdk_version == right.target_sdk_version &&
         left.is_shared == right.is_shared && left.dex_path == right.dex_path &&
         left.library_search_path == right.library_search_path &&
         left.library_permitted_path == right.library_permitted_path &&
         left.uses_library_list == right.uses_library_list;
}

}  // namespace

void InitializeClassLoaderNamespaces() {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_initialized = true;
}

void ResetClassLoaderNamespaces() {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_namespaces.clear();
  g_initialized = false;
}

bool CreateClassLoaderNamespace(JNIEnv* env, int32_t target_sdk_version,
                                jobject class_loader, bool is_shared,
                                jstring dex_path, jstring library_search_path,
                                jstring library_permitted_path,
                                jstring uses_library_list,
                                std::string* error) {
  if (env == nullptr || class_loader == nullptr || error == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  Namespace created{target_sdk_version, is_shared, {}, {}, {}, {}};
  if (!CopyString(env, dex_path, &created.dex_path) ||
      !CopyString(env, library_search_path, &created.library_search_path) ||
      !CopyString(env, library_permitted_path,
                  &created.library_permitted_path) ||
      !CopyString(env, uses_library_list, &created.uses_library_list)) {
    return false;
  }
  const uint64_t identity = EnsureClassLoaderIdentity(env, class_loader);
  if (identity == 0) {
    *error = "ClassLoader identity allocation failed";
    return false;
  }
  std::lock_guard<std::mutex> lock(g_mutex);
  if (!g_initialized) {
    *error = "NativeLoader is not initialized";
    return false;
  }
  auto [position, inserted] = g_namespaces.emplace(identity, created);
  if (!inserted && !SameNamespace(position->second, created)) {
    *error = "ClassLoader namespace already exists with different policy";
    return false;
  }
  return true;
}

bool FindClassLoaderLibrary(JNIEnv* env, jobject class_loader,
                            const char* soname, std::string* path,
                            std::string* error) {
  if (env == nullptr || class_loader == nullptr || soname == nullptr ||
      path == nullptr || error == nullptr || env->ExceptionCheck()) {
    return false;
  }
  const uint64_t identity = FindClassLoaderIdentity(env, class_loader);
  std::string search_path;
  {
    std::lock_guard<std::mutex> lock(g_mutex);
    const auto found = g_namespaces.find(identity);
    if (!g_initialized || identity == 0 || found == g_namespaces.end()) {
      *error = "ClassLoader namespace not found";
      return false;
    }
    search_path = found->second.library_search_path;
  }
  *path = FindLibraryInSearchPath(soname, search_path);
  if (path->empty()) {
    *error = std::string("Library not found in ClassLoader namespace: ") + soname;
    return false;
  }
  return true;
}

}  // namespace darwin_art::loader

namespace android {

extern "C" jstring CreateClassLoaderNamespace(
    JNIEnv* env, int32_t target_sdk_version, jobject class_loader,
    bool is_shared, jstring dex_path, jstring library_path,
    jstring permitted_path, jstring uses_library_list) {
  std::string error;
  if (darwin_art::loader::CreateClassLoaderNamespace(
          env, target_sdk_version, class_loader, is_shared, dex_path,
          library_path, permitted_path, uses_library_list, &error)) {
    return nullptr;
  }
  return env->ExceptionCheck() ? nullptr : env->NewStringUTF(error.c_str());
}

}  // namespace android
