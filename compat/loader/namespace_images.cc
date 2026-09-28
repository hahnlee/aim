#include "namespace_handles.h"

namespace aim::loader {
int NamespaceHandles::FindResidentFile(android_namespace_t* handle,
    const AimElfFileIdentity& file, LinkerImageLease** output, bool search_links) {
  if (!output) return -1;
  *output = nullptr;
  std::lock_guard lock(mutex_);
  const auto id = Resolve(handle);
  if (!id) return -1;
  return aim_linker_namespace_find_file_scoped(configured_->registry(), id,
      file.device, file.inode, file.offset, search_links, output);
}
int NamespaceHandles::FindResident(android_namespace_t* handle, const char* soname,
                                   LinkerImageLease** output) {
  if (!output) return -1;
  *output = nullptr;
  std::lock_guard lock(mutex_);
  const auto id = Resolve(handle);
  if (!id) return -1;
  return aim_linker_namespace_find(configured_->registry(), id, soname, output);
}
}  // namespace aim::loader
