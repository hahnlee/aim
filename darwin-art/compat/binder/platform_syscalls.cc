#include "platform_syscalls.h"

#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_ioctl.h"
#include "darwin_art_bionic_vm.h"
#include "../../tools/bionic-process-state-facade/include/darwin_art_bionic_process_state.h"

#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <sys/mman.h>
#include <unistd.h>

extern "C" int32_t darwin_art_bionic_errno_load(void);

namespace {

void PublishAndroidErrno() { errno = darwin_art_bionic_errno_load(); }

int AndroidOpenFlags(int flags) {
  int translated = flags & O_ACCMODE;
#define MAP_FLAG(host_flag, android_flag)                                      \
  do {                                                                         \
    if ((flags & (host_flag)) != 0)                                            \
      translated |= (android_flag);                                            \
  } while (0)
  MAP_FLAG(O_CREAT, 64);
  MAP_FLAG(O_EXCL, 128);
  MAP_FLAG(O_TRUNC, 512);
  MAP_FLAG(O_APPEND, 1024);
  MAP_FLAG(O_NONBLOCK, DARWIN_ART_ANDROID_O_NONBLOCK);
  MAP_FLAG(O_DSYNC, 4096);
  MAP_FLAG(O_SYNC, 1052672);
  MAP_FLAG(O_DIRECTORY, DARWIN_ART_ANDROID_O_DIRECTORY);
  MAP_FLAG(O_NOFOLLOW, DARWIN_ART_ANDROID_O_NOFOLLOW);
#ifdef O_CLOEXEC
  MAP_FLAG(O_CLOEXEC, DARWIN_ART_ANDROID_O_CLOEXEC);
#endif
#undef MAP_FLAG
  return translated;
}

int AndroidMapFlags(int flags) {
  int translated = 0;
  if ((flags & MAP_SHARED) != 0)
    translated |= 0x01;
  if ((flags & MAP_PRIVATE) != 0)
    translated |= 0x02;
  if ((flags & MAP_FIXED) != 0)
    translated |= 0x10;
#ifdef MAP_ANON
  if ((flags & MAP_ANON) != 0)
    translated |= 0x20;
#endif
#ifdef MAP_NORESERVE
  if ((flags & MAP_NORESERVE) != 0)
    translated |= 0x4000;
#endif
  return translated;
}

} // namespace

extern "C" int darwin_art_binder_platform_open(const char *path, int flags,
                                               ...) {
  uint32_t mode = 0;
  if ((flags & O_CREAT) != 0) {
    va_list arguments;
    va_start(arguments, flags);
    mode = static_cast<uint32_t>(va_arg(arguments, int));
    va_end(arguments);
  }
  const int result =
      darwin_art_bionic_open(path, AndroidOpenFlags(flags), mode);
  if (result < 0)
    PublishAndroidErrno();
  return result;
}

extern "C" int darwin_art_binder_platform_access(const char *path, int mode) {
  const int result = darwin_art_bionic_access(path, mode);
  if (result < 0)
    PublishAndroidErrno();
  return result;
}

extern "C" ssize_t darwin_art_binder_platform_read(int fd, void *buffer,
                                                   size_t count) {
  const intptr_t result = darwin_art_bionic_read(fd, buffer, count);
  if (result < 0)
    PublishAndroidErrno();
  return static_cast<ssize_t>(result);
}

extern "C" int darwin_art_binder_platform_close(int fd) {
  const int result = darwin_art_bionic_close(fd);
  if (result < 0)
    PublishAndroidErrno();
  return result;
}

extern "C" int darwin_art_binder_platform_ioctl(int fd, unsigned long request,
                                                void *argument) {
  const int result =
      darwin_art_bionic_ioctl(fd, static_cast<int>(request), argument);
  if (result < 0)
    PublishAndroidErrno();
  return result;
}

extern "C" void *darwin_art_binder_platform_mmap(void *address, size_t length,
                                                 int protection, int flags,
                                                 int fd, off_t offset) {
  void *const result = darwin_art_bionic_mmap(
      address, length, protection, AndroidMapFlags(flags), fd, offset);
  if (result == MAP_FAILED)
    PublishAndroidErrno();
  return result;
}

extern "C" int darwin_art_binder_platform_munmap(void *address, size_t length) {
  const int result = darwin_art_bionic_munmap(address, length);
  if (result < 0)
    PublishAndroidErrno();
  return result;
}

// Android processes see their Android uid from getuid(); IPCThreadState
// seeds a thread's own calling uid from it.
extern "C" uid_t darwin_art_binder_platform_getuid(void) {
  DarwinArtProcessCredentialsOutput credentials{};
  return darwin_art_bionic_process_state_read_credential_ids_core(&credentials) == 0
             ? static_cast<uid_t>(credentials.uid)
             : getuid();
}
