#include "namespace_handles.h"
#include "namespace_operation.h"

namespace darwin_art::loader {
int NamespaceHandles::OpenPath(android_namespace_t* caller, const char* path,
    const DarwinArtElfLifecycleCallbacks* lifecycle, LinkerImageLease** output, std::string* error, bool nodelete) {
  NamespaceOperation operation(configured_->registry());
  if (!operation) {
    if (output) *output = nullptr;
    if (error) *error = "cannot enter linker operation";
    return -1;
  }
  const int local = OpenPathInNamespace(caller, path, lifecycle, output, error, true, nodelete);
  if (local == 0 || !output || !path) return local;
  const std::string original = error ? *error : "";
  uint64_t id;
  { std::lock_guard lock(mutex_); id = Resolve(caller); }
  for (size_t index = 0;; ++index) {
    uint64_t target = 0;
    const int status = darwin_art_linker_namespace_path_target(
        configured_->registry(), id, path, index, &target);
    if (status != 0) break;
    android_namespace_t* handle;
    { std::lock_guard lock(mutex_); handle = Intern(target); }
    // AOSP load_library(target, search_linked_namespaces=false).
    if (OpenPathInNamespace(handle, path, lifecycle, output, error, false, nodelete) == 0) return 0;
  }
  if (error) *error = original;
  return local;
}
}  // namespace darwin_art::loader
