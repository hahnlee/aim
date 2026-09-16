#include "namespace_handles.h"
#include "namespace_operation.h"
#include "namespace_search_paths.h"
#include "process_namespaces.h"
#include <cstdlib>

namespace darwin_art::loader {
bool NamespaceHandles::InitializeAnonymous(const char* sonames, const char* paths,
    std::string* error) {
  if (error) error->clear();
  NamespaceOperation operation(configured_->registry());
  if (!operation) {
    if (error) *error = "cannot enter linker operation";
    return false;
  }
  auto* parent = Exported("default");
  const auto defaults = ResolveLibraryPaths(paths);
  // AOSP explicitly permits repeated init, unlike direct create with the
  // ALSO_USED_AS_ANONYMOUS flag. Existing namespace identities remain alive.
  auto* child = CreateChild(parent, true, false, false, nullptr,
      defaults.c_str(), nullptr, error, "(anonymous)");
  if (!child) std::abort(); // Original init CHECKs creation success.
  {
    std::lock_guard lock(mutex_);
    anonymous_id_ = Resolve(child);
  }
  // Original init assigns anonymous before linking; failed link leaves the
  // newly created namespace installed, it does not restore its predecessor.
  return Link(child, parent, sonames, error);
}
}

extern "C" bool __loader_android_init_anonymous_namespace(const char* sonames,
    const char* paths) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner) {
    darwin_art_linker_set_error("Android process namespaces are not installed");
    return false;
  }
  std::string error;
  if (owner->InitializeAnonymous(sonames, paths, &error)) return true;
  darwin_art_linker_set_error(error.c_str());
  return false;
}
