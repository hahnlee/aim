#include "archive_open.h"
#include <atomic>
#include <cerrno>
#include <cstring>
#include <fcntl.h>

namespace { std::atomic<DarwinArtArchiveOpener> guest_opener{nullptr}; }
void darwin_art_set_archive_opener(DarwinArtArchiveOpener opener) {
  guest_opener.store(opener, std::memory_order_release);
}
int darwin_art_archive_open(const char* path, int flags, ...) {
  // The AOSP ZIP reader opens read-only archives; never accept write intent.
  if (path == nullptr || (flags & O_ACCMODE) != O_RDONLY || (flags & (O_CREAT | O_TRUNC))) {
    errno = EINVAL;
    return -1;
  }
  const auto opener = guest_opener.load(std::memory_order_acquire);
  const char* roots[] = {"/system/", "/apex/", "/data/", "/vendor/", "/product/", "/storage/"};
  for (const char* root : roots) {
    if (std::strncmp(path, root, std::strlen(root)) == 0 && opener != nullptr) return opener(path);
  }
  return open(path, flags);
}
