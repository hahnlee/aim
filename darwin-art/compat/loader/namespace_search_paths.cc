#include "namespace_handles.h"
#include "namespace_operation.h"
#include "namespace_search_paths.h"
#include "linker_utils.h"

namespace darwin_art::loader {
std::string ResolveLibraryPaths(const char* paths) {
  std::vector<std::string> raw, resolved;
  // Original Bionic parsing/canonicalization, built against guest filesystem
  // adapters. Includes zip-entry handling; no Darwin realpath/environment use.
  split_path(paths, ":", &raw);
  resolve_paths(raw, &resolved);
  std::string joined;
  for (const auto& path : resolved) {
    if (!joined.empty()) joined += ':';
    joined += path;
  }
  return joined;
}
bool NamespaceHandles::UpdateLibrarySearchPath(const char* paths) {
  NamespaceOperation operation(configured_->registry());
  if (!operation) return false;
  const auto joined = ResolveLibraryPaths(paths);
  return darwin_art_linker_replace_search_paths(configured_->registry(),
      configured_->Find("default"), joined.c_str()) == 0;
}
}
