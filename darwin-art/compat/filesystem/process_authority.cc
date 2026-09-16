#include "process_authority.h"
#include "../darwin_provider_owners.h"
#include <fcntl.h>
#include <unistd.h>

namespace darwin_art::filesystem {
namespace {
struct RootFd {
  int fd;
  ~RootFd() { if (fd >= 0) close(fd); }
  RootFd(const RootFd&) = delete;
  RootFd& operator=(const RootFd&) = delete;
  explicit RootFd(int value) : fd(value) {}
};
}
bool AcquireProcessAuthority(const char* full_root, std::string* error) {
  RootFd root(full_root != nullptr && full_root[0] == '/'
      ? open(full_root, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW)
      : -1);
  // Existing process authority may be borrowed without a new root. Only the
  // provider owner decides that; no pathname fallback or second installation.
  return providers::acquire_filesystem(root.fd, error);
}
ProcessAuthorityLease::~ProcessAuthorityLease() {
  if (held_) providers::release_filesystem();
}
bool ProcessAuthorityLease::Acquire(const char* full_root, std::string* error) {
  if (held_) {
    *error = "process filesystem lease already acquired";
    return false;
  }
  held_ = AcquireProcessAuthority(full_root, error);
  return held_;
}
void ProcessAuthorityLease::Transfer() { held_ = false; }
}
