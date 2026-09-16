#ifndef DARWIN_ART_OPENJDK_NIO_FS_REDIRECT_H_
#define DARWIN_ART_OPENJDK_NIO_FS_REDIRECT_H_

// FileDescriptor.fd contains a Darwin ART virtual descriptor, not a Darwin
// kernel descriptor. OpenJDK's upstream FileDispatcherImpl is compiled as
// host C, so route its descriptor operations back through the same Bionic
// filesystem owner used by java.io and Android native code.

#include "darwin_art_bionic_errno.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_socket_broker.h"

#include <errno.h>
#include <dirent.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/uio.h>
#include <unistd.h>

static inline void darwin_art_openjdk_nio_publish_errno(intptr_t result) {
  if (result < 0) {
    // OpenJDK compares errno with the host constants (EINTR/EAGAIN/etc.)
    // after a Bionic-owned operation.  The facade deliberately keeps its
    // Android errno in separate TLS, so translate at this C/OpenJDK boundary
    // instead of exposing Linux numbers to Darwin code.
    switch (darwin_art_bionic_errno_load()) {
      case 2: errno = ENOENT; break;
      case 5: errno = EIO; break;
      case 9: errno = EBADF; break;
      case 11: errno = EAGAIN; break;
      case 12: errno = ENOMEM; break;
      case 13: errno = EACCES; break;
      case 14: errno = EFAULT; break;
      case 17: errno = EEXIST; break;
      case 20: errno = ENOTDIR; break;
      case 21: errno = EISDIR; break;
      case 22: errno = EINVAL; break;
      case 24: errno = EMFILE; break;
      case 25: errno = ENOTTY; break;
      case 27: errno = EFBIG; break;
      case 28: errno = ENOSPC; break;
      case 30: errno = EROFS; break;
      case 32: errno = EPIPE; break;
      case 34: errno = ERANGE; break;
      case 35: errno = EDEADLK; break;
      case 36: errno = ENAMETOOLONG; break;
      case 37: errno = ENOLCK; break;
      case 38: errno = ENOSYS; break;
      case 39: errno = ENOTEMPTY; break;
      case 40: errno = ELOOP; break;
      case 43: errno = EIDRM; break;
      case 60: errno = ENOSTR; break;
      case 61: errno = ENODATA; break;
      case 62: errno = ETIME; break;
      case 63: errno = ENOSR; break;
      case 67: errno = ENOLINK; break;
      case 71: errno = EPROTO; break;
      case 72: errno = EMULTIHOP; break;
      case 74: errno = EBADMSG; break;
      case 75: errno = EOVERFLOW; break;
      case 84: errno = EILSEQ; break;
      case 87: errno = EUSERS; break;
      case 88: errno = ENOTSOCK; break;
      case 89: errno = EDESTADDRREQ; break;
      case 90: errno = EMSGSIZE; break;
      case 91: errno = EPROTOTYPE; break;
      case 92: errno = ENOPROTOOPT; break;
      case 93: errno = EPROTONOSUPPORT; break;
      case 95: errno = ENOTSUP; break;
      case 97: errno = EAFNOSUPPORT; break;
      case 98: errno = EADDRINUSE; break;
      case 99: errno = EADDRNOTAVAIL; break;
      case 100: errno = ENETDOWN; break;
      case 101: errno = ENETUNREACH; break;
      case 102: errno = ENETRESET; break;
      case 103: errno = ECONNABORTED; break;
      case 104: errno = ECONNRESET; break;
      case 105: errno = ENOBUFS; break;
      case 106: errno = EISCONN; break;
      case 107: errno = ENOTCONN; break;
      case 110: errno = ETIMEDOUT; break;
      case 111: errno = ECONNREFUSED; break;
      case 113: errno = EHOSTUNREACH; break;
      case 114: errno = EALREADY; break;
      case 115: errno = EINPROGRESS; break;
      case 116: errno = ESTALE; break;
      case 122: errno = EDQUOT; break;
      case 125: errno = ECANCELED; break;
      default: errno = EIO; break;
    }
  }
}

