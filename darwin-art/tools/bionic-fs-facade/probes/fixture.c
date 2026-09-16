#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/sysmacros.h>
#include <unistd.h>

static int BytesEqual(const char* left, const char* right, size_t count) {
  for (size_t index = 0; index < count; ++index) {
    if (left[index] != right[index]) return 0;
  }
  return 1;
}

static int NameEqual(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

static int AllZero(const uint8_t* bytes, size_t count) {
  for (size_t index = 0; index < count; ++index) {
    if (bytes[index] != 0) return 0;
  }
  return 1;
}

__attribute__((visibility("default"))) int
bionic_fs_fixture_pc_2_symlinks(void) {
  long value = pathconf("/system/etc/payload.txt", _PC_2_SYMLINKS);
  return value >= -1 && value <= INT32_MAX ? (int)value : INT32_MIN;
}

__attribute__((visibility("default"))) int bionic_fs_fixture_statvfs_bsize(void) {
  struct statvfs status;
  return statvfs("/system/etc/payload.txt", &status) == 0 &&
                 status.f_bsize <= INT32_MAX
             ? (int)status.f_bsize
             : -1;
}

__attribute__((visibility("default"))) int bionic_fs_fixture_statvfs_flags(void) {
  struct statvfs status;
  return statvfs("/system/etc/payload.txt", &status) == 0 &&
                 status.f_flag <= INT32_MAX
             ? (int)status.f_flag
             : -1;
}

__attribute__((visibility("default"))) int bionic_fs_fixture_run(void) {
  static const char expected[] = "brokered-data";
  char buffer[sizeof(expected)] = {0};
  int fd = open("/system/etc/payload.txt", O_RDONLY | O_CLOEXEC);
  if (fd < 0) return 1;
  if (fd < 10000) return 14;
  struct stat status;
  if (fstat(fd, &status) != 0 || !S_ISREG(status.st_mode) ||
      status.st_size != (off_t)(sizeof(expected) - 1)) return 2;
  ssize_t count = read(fd, buffer, sizeof(expected) - 1);
  if (count != (ssize_t)(sizeof(expected) - 1) ||
      !BytesEqual(buffer, expected, sizeof(expected) - 1)) return 3;
  if (close(fd) != 0) return 4;

  fd = open("/data/libcxx-copy", O_RDONLY);
  if (fd < 10000 || fstat(fd, &status) != 0 || !S_ISREG(status.st_mode) ||
      (status.st_mode & 0777) != 0644 ||
      status.st_size != (off_t)(sizeof(expected) - 1)) return 101;
  for (size_t index = 0; index < sizeof(buffer); ++index) buffer[index] = 0;
  if (read(fd, buffer, sizeof(expected) - 1) !=
          (ssize_t)(sizeof(expected) - 1) ||
      !BytesEqual(buffer, expected, sizeof(expected) - 1) || close(fd) != 0)
    return 102;
  if (mkdir("/data/guest", 0750) != 0) return 103;
  if (stat("/data/guest", &status) != 0 || !S_ISDIR(status.st_mode) ||
      (status.st_mode & 0777) != 0750)
    return 108;
  errno = 0;
  if (mkdir("/data/guest", 0750) != -1 || errno != EEXIST) return 104;
  fd = open("/data/guest/empty", O_WRONLY | O_CREAT | O_TRUNC, 0600);
  if (fd < 10000 || fstat(fd, &status) != 0 ||
      (status.st_mode & 0777) != 0600 || fchmod(fd, 0640) != 0 ||
      ftruncate(fd, 2) != 0 ||
      fstat(fd, &status) != 0 || status.st_size != 2 || close(fd) != 0)
    return 105;
  fd = open("/data/guest/empty", O_RDONLY);
  buffer[0] = 1;
  buffer[1] = 1;
  if (fd < 10000 || read(fd, buffer, 2) != 2 || buffer[0] != 0 ||
      buffer[1] != 0 || close(fd) != 0)
    return 106;
  errno = 0;
  if (symlink("target", "/data/guest/link") != -1 || errno != EROFS)
    return 107;

  fd = openat(AT_FDCWD, "etc/payload.txt", O_RDONLY);
  if (fd < 0 || close(fd) != 0) return 5;

  errno = 0;
  if (open("/system/etc/missing", O_RDONLY) != -1 || errno != ENOENT) return 6;
  errno = 0;
  if (open("/system/etc/payload.txt", O_RDONLY | O_DIRECT) != -1 ||
      errno != EOPNOTSUPP) return 7;
  errno = 0;
  if (open("/system/etc/payload.txt", O_WRONLY) != -1 || errno != EROFS) return 8;
  errno = 0;
  if (open("/system/../secret", O_RDONLY) != -1 || errno != EACCES) return 9;
  errno = 0;
  if (open("/system/etc/outside-link", O_RDONLY) != -1 || errno != ELOOP) return 10;

  int directory = open("/system/etc", O_RDONLY | O_DIRECTORY);
  if (directory < 0) return 11;
  errno = 0;
  if (openat(directory, "payload.txt", O_RDONLY) != -1 ||
      errno != EOPNOTSUPP) return 12;
  errno = 0;
  if (mkdirat(directory, "new-directory", 0700) != -1 || errno != EROFS)
    return 115;
  errno = 0;
  if (mkdirat(-7, "new-directory", 0700) != -1 || errno != EBADF)
    return 116;
  errno = 0;
  if (mkdirat(-7, "/system/etc/new-directory", 0700) != -1 ||
      errno != EROFS)
    return 117;
  if (close(directory) != 0) return 13;

  if (stat("/system/etc/payload.txt", &status) != 0 ||
      !S_ISREG(status.st_mode) ||
      status.st_size != (off_t)(sizeof(expected) - 1)) return 15;
  if (lstat("/system/etc/payload.txt", &status) != 0 ||
      !S_ISREG(status.st_mode)) return 16;
  errno = 0;
  if (stat("/system/etc/outside-link", &status) != -1 || errno != ELOOP)
    return 17;
  errno = 0;
  if (lstat("/system/etc/outside-link", &status) != -1 ||
      errno != EOPNOTSUPP) return 18;
  errno = 0;
  if (readlink("/system/etc/outside-link", buffer, sizeof(buffer)) != -1 ||
      errno != EOPNOTSUPP) return 19;

  char cwd[64];
  if (getcwd(cwd, sizeof(cwd)) != cwd || !NameEqual(cwd, "/system")) return 20;
  errno = 0;
  if (getcwd(cwd, 4) != NULL || errno != ERANGE) return 21;
  errno = 0;
  if (getcwd(NULL, 0) != NULL || errno != EOPNOTSUPP) return 22;
  if (chdir("etc") != 0 || getcwd(cwd, sizeof(cwd)) != cwd ||
      !NameEqual(cwd, "/system/etc")) return 23;
  if (stat("payload.txt", &status) != 0 || !S_ISREG(status.st_mode)) return 24;
  errno = 0;
  if (chdir("payload.txt") != -1 || errno != ENOTDIR) return 25;
  if (chdir("..") != 0 || getcwd(cwd, sizeof(cwd)) != cwd ||
      !NameEqual(cwd, "/system")) return 26;

  DIR* stream = opendir("etc");
  if (stream == NULL) return 27;
  int saw_payload = 0;
  int saw_link = 0;
  errno = 777;
  for (;;) {
    struct dirent* entry = readdir(stream);
    if (entry == NULL) break;
    if (entry->d_reclen < 24 || entry->d_reclen > sizeof(struct dirent)) return 28;
    if (NameEqual(entry->d_name, "payload.txt")) {
      if (entry->d_type != DT_UNKNOWN && entry->d_type != DT_REG) return 29;
      saw_payload = 1;
    }
    if (NameEqual(entry->d_name, "outside-link")) {
      if (entry->d_type != DT_UNKNOWN && entry->d_type != DT_LNK) return 30;
      saw_link = 1;
    }
  }
  if (errno != 777 || !saw_payload || !saw_link) return 31;
  if (closedir(stream) != 0) return 32;
  int directory_fd = open("etc", O_RDONLY | O_DIRECTORY);
  if (directory_fd < 0) return 64;
  stream = fdopendir(directory_fd);
  if (stream == NULL) return 65;
  if (dirfd(stream) != directory_fd) return 66;
  errno = 0;
  struct stat directory_status;
  if (fstat(directory_fd, &directory_status) != 0 ||
      !S_ISDIR(directory_status.st_mode)) return 66;
  if (fstatat(directory_fd, "", &directory_status, AT_EMPTY_PATH) != 0 ||
      !S_ISDIR(directory_status.st_mode)) return 66;
  saw_payload = 0;
  for (;;) {
    struct dirent* entry = readdir(stream);
    if (entry == NULL) break;
    if (NameEqual(entry->d_name, "payload.txt")) saw_payload = 1;
  }
  if (!saw_payload || closedir(stream) != 0) return 67;
  errno = 0;
  if (close(directory_fd) != -1 || errno != EBADF) return 66;
  int regular_fd = open("etc/payload.txt", O_RDONLY);
  if (regular_fd < 0) return 68;
  errno = 0;
  if (fdopendir(regular_fd) != NULL || errno != ENOTDIR) return 69;
  if (close(regular_fd) != 0) return 70;
  errno = 0;
  if (fdopendir(-7) != NULL || errno != EBADF) return 71;
  errno = 0;
  if (opendir("etc/payload.txt") != NULL || errno != ENOTDIR) return 33;
  errno = 0;
  if (opendir("etc/outside-link") != NULL || errno != ELOOP) return 34;
  errno = 0;
  if (readlink("etc/payload.txt", buffer, sizeof(buffer)) != -1 ||
      errno != EINVAL) return 35;
  errno = 0;
  if (readlink("etc/outside-dir-link/secret", buffer, sizeof(buffer)) != -1 ||
      errno != ENOTDIR) return 36;

  fd = open("/system/etc/payload.txt", O_RDONLY);
  if (fd < 0) return 37;
  errno = 0;
  if (isatty(fd) != 0 || errno != ENOTTY) return 38;
  errno = 0;
  if (isatty(-7) != 0 || errno != EBADF) return 39;

  errno = 991;
  if (pathconf("/system/etc/payload.txt", _PC_NAME_MAX) <= 0 || errno != 991)
    return 40;
  errno = 0;
  if (pathconf("/system/etc/payload.txt", 999) != -1 || errno != EINVAL)
    return 41;

  struct statvfs filesystem;
  if (statvfs("/system/etc/payload.txt", &filesystem) != 0 ||
      filesystem.f_bsize == 0 || filesystem.f_namemax == 0 ||
      (filesystem.f_flag & ST_RDONLY) == 0)
    return 43;
  for (size_t index = 0; index < 6; ++index) {
    if (filesystem.__f_reserved[index] != 0) return 44;
  }

  char resolved[4096];
  if (realpath("/system/etc/../etc/payload.txt", resolved) != resolved ||
      !NameEqual(resolved, "/system/etc/payload.txt"))
    return 45;
  errno = 0;
  if (realpath("/system/etc/payload.txt", NULL) != NULL ||
      errno != EOPNOTSUPP)
    return 46;

  errno = 0;
  if (fchmod(fd, 0600) != -1 || errno != EROFS) return 47;
  errno = 0;
  if (fchmod(-7, 0600) != -1 || errno != EBADF) return 48;
  errno = 0;
  if (ftruncate(fd, 0) != -1 || errno != EROFS) return 49;
  errno = 0;
  if (fchmodat(AT_FDCWD, "/system/etc/payload.txt", 0600, 0) != -1 ||
      errno != EROFS)
    return 50;
  errno = 0;
  if (fchmodat(AT_FDCWD, "/system/etc/payload.txt", 0600, 0x40000000) !=
          -1 ||
      errno != EINVAL)
    return 51;
  errno = 0;
  if (link("/system/etc/payload.txt", "/system/etc/hard-link") != -1 ||
      errno != EROFS)
    return 52;
  errno = 0;
  if (mkdir("/system/etc/new-directory", 0700) != -1 || errno != EROFS)
    return 53;
  errno = 0;
  if (remove("/system/etc/payload.txt") != -1 || errno != EROFS) return 54;
  errno = 0;
  if (rename("/system/etc/payload.txt", "/system/etc/renamed") != -1 ||
      errno != EROFS)
    return 55;
  errno = 0;
  if (symlink("../../outside", "/system/etc/new-link") != -1 ||
      errno != EROFS)
    return 56;
  errno = 0;
  if (truncate("/system/etc/payload.txt", 0) != -1 || errno != EROFS)
    return 57;
  errno = 0;
  if (unlinkat(AT_FDCWD, "/system/etc/payload.txt", 0) != -1 ||
      errno != EROFS)
    return 58;
  errno = 0;
  if (utimensat(AT_FDCWD, "/system/etc/payload.txt", NULL,
                AT_SYMLINK_NOFOLLOW) != -1 ||
      errno != EROFS)
    return 59;
  errno = 0;
  if (utimensat(fd, NULL, NULL, 0) != -1 || errno != EROFS) return 60;
  errno = 0;
  if (mkdir("/outside/new-directory", 0700) != -1 || errno != EACCES)
    return 61;
  if (close(fd) != 0) return 62;

  fd = open("/system/etc/payload.txt", O_RDONLY);
  if (fd < 0 || fstat(fd, &status) != 0 ||
      status.st_size != (off_t)(sizeof(expected) - 1) || close(fd) != 0)
    return 63;

  uint8_t random_first[32] = {0};
  uint8_t random_second[32] = {0};
  if (stat("/dev/random", &status) != 0 || !S_ISCHR(status.st_mode) ||
      status.st_rdev != makedev(1, 8) ||
      lstat("/dev/urandom", &status) != 0 || !S_ISCHR(status.st_mode) ||
      status.st_rdev != makedev(1, 9))
    return 89;
  int random_fd = open("/dev/random", O_RDONLY);
  if (random_fd < 10000) return 72;
  if (fstat(random_fd, &status) != 0 || !S_ISCHR(status.st_mode) ||
      status.st_size != 0 || status.st_rdev != makedev(1, 8))
    return 73;
  if (read(random_fd, random_first, sizeof(random_first)) !=
          (ssize_t)sizeof(random_first) ||
      read(random_fd, random_second, sizeof(random_second)) !=
          (ssize_t)sizeof(random_second) ||
      AllZero(random_first, sizeof(random_first)) ||
      AllZero(random_second, sizeof(random_second)) ||
      BytesEqual((const char*)random_first, (const char*)random_second,
                 sizeof(random_first)))
    return 74;
  if (read(random_fd, NULL, 0) != 0) return 75;
  errno = 0;
  if (isatty(random_fd) != 0 || errno != ENOTTY) return 76;
  errno = 0;
  if (fdopendir(random_fd) != NULL || errno != ENOTDIR) return 77;
  errno = 0;
  if (fchmod(random_fd, 0600) != -1 || errno != EROFS) return 78;
  errno = 0;
  if (ftruncate(random_fd, 0) != -1 || errno != EROFS) return 79;
  if (close(random_fd) != 0) return 80;
  errno = 0;
  if (read(random_fd, random_first, sizeof(random_first)) != -1 ||
      errno != EBADF)
    return 81;

  int urandom_fd = open("/dev/urandom", O_RDONLY);
  if (urandom_fd != random_fd) return 82;
  if (fstat(urandom_fd, &status) != 0 || !S_ISCHR(status.st_mode) ||
      status.st_rdev != makedev(1, 9) ||
      read(urandom_fd, random_first, sizeof(random_first)) !=
          (ssize_t)sizeof(random_first) ||
      AllZero(random_first, sizeof(random_first)))
    return 83;
  if (close(urandom_fd) != 0) return 84;

  int cloexec_random_fd = open("/dev/random", O_RDONLY | O_CLOEXEC);
  if (cloexec_random_fd < 0 || close(cloexec_random_fd) != 0) return 85;
  errno = 0;
  if (open("/dev/random", O_WRONLY) != -1 || errno != EROFS) return 86;
  errno = 0;
  if (open("/dev/random/", O_RDONLY) != -1 || errno != EACCES) return 87;
  errno = 0;
  if (open("../../dev/random", O_RDONLY) != -1 || errno != EACCES) return 90;
  errno = 0;
  if (stat("/dev/random/", &status) != -1 || errno != EACCES) return 91;
  random_fd = openat(-7, "/dev/random", O_RDONLY);
  if (random_fd < 10000 || close(random_fd) != 0) return 88;
  return 42;
}
