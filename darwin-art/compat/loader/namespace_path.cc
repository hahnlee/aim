#include "namespace_handles.h"
#include "namespace_operation.h"
#include "namespace_open_reference.h"
#include "darwin_art_guest_image.h"
#include "darwin_art_bionic_errno.h"
#include <cstring>
namespace darwin_art::loader {
int NamespaceHandles::OpenLocalPath(android_namespace_t* handle, const char* path,
    const DarwinArtElfLifecycleCallbacks* lifecycle, LinkerImageLease** output, std::string* error) {
  return OpenPathInNamespace(handle, path, lifecycle, output, error, true);
}
int NamespaceHandles::OpenPathInNamespace(android_namespace_t* handle, const char* path,
    const DarwinArtElfLifecycleCallbacks* lifecycle, LinkerImageLease** output, std::string* error,
    bool search_links, bool nodelete) {
  if (error) error->clear();
  auto fail = [error](const char* message) { if (error) *error = message; return -1; };
  if (!output) return fail("missing path output");
  *output = nullptr;
  if (!path || !*path) return fail("missing library path");
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker operation");
  uint64_t id;
  { std::lock_guard lock(mutex_); id = Resolve(handle); }
  char canonical[4096]{};
  int fd = -1;
  const int status = darwin_art_linker_namespace_open_path_resident(configured_->registry(), id, path,
      darwin_art_fs_open_native_image, darwin_art_bionic_errno_load, canonical, sizeof(canonical), &fd,
      output, search_links);
  NamespaceFile file;
  file.fd.reset(fd);
  if (status == 2) return AcquireNamespaceOpen(output, error, nodelete);
  if (status == 1) return 1;
  if (status != 0) return fail("explicit namespace path admission failed");
  file.canonical_path = canonical;
  file.target = handle;
  const char* basename = std::strrchr(path, '/');
  return LoadAdmittedFile(file, basename ? basename + 1 : path, lifecycle, output, error, search_links, nodelete);
}
}
