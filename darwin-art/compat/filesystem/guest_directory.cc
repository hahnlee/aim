#include "guest_directory.h"
#include "guest_config.h"
#include "darwin_art_bionic_errno.h"
#include <cerrno>

namespace darwin_art::filesystem {
std::unique_ptr<GuestDirectory> GuestDirectory::Open(const char* path) {
  if (!path || !*path) { errno = EINVAL; return nullptr; }
  // Allocate before acquiring the resource so allocation failure cannot leak it.
  auto directory = std::unique_ptr<GuestDirectory>(new GuestDirectory);
  directory->token_ = darwin_art_bionic_opendir(path);
  if (!directory->token_) {
    RestoreGuestConfigErrno();
    return nullptr;
  }
  return directory;
}

GuestDirectory::~GuestDirectory() {
  if (token_) {
    const int error = errno;
    darwin_art_bionic_closedir(token_);
    errno = error;
  }
}

GuestDirectory::Result GuestDirectory::Next(DarwinArtAndroidDirent* entry) {
  if (!entry) { errno = EINVAL; return Result::Error; }
  // Bionic leaves errno unchanged at EOF. Clear its separate TLS cell so EOF
  // and an actual read failure remain distinguishable at this host boundary.
  darwin_art_bionic_errno_store(0);
  const auto* next = darwin_art_bionic_readdir(token_);
  if (next) {
    *entry = *next;
    return Result::Entry;
  }
  if (darwin_art_bionic_errno_load() == 0) return Result::End;
  RestoreGuestConfigErrno();
  return Result::Error;
}
}  // namespace darwin_art::filesystem