static inline int darwin_art_openjdk_nio_is_guest_path(const char* path) {
  if (path == NULL || path[0] != '/') return 1;
  // Runtime inputs are immutable capabilities passed as exact paths. This is
  // relocatable with the application bundle and does not turn a workspace
  // prefix (or an application-created lookalike) into host filesystem access.
  const char* cursor = getenv("DARWIN_ART_RUNTIME_HOST_FILES");
  while (cursor != NULL && *cursor != '\0') {
    const char* separator = strchr(cursor, ':');
    const size_t length = separator == NULL ? strlen(cursor)
                                             : (size_t)(separator - cursor);
    if (length == strlen(path) && memcmp(cursor, path, length) == 0) return 0;
    cursor = separator == NULL ? NULL : separator + 1;
  }
  return 1;
}

static inline int darwin_art_openjdk_nio_is_runtime_read(int flags) {
  const int android_access_mode = flags & 3;
  const int android_write_effects = flags & (0x40 | 0x80 | 0x200 | 0x400);
  return android_access_mode == 0 && android_write_effects == 0;
}

static inline int darwin_art_openjdk_nio_runtime_read_host_flags(
    int android_flags, int* host_flags) {
  // Linux/Android bits accepted for an immutable regular-file capability.
  // Translate them explicitly; their numeric values are not Darwin ABI.
  const int android_largefile = 0x8000;
  const int android_directory = 0x10000;
  const int android_nofollow = 0x20000;
  const int android_cloexec = 0x80000;
  const int accepted = android_largefile | android_directory |
                       android_nofollow | android_cloexec;
  if (!darwin_art_openjdk_nio_is_runtime_read(android_flags) ||
      (android_flags & ~accepted) != 0) {
    errno = EOPNOTSUPP;
    return -1;
  }
  int translated = O_RDONLY;
  if ((android_flags & android_directory) != 0) translated |= O_DIRECTORY;
  if ((android_flags & android_nofollow) != 0) translated |= O_NOFOLLOW;
  if ((android_flags & android_cloexec) != 0) translated |= O_CLOEXEC;
  *host_flags = translated;
  return 0;
}

static inline int darwin_art_openjdk_nio_is_virtual_fd(int fd) {
  return fd >= 0 &&
         ((((uint32_t)fd & UINT32_C(0x40000000)) != 0) ||
          darwin_art_bionic_fs_owns_fd_core(fd) != 0);
}

static inline int darwin_art_openjdk_nio_is_central_fd(int fd) {
  return fd >= 0 &&
         (((uint32_t)fd & UINT32_C(0x40000000)) != 0);
}

