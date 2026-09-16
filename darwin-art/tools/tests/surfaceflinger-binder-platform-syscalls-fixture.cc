#include <fcntl.h>
#include <stdarg.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

extern "C" int darwin_art_binder_platform_open(const char *path, int flags,
                                               ...) {
  int mode = 0;
  if ((flags & O_CREAT) != 0) {
    va_list arguments;
    va_start(arguments, flags);
    mode = va_arg(arguments, int);
    va_end(arguments);
  }
  return open(path, flags, mode);
}

extern "C" int darwin_art_binder_platform_access(const char *path, int mode) {
  return access(path, mode);
}

extern "C" ssize_t darwin_art_binder_platform_read(int fd, void *buffer,
                                                   size_t count) {
  return read(fd, buffer, count);
}

extern "C" int darwin_art_binder_platform_close(int fd) { return close(fd); }

extern "C" int darwin_art_binder_platform_ioctl(int fd, unsigned long request,
                                                void *argument) {
  return ioctl(fd, request, argument);
}

extern "C" void *darwin_art_binder_platform_mmap(void *address, size_t length,
                                                 int protection, int flags,
                                                 int fd, off_t offset) {
  return mmap(address, length, protection, flags, fd, offset);
}

extern "C" int darwin_art_binder_platform_munmap(void *address, size_t length) {
  return munmap(address, length);
}

// Standalone archive probes use real Darwin descriptors. Production resolves
// these names to the process-wide Bionic guest descriptor namespace.
extern "C" int darwin_art_binder_duplicate_file_descriptor(int fd) {
  return fcntl(fd, F_DUPFD_CLOEXEC, 0);
}

extern "C" int darwin_art_binder_close_file_descriptor(int fd) {
  return close(fd);
}
