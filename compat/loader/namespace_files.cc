#include "namespace_handles.h"
#include "aim_guest_image.h"
#include "aim_bionic_errno.h"

namespace aim::loader {
int NamespaceHandles::OpenFile(android_namespace_t* handle, const char* soname,
    const char* runpath, const char* source_image, NamespaceFile* output) {
  if (!output) return -1;
  *output = NamespaceFile{};
  std::lock_guard lock(mutex_);
  const auto id = Resolve(handle);
  if (!id) return -1;
  char canonical[4096];
  uint64_t target = 0;
  int fd = -1;
  const auto result = aim_linker_namespace_open(configured_->registry(), id,
      soname, runpath, source_image, aim_fs_open_native_directory,
      aim_fs_open_native_image, aim_bionic_errno_load,
      canonical, sizeof(canonical), &target, &fd);
  android::base::unique_fd owned(fd);
  if (result != 0) return result;
  // Construct before transferring the fd, including on allocation failure.
  std::string path(canonical);
  auto* target_handle = Intern(target);
  output->canonical_path = std::move(path);
  output->target = target_handle;
  output->fd = std::move(owned);
  return 0;
}
}  // namespace aim::loader
