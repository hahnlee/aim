// Boundary contract test, not a replacement for runtime/app acceptance.
#include "filesystem/process_authority.h"
#include "darwin_provider_owners.h"
#include <cassert>
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

namespace {
int admitted = -1;
bool borrow = false;
int released = 0;
}
namespace darwin_art::providers {
void release_filesystem() { ++released; }
bool acquire_filesystem(int fd, std::string*) {
  admitted = fd;
  if (fd < 0) return borrow;
  struct stat st{};
  assert(fstat(fd, &st) == 0 && S_ISDIR(st.st_mode));
  assert(fcntl(fd, F_GETFD) & FD_CLOEXEC);
  return true;
}
}
int main() {
  std::string error;
  using darwin_art::filesystem::AcquireProcessAuthority;
  assert(!AcquireProcessAuthority(nullptr, &error) && admitted == -1);
  assert(!AcquireProcessAuthority("relative", &error) && admitted == -1);
  assert(!AcquireProcessAuthority("/darwin-art-no-such-root", &error));
  assert(!AcquireProcessAuthority("/dev/null", &error));
  assert(AcquireProcessAuthority("/", &error));
  assert(fcntl(admitted, F_GETFD) == -1 && errno == EBADF);
  borrow = true;
  assert(AcquireProcessAuthority(nullptr, &error) && admitted == -1);
  using darwin_art::filesystem::ProcessAuthorityLease;
  {
    ProcessAuthorityLease lease;
    assert(lease.Acquire(nullptr, &error));
    assert(!lease.Acquire(nullptr, &error));
  }
  assert(released == 1);
  {
    ProcessAuthorityLease lease;
    assert(lease.Acquire(nullptr, &error));
    lease.Transfer();
  }
  assert(released == 1);
  darwin_art::providers::release_filesystem();
  borrow = false;
  {
    ProcessAuthorityLease lease;
    assert(!lease.Acquire(nullptr, &error));
  }
  assert(released == 2);
}
