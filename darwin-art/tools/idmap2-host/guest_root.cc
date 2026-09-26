// idmap2 records the target and overlay paths it was given in each idmap
// header, and ApkAssets later opens the overlay by that recorded path. At
// image assembly the APKs are staged on the host, so idmap2 is given their
// device paths and every archive open is resolved under the staged image
// root named by DARWIN_ART_IDMAP2_GUEST_ROOT.
#include "archive_open.h"

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <string>

namespace {

const char* guest_root = nullptr;

int OpenUnderGuestRoot(const char* path) {
  if (std::strstr(path, "/../") != nullptr) {
    errno = EINVAL;
    return -1;
  }
  const std::string host = std::string(guest_root) + path;
  return open(host.c_str(), O_RDONLY | O_CLOEXEC);
}

__attribute__((constructor)) void InstallGuestRoot() {
  guest_root = std::getenv("DARWIN_ART_IDMAP2_GUEST_ROOT");
  if (guest_root == nullptr || guest_root[0] != '/') {
    std::fprintf(stderr, "idmap2: DARWIN_ART_IDMAP2_GUEST_ROOT must name the staged image root\n");
    std::exit(64);
  }
  darwin_art_set_archive_opener(&OpenUnderGuestRoot);
}

}  // namespace
