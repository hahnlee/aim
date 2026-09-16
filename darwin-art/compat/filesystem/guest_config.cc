#include "guest_config.h"
#include "darwin_art_bionic_errno.h"
#include "guest_file.h"
#include <cerrno>

namespace darwin_art::filesystem {
void RestoreGuestConfigErrno() {
  const int guest = darwin_art_bionic_errno_load();
  for (int host = 1; host <= ELAST; ++host) {
    int mapped = 0;
    if (darwin_art_bionic_errno_from_darwin(host, &mapped) && mapped == guest) {
      errno = host;
      return;
    }
  }
  errno = EIO;
}
bool ReadGuestConfig(const std::string& path, std::string* output) {
  if (!output || path.empty() || path.front() != '/' ||
      path.find('\0') != std::string::npos) { errno = EINVAL; return false; }
  auto file = GuestFile::Open(path);
  return file && file->ReadAll(output);
}
}