static inline ssize_t darwin_art_openjdk_nio_read(int fd, void* bytes,
                                                   size_t count) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return read(fd, bytes, count);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    const intptr_t result =
        darwin_art_bionic_socket_broker_read(fd, bytes, count);
    darwin_art_openjdk_nio_publish_errno(result);
    return (ssize_t)result;
  }
  const intptr_t result = darwin_art_bionic_read(fd, bytes, count);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline ssize_t darwin_art_openjdk_nio_write(int fd, const void* bytes,
                                                    size_t count) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return write(fd, bytes, count);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    const intptr_t result =
        darwin_art_bionic_socket_broker_write(fd, bytes, count);
    darwin_art_openjdk_nio_publish_errno(result);
    return (ssize_t)result;
  }
  const intptr_t result = darwin_art_bionic_write(fd, bytes, count);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline ssize_t darwin_art_openjdk_nio_pread(int fd, void* bytes,
                                                    size_t count,
                                                    off_t offset) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return pread(fd, bytes, count, offset);
  const intptr_t result = darwin_art_bionic_pread(fd, bytes, count, offset);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline ssize_t darwin_art_openjdk_nio_pwrite(int fd,
                                                     const void* bytes,
                                                     size_t count,
                                                     off_t offset) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return pwrite(fd, bytes, count, offset);
  const intptr_t result = darwin_art_bionic_pwrite(fd, bytes, count, offset);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline ssize_t darwin_art_openjdk_nio_readv(int fd,
                                                    const struct iovec* iov,
                                                    int count) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return readv(fd, iov, count);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    const intptr_t result = darwin_art_bionic_socket_broker_readv(fd, iov, count);
    darwin_art_openjdk_nio_publish_errno(result);
    return (ssize_t)result;
  }
  const intptr_t result = darwin_art_bionic_readv(fd, iov, count);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline ssize_t darwin_art_openjdk_nio_writev(
    int fd, const struct iovec* iov, int count) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return writev(fd, iov, count);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    const intptr_t result = darwin_art_bionic_socket_broker_writev(fd, iov, count);
    darwin_art_openjdk_nio_publish_errno(result);
    return (ssize_t)result;
  }
  const intptr_t result = darwin_art_bionic_writev(fd, iov, count);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline int darwin_art_openjdk_nio_close(int fd) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return close(fd);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    const int result = darwin_art_bionic_socket_broker_close(fd);
    darwin_art_openjdk_nio_publish_errno(result);
    return result;
  }
  const int result = darwin_art_bionic_close(fd);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_fsync(int fd) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return fsync(fd);
  const int result = darwin_art_bionic_fsync(fd);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_fdatasync(int fd) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return fsync(fd);
  const int result = darwin_art_bionic_fdatasync(fd);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_ftruncate(int fd, off_t length) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return ftruncate(fd, length);
  const int result = darwin_art_bionic_ftruncate(fd, length);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_fstat(int fd, struct stat* status) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return fstat(fd, status);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    const int host_fd = darwin_art_bionic_fd_export_for_scm(fd);
    if (host_fd < 0) {
      darwin_art_openjdk_nio_publish_errno(-1);
      return -1;
    }
    const int result = fstat(host_fd, status);
    const int saved_errno = errno;
    (void)close(host_fd);
    errno = saved_errno;
    return result;
  }
  DarwinArtAndroidStat android_status;
  const int result = darwin_art_bionic_fstat(fd, &android_status);
  darwin_art_openjdk_nio_publish_errno(result);
  if (result != 0) return result;
  memset(status, 0, sizeof(*status));
  status->st_dev = (dev_t)android_status.st_dev;
  status->st_ino = (ino_t)android_status.st_ino;
  status->st_mode = (mode_t)android_status.st_mode;
  status->st_nlink = (nlink_t)android_status.st_nlink;
  status->st_uid = (uid_t)android_status.st_uid;
  status->st_gid = (gid_t)android_status.st_gid;
  status->st_rdev = (dev_t)android_status.st_rdev;
  status->st_size = (off_t)android_status.st_size;
  status->st_blksize = (blksize_t)android_status.st_blksize;
  status->st_blocks = (blkcnt_t)android_status.st_blocks;
  status->st_atimespec.tv_sec = (time_t)android_status.st_atim.tv_sec;
  status->st_atimespec.tv_nsec = android_status.st_atim.tv_nsec;
  status->st_mtimespec.tv_sec = (time_t)android_status.st_mtim.tv_sec;
  status->st_mtimespec.tv_nsec = android_status.st_mtim.tv_nsec;
  status->st_ctimespec.tv_sec = (time_t)android_status.st_ctim.tv_sec;
  status->st_ctimespec.tv_nsec = android_status.st_ctim.tv_nsec;
  status->st_birthtimespec = status->st_ctimespec;
  return 0;
}

