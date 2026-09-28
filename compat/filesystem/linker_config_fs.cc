#include "linker_config_fs.h"
#include "aim_bionic_fs.h"
#include <cerrno>
#include <cstring>

namespace aim::filesystem {
int GuestLinkerAccess(const char* path, int mode) {
  const int result = aim_bionic_access(path, mode);
  if (result < 0) RestoreGuestConfigErrno();
  return result;
}
int GuestLinkerStat(const char* path, AimAndroidStat* status) {
  const int result = aim_bionic_stat(path, status);
  if (result < 0) RestoreGuestConfigErrno();
  return result;
}
char* GuestLinkerRealpathBuffer(const char* path, char* output, size_t capacity) {
  if (!output || !capacity) { errno = EINVAL; return nullptr; }
  // Android PATH_MAX is 4096; Darwin's is 1024. Never let the guest ABI write
  // its full result straight into a host-sized parser stack buffer.
  char guest[4096];
  if (!aim_bionic_realpath(path, guest)) {
    RestoreGuestConfigErrno();
    return nullptr;
  }
  const size_t size = std::strlen(guest) + 1;
  if (size > capacity) { errno = ENAMETOOLONG; return nullptr; }
  std::memcpy(output, guest, size);
  return output;
}
}
