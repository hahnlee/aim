#include "system_region.h"

#include <cerrno>
#include <cstdlib>
#include <fcntl.h>
#include <limits>
#include <new>
#include <sys/stat.h>
#include <unistd.h>

namespace darwin_art::memory {

std::unique_ptr<SystemRegion> SystemRegion::Create(size_t size) {
  if (size == 0 || size > static_cast<size_t>(std::numeric_limits<off_t>::max())) {
    errno = EINVAL;
    return nullptr;
  }
  char path[] = "/tmp/darwin-art-system-memory.XXXXXX";
  int writer = mkstemp(path);
  if (writer < 0) return nullptr;
  int reader = -1;
  auto fail = [&]() -> std::unique_ptr<SystemRegion> {
    const int error = errno;
    if (reader >= 0) close(reader);
    close(writer);
    unlink(path);
    errno = error;
    return nullptr;
  };
  if (fcntl(writer, F_SETFD, FD_CLOEXEC) < 0 ||
      ftruncate(writer, static_cast<off_t>(size)) < 0) return fail();
  // dup() preserves O_RDWR; it cannot yield a read-only capability. Open the
  // same inode independently, then remove its name before publishing any FD.
  reader = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (reader < 0) return fail();
  struct stat write_stat{}, read_stat{};
  if (fstat(writer, &write_stat) < 0 || fstat(reader, &read_stat) < 0) return fail();
  if (write_stat.st_dev != read_stat.st_dev || write_stat.st_ino != read_stat.st_ino) {
    errno = ESTALE;
    return fail();
  }
  if (unlink(path) < 0) return fail();
  auto* region = new (std::nothrow) SystemRegion(writer, reader, size);
  if (region == nullptr) {
    close(reader);
    close(writer);
    errno = ENOMEM;
  }
  return std::unique_ptr<SystemRegion>(region);
}

SystemRegion::~SystemRegion() {
  const int error = errno;
  close(reader_);
  close(writer_);
  errno = error;
}

int SystemRegion::DuplicateWriter() const { return fcntl(writer_, F_DUPFD_CLOEXEC, 0); }
int SystemRegion::DuplicateReader() const { return fcntl(reader_, F_DUPFD_CLOEXEC, 0); }

}  // namespace darwin_art::memory
