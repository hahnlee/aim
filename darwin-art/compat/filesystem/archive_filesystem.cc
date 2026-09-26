#include "archive_open.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_errno.h"
#include <cerrno>
#include <sys/stat.h>
#include <unistd.h>

namespace {
int HostError(int guest) {
  for (int host = 1; host <= ELAST; ++host) {
    int32_t mapped = 0;
    if (darwin_art_bionic_errno_from_darwin(host, &mapped) && mapped == guest) return host;
  }
  return EIO;
}
int OpenArchive(const char* path) {
  constexpr int kAndroidReadOnlyCloseOnExec = 0x80000;
  const int guest = darwin_art_bionic_fs_open_core(path, kAndroidReadOnlyCloseOnExec, 0);
  if (guest < 0) { errno = HostError(darwin_art_bionic_errno_load()); return -1; }
  int host = -1;
  const int duplicated = darwin_art_bionic_fs_dup_host_fd_core(guest, &host);
  const int failure = duplicated < 0 ? HostError(darwin_art_bionic_errno_load()) : ENOTSUP;
  const int closed = darwin_art_bionic_fs_close_core(guest);
  if (duplicated != 1 || closed != 0) {
    if (host >= 0) close(host);
    errno = duplicated != 1 ? failure : HostError(darwin_art_bionic_errno_load());
    return -1;
  }
  return host; // Independent native descriptor, owned by AOSP ZipArchive.
}
bool StatGuest(const char* path, struct stat* status) {
  DarwinArtAndroidStat guest{};
  if (darwin_art_bionic_fs_stat_core(path, &guest) != 0) {
    errno = HostError(darwin_art_bionic_errno_load());
    return false;
  }
  *status = {};
  // Android and Darwin share the S_IF* type and permission bit values.
  status->st_mode = static_cast<mode_t>(guest.st_mode);
  status->st_size = guest.st_size;
  status->st_nlink = static_cast<nlink_t>(guest.st_nlink);
  return true;
}
}
void darwin_art_install_archive_filesystem(bool enabled) {
  darwin_art_set_archive_opener(enabled ? &OpenArchive : nullptr);
  darwin_art_set_archive_stat(enabled ? &StatGuest : nullptr);
}
