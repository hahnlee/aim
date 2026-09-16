#include "process_namespaces.h"
#include "namespace_handles.h"
#include <cstdlib>
#include <cstring>

// Bionic exposes default configured directories here, not LD_LIBRARY_PATH.
// Undersized output is fatal, as in do_android_get_LD_LIBRARY_PATH.
extern "C" void __loader_android_get_LD_LIBRARY_PATH(char* buffer, size_t capacity) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  std::string paths;
  if (!owner || !owner->DefaultLibraryPath(&paths)) std::abort();
  // AOSP leaves the caller's buffer untouched when the path vector is empty.
  if (paths.empty()) return;
  if (!buffer || capacity <= paths.size()) std::abort();
  std::memcpy(buffer, paths.c_str(), paths.size() + 1);
}

extern "C" void __loader_android_update_LD_LIBRARY_PATH(const char* paths) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner || !owner->UpdateLibrarySearchPath(paths)) std::abort();
}