static inline int darwin_art_openjdk_nio_ioctl(int fd, unsigned long request,
                                                void* argument) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return ioctl(fd, request, argument);
  if (darwin_art_openjdk_nio_is_central_fd(fd)) {
    // FileInputStream.available0() uses FIONREAD on Darwin. Central guest
    // descriptors are broker-owned capabilities rather than fs-facade slots,
    // so borrow an SCM-safe host descriptor for this synchronous query.
    const int host_fd = darwin_art_bionic_fd_export_for_scm(fd);
    if (host_fd < 0) {
      darwin_art_openjdk_nio_publish_errno(-1);
      return -1;
    }
    const int result = ioctl(host_fd, request, argument);
    const int saved_errno = errno;
    (void)close(host_fd);
    errno = saved_errno;
    return result;
  }
  const int result = darwin_art_bionic_ioctl(fd, (int)request, argument);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_copy_stat(
    int result, const DarwinArtAndroidStat* android_status,
    struct stat* status) {
  darwin_art_openjdk_nio_publish_errno(result);
  if (result != 0) return result;
  memset(status, 0, sizeof(*status));
  status->st_dev = (dev_t)android_status->st_dev;
  status->st_ino = (ino_t)android_status->st_ino;
  status->st_mode = (mode_t)android_status->st_mode;
  status->st_nlink = (nlink_t)android_status->st_nlink;
  status->st_uid = (uid_t)android_status->st_uid;
  status->st_gid = (gid_t)android_status->st_gid;
  status->st_rdev = (dev_t)android_status->st_rdev;
  status->st_size = (off_t)android_status->st_size;
  status->st_blksize = (blksize_t)android_status->st_blksize;
  status->st_blocks = (blkcnt_t)android_status->st_blocks;
  status->st_atimespec.tv_sec = (time_t)android_status->st_atim.tv_sec;
  status->st_atimespec.tv_nsec = android_status->st_atim.tv_nsec;
  status->st_mtimespec.tv_sec = (time_t)android_status->st_mtim.tv_sec;
  status->st_mtimespec.tv_nsec = android_status->st_mtim.tv_nsec;
  status->st_ctimespec.tv_sec = (time_t)android_status->st_ctim.tv_sec;
  status->st_ctimespec.tv_nsec = android_status->st_ctim.tv_nsec;
  status->st_birthtimespec = status->st_ctimespec;
  return 0;
}

static inline int darwin_art_openjdk_nio_stat(const char* path,
                                               struct stat* status) {
  if (!darwin_art_openjdk_nio_is_guest_path(path)) return stat(path, status);
  DarwinArtAndroidStat android_status;
  const int result = darwin_art_bionic_stat(path, &android_status);
  return darwin_art_openjdk_nio_copy_stat(result, &android_status, status);
}

static inline int darwin_art_openjdk_nio_lstat(const char* path,
                                                struct stat* status) {
  if (!darwin_art_openjdk_nio_is_guest_path(path)) return lstat(path, status);
  DarwinArtAndroidStat android_status;
  const int result = darwin_art_bionic_lstat(path, &android_status);
  return darwin_art_openjdk_nio_copy_stat(result, &android_status, status);
}

static inline off_t darwin_art_openjdk_nio_lseek(int fd, off_t offset,
                                                  int whence) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return lseek(fd, offset, whence);
  if (!darwin_art_openjdk_nio_is_central_fd(fd)) {
    const int64_t result = darwin_art_bionic_lseek(fd, offset, whence);
    darwin_art_openjdk_nio_publish_errno(result);
    return (off_t)result;
  }

  // Central-broker descriptors do not belong to the filesystem facade. A
  // duplicate shares the underlying open-file description (and therefore its
  // position), while keeping the host descriptor number out of guest state.
  const int host_fd = darwin_art_bionic_fd_export_for_scm(fd);
  if (host_fd < 0) {
    darwin_art_openjdk_nio_publish_errno(-1);
    return (off_t)-1;
  }
  const off_t result = lseek(host_fd, offset, whence);
  const int saved_errno = errno;
  (void)close(host_fd);
  errno = saved_errno;
  return result;
}

static inline void* darwin_art_openjdk_nio_mmap(void* address, size_t length,
                                                 int protection, int flags,
                                                 int fd, off_t offset) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return mmap(address, length, protection, flags, fd, offset);

  // mmap retains the vnode after this duplicate is closed, so this is a
  // zero-copy bridge from the Android virtual descriptor to Darwin VM.
  const int host_fd = darwin_art_bionic_fd_export_for_scm(fd);
  if (host_fd < 0) {
    darwin_art_openjdk_nio_publish_errno(-1);
    return MAP_FAILED;
  }
  void* const result =
      mmap(address, length, protection, flags, host_fd, offset);
  const int saved_errno = errno;
  (void)close(host_fd);
  errno = saved_errno;
  return result;
}

