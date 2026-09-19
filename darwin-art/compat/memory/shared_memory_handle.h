#pragma once

#include <cerrno>
#include <unistd.h>

extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int, int *);
extern "C" int darwin_art_bionic_fs_owns_fd_core(int);

namespace darwin_art::memory {
// Resolve only guest memory operations. Native marker/import/export paths do
// not use this boundary. A failed FS resolution is never retried as a raw FD.
class SharedMemoryHandle final {
public:
  explicit SharedMemoryHandle(int guest) noexcept {
    const int status = darwin_art_bionic_fs_dup_host_fd_core(guest, &native_);
    owned_ = status == 1;
    valid_ = status >= 0;
    if (status == 0) native_ = guest;
  }
  ~SharedMemoryHandle() noexcept {
    const int saved = errno;
    if (owned_ && native_ >= 0) (void)::close(native_);
    errno = saved;
  }
  SharedMemoryHandle(const SharedMemoryHandle &) = delete;
  SharedMemoryHandle &operator=(const SharedMemoryHandle &) = delete;
  bool valid() const noexcept { return valid_; }
  bool filesystem_owned() const noexcept { return owned_; }
  int native() const noexcept { return native_; }
private:
  int native_ = -1;
  bool owned_ = false;
  bool valid_ = false;
};
}
