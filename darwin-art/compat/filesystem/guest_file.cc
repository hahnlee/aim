#include "guest_file.h"
#include "guest_config.h"
#include "darwin_art_bionic_fs.h"
#include <cerrno>
#include <sys/stat.h>

namespace darwin_art::filesystem {
std::unique_ptr<GuestFile> GuestFile::Open(const std::string& path, bool read_write) {
  if (path.empty() || path.front() != '/' || path.find('\0') != std::string::npos) {
    errno = EINVAL;
    return nullptr;
  }
  auto file = std::unique_ptr<GuestFile>(new GuestFile);
  file->fd_ = darwin_art_bionic_open(path.c_str(),
      (read_write ? DARWIN_ART_ANDROID_O_RDWR : DARWIN_ART_ANDROID_O_RDONLY) |
      DARWIN_ART_ANDROID_O_CLOEXEC, 0);
  if (file->fd_ < 0) { RestoreGuestConfigErrno(); return nullptr; }
  return file;
}

GuestFile::~GuestFile() {
  if (fd_ >= 0) {
    const int error = errno;
    darwin_art_bionic_close(fd_);
    errno = error;
  }
}

bool GuestFile::ReadAll(std::string* output) {
  if (!output) { errno = EINVAL; return false; }
  DarwinArtAndroidStat status{};
  if (darwin_art_bionic_fstat(fd_, &status) < 0) { RestoreGuestConfigErrno(); return false; }
  if (!S_ISREG(status.st_mode)) { errno = EISDIR; return false; }
  output->clear();
  char buffer[4096];
  for (;;) {
    const intptr_t count = darwin_art_bionic_read(fd_, buffer, sizeof(buffer));
    if (count == 0) return true;
    if (count < 0) {
      RestoreGuestConfigErrno();
      if (errno == EINTR) continue;
      return false;
    }
    output->append(buffer, static_cast<size_t>(count));
  }
}

bool GuestFile::WriteAtStart(std::string_view bytes) {
  // Linux SEEK_SET is 0. Retain the admitted FD across read, seek and write.
  if (darwin_art_bionic_lseek(fd_, 0, 0) < 0) { RestoreGuestConfigErrno(); return false; }
  while (!bytes.empty()) {
    const intptr_t count = darwin_art_bionic_write(fd_, bytes.data(), bytes.size());
    if (count < 0) {
      RestoreGuestConfigErrno();
      if (errno == EINTR) continue;
      return false;
    }
    if (count == 0) { errno = EIO; return false; }
    bytes.remove_prefix(static_cast<size_t>(count));
  }
  return true;
}
}  // namespace darwin_art::filesystem