static inline int darwin_art_openjdk_nio_open(const char* path, int flags,
                                               ...) {
  uint32_t mode = 0;
  // UnixNativeDispatcher_open0 receives Android/Linux flag values, while
  // this translation unit also includes Darwin's fcntl constants. In
  // particular Android O_CREAT is 0x40 (not Darwin O_CREAT 0x200).
  if ((flags & 0x40) != 0) {
    va_list arguments;
    va_start(arguments, flags);
    mode = (uint32_t)va_arg(arguments, int);
    va_end(arguments);
  }
  if (!darwin_art_openjdk_nio_is_guest_path(path)) {
    // Immutable runtime files are host-backed only for read operations.
    int host_flags = 0;
    if (darwin_art_openjdk_nio_runtime_read_host_flags(flags, &host_flags) != 0)
      return -1;
    return open(path, host_flags);
  }
  const int result = darwin_art_bionic_open(path, flags, mode);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

// FileDispatcherImpl's preClose0 is an internal host-JDK operation: its
// O_RDWR|O_CLOEXEC constants are Darwin values and /dev/null is the host
// wakeup endpoint. UnixNativeDispatcher_open0 continues to use the Android
// wrapper above, so these two ABI domains cannot share the open macro.
static inline int darwin_art_openjdk_nio_host_open(const char* path, int flags,
                                                    ...) {
  mode_t mode = 0;
  if ((flags & O_CREAT) != 0) {
    va_list arguments;
    va_start(arguments, flags);
    mode = (mode_t)va_arg(arguments, int);
    va_end(arguments);
  }
  // The source-level open macro is declared below this function, so this call
  // binds directly to Darwin libc without an assembler-name alias.
  return open(path, flags, mode);
}

static inline int darwin_art_openjdk_nio_dup(int fd) {
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd)) return dup(fd);
  const int result = darwin_art_bionic_fs_fcntl_core(fd, 0, 0);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline char* darwin_art_openjdk_nio_getcwd(char* buffer, size_t size) {
  char* const result = darwin_art_bionic_getcwd(buffer, size);
  if (result == NULL) darwin_art_openjdk_nio_publish_errno(-1);
  return result;
}

static inline int darwin_art_openjdk_nio_chmod(const char* path, mode_t mode) {
  const int result = darwin_art_bionic_chmod(path, (uint32_t)mode);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_fchmod(int fd, mode_t mode) {
  const int result = darwin_art_bionic_fchmod(fd, (uint32_t)mode);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_fchown(int fd, uid_t owner,
                                                 gid_t group) {
  const int result = darwin_art_bionic_fchown(fd, (uint32_t)owner,
                                               (uint32_t)group);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline int darwin_art_openjdk_nio_utimes(
    const char* path, const struct timeval times[2]) {
  DarwinArtAndroidTimespec android_times[2];
  for (size_t index = 0; index < 2; ++index) {
    android_times[index].tv_sec = times[index].tv_sec;
    android_times[index].tv_nsec = (int64_t)times[index].tv_usec * 1000;
  }
  const int result = darwin_art_bionic_utimensat(
      DARWIN_ART_ANDROID_AT_FDCWD, path, android_times, 0);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

static inline DIR* darwin_art_openjdk_nio_opendir(const char* path) {
  void* const result = darwin_art_bionic_opendir(path);
  if (result == NULL) darwin_art_openjdk_nio_publish_errno(-1);
  return (DIR*)result;
}

static inline struct dirent* darwin_art_openjdk_nio_readdir(DIR* directory) {
  DarwinArtAndroidDirent* const android_entry =
      darwin_art_bionic_readdir((void*)directory);
  if (android_entry == NULL) return NULL;
  static _Thread_local struct dirent entry;
  memset(&entry, 0, sizeof(entry));
  entry.d_ino = (ino_t)android_entry->d_ino;
  entry.d_type = android_entry->d_type;
  const size_t length = strnlen(android_entry->d_name,
                                sizeof(android_entry->d_name));
  const size_t copied = length < sizeof(entry.d_name) - 1
                            ? length
                            : sizeof(entry.d_name) - 1;
  memcpy(entry.d_name, android_entry->d_name, copied);
  entry.d_name[copied] = '\0';
  entry.d_namlen = (uint16_t)copied;
  return &entry;
}

static inline int darwin_art_openjdk_nio_closedir(DIR* directory) {
  const int result = darwin_art_bionic_closedir((void*)directory);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

#define DARWIN_ART_OPENJDK_NIO_PATH_CALL(name, guest_expression) \
  static inline int darwin_art_openjdk_nio_##name {              \
    const int result = (guest_expression);                        \
    darwin_art_openjdk_nio_publish_errno(result);                 \
    return result;                                                \
  }

DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    mkdir(const char* path, mode_t mode),
    darwin_art_bionic_mkdir(path, (uint32_t)mode))
DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    rmdir(const char* path),
    darwin_art_bionic_unlinkat(DARWIN_ART_ANDROID_AT_FDCWD, path,
                               DARWIN_ART_ANDROID_AT_REMOVEDIR))
DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    link(const char* old_path, const char* new_path),
    darwin_art_bionic_link(old_path, new_path))
DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    unlink(const char* path),
    darwin_art_bionic_unlinkat(DARWIN_ART_ANDROID_AT_FDCWD, path, 0))
DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    rename(const char* old_path, const char* new_path),
    darwin_art_bionic_rename(old_path, new_path))
DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    symlink(const char* target, const char* link_path),
    darwin_art_bionic_symlink(target, link_path))
DARWIN_ART_OPENJDK_NIO_PATH_CALL(
    access(const char* path, int mode), darwin_art_bionic_access(path, mode))

#undef DARWIN_ART_OPENJDK_NIO_PATH_CALL

static inline ssize_t darwin_art_openjdk_nio_readlink(const char* path,
                                                       char* buffer,
                                                       size_t size) {
  if (!darwin_art_openjdk_nio_is_guest_path(path))
    return readlink(path, buffer, size);
  const intptr_t result = darwin_art_bionic_readlink(path, buffer, size);
  darwin_art_openjdk_nio_publish_errno(result);
  return (ssize_t)result;
}

static inline char* darwin_art_openjdk_nio_realpath(const char* path,
                                                     char* resolved) {
  if (!darwin_art_openjdk_nio_is_guest_path(path))
    return realpath(path, resolved);
  char* const result = darwin_art_bionic_realpath(path, resolved);
  if (result == NULL) darwin_art_openjdk_nio_publish_errno(-1);
  return result;
}

static inline int darwin_art_openjdk_nio_statvfs(
    const char* path, struct statvfs* status) {
  if (!darwin_art_openjdk_nio_is_guest_path(path)) return statvfs(path, status);
  DarwinArtAndroidStatvfs android_status;
  const int result = darwin_art_bionic_statvfs(path, &android_status);
  darwin_art_openjdk_nio_publish_errno(result);
  if (result != 0) return result;
  memset(status, 0, sizeof(*status));
  status->f_bsize = (unsigned long)android_status.f_bsize;
  status->f_frsize = (unsigned long)android_status.f_frsize;
  status->f_blocks = (fsblkcnt_t)android_status.f_blocks;
  status->f_bfree = (fsblkcnt_t)android_status.f_bfree;
  status->f_bavail = (fsblkcnt_t)android_status.f_bavail;
  status->f_files = (fsfilcnt_t)android_status.f_files;
  status->f_ffree = (fsfilcnt_t)android_status.f_ffree;
  status->f_favail = (fsfilcnt_t)android_status.f_favail;
  status->f_fsid = (unsigned long)android_status.f_fsid;
  status->f_flag = (unsigned long)android_status.f_flag;
  status->f_namemax = (unsigned long)android_status.f_namemax;
  return 0;
}

static inline long darwin_art_openjdk_nio_pathconf(const char* path,
                                                    int name) {
  if (!darwin_art_openjdk_nio_is_guest_path(path)) return pathconf(path, name);
  const int64_t result = darwin_art_bionic_pathconf(path, name);
  darwin_art_openjdk_nio_publish_errno(result);
  return (long)result;
}

static inline int darwin_art_openjdk_nio_dup2(int old_fd, int new_fd) {
  // preClose0 uses dup2(/dev/null, fd) solely to wake blocking file I/O.
  // Darwin ART interrupts blocking channels through NativeThread signals;
  // preserving the virtual descriptor here lets close0 perform the one
  // authoritative close without exposing a virtual fd to the host kernel.
  if (!darwin_art_openjdk_nio_is_virtual_fd(old_fd) &&
      !darwin_art_openjdk_nio_is_virtual_fd(new_fd))
    return dup2(old_fd, new_fd);
  return new_fd;
}

static inline int darwin_art_openjdk_nio_fcntl(int fd, int command, ...) {
  struct DarwinArtOpenJdkAndroidFlock {
    int16_t type;
    int16_t whence;
    int32_t padding0;
    int64_t start;
    int64_t length;
    int32_t pid;
    int32_t padding1;
  } android_lock = {0};
  va_list arguments;
  va_start(arguments, command);
  struct flock* host_lock = va_arg(arguments, struct flock*);
  va_end(arguments);
  if (host_lock == NULL) {
    errno = EFAULT;
    return -1;
  }
  if (!darwin_art_openjdk_nio_is_virtual_fd(fd))
    return fcntl(fd, command, host_lock);
  android_lock.type = host_lock->l_type == F_RDLCK
                          ? 0
                          : host_lock->l_type == F_WRLCK ? 1 : 2;
  android_lock.whence = host_lock->l_whence;
  android_lock.start = host_lock->l_start;
  android_lock.length = host_lock->l_len;
  const int android_command = command == F_SETLK ? 6 : 7;
  const int result = darwin_art_bionic_fs_fcntl_core(
      fd, android_command, (intptr_t)&android_lock);
  darwin_art_openjdk_nio_publish_errno(result);
  return result;
}

#define read darwin_art_openjdk_nio_read
#define write darwin_art_openjdk_nio_write
#define pread darwin_art_openjdk_nio_pread
#define pwrite darwin_art_openjdk_nio_pwrite
#define readv darwin_art_openjdk_nio_readv
#define writev darwin_art_openjdk_nio_writev
#define close darwin_art_openjdk_nio_close
#define fsync darwin_art_openjdk_nio_fsync
#define ftruncate darwin_art_openjdk_nio_ftruncate
#define fstat darwin_art_openjdk_nio_fstat
#define lseek darwin_art_openjdk_nio_lseek
#define mmap darwin_art_openjdk_nio_mmap
#ifdef DARWIN_ART_OPENJDK_NIO_HOST_OPEN
#define open darwin_art_openjdk_nio_host_open
#else
#define open darwin_art_openjdk_nio_open
#endif
#define dup darwin_art_openjdk_nio_dup
#define getcwd darwin_art_openjdk_nio_getcwd
#define stat(...) darwin_art_openjdk_nio_stat(__VA_ARGS__)
#define lstat(...) darwin_art_openjdk_nio_lstat(__VA_ARGS__)
#define chmod darwin_art_openjdk_nio_chmod
#define fchmod darwin_art_openjdk_nio_fchmod
#define fchown darwin_art_openjdk_nio_fchown
#define utimes darwin_art_openjdk_nio_utimes
#define opendir darwin_art_openjdk_nio_opendir
#define readdir darwin_art_openjdk_nio_readdir
#define closedir darwin_art_openjdk_nio_closedir
#define mkdir darwin_art_openjdk_nio_mkdir
#define rmdir darwin_art_openjdk_nio_rmdir
#define link darwin_art_openjdk_nio_link
#define unlink darwin_art_openjdk_nio_unlink
#define rename darwin_art_openjdk_nio_rename
#define symlink darwin_art_openjdk_nio_symlink
#define readlink darwin_art_openjdk_nio_readlink
#define realpath darwin_art_openjdk_nio_realpath
#define access darwin_art_openjdk_nio_access
#define statvfs(...) darwin_art_openjdk_nio_statvfs(__VA_ARGS__)
#define pathconf darwin_art_openjdk_nio_pathconf
#define dup2 darwin_art_openjdk_nio_dup2
#define fcntl darwin_art_openjdk_nio_fcntl

#endif  // DARWIN_ART_OPENJDK_NIO_FS_REDIRECT_H_
