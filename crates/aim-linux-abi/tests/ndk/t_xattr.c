// Extended attributes' permission rules as xattr(7) and fs/xattr.c's
// xattr_permission have them: user.* reads and writes need read and write
// permission on the file, and exist only on regular files and directories;
// trusted.* needs CAP_SYS_ADMIN.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <sys/xattr.h>
#include <unistd.h>

#include "check.h"

static char base[256];

static void path(char* out, size_t size, const char* name) {
  snprintf(out, size, "%s/%s", base, name);
}

static void owner_reads_and_writes(void) {
  char file[512];
  path(file, sizeof file, "own");
  int fd = open(file, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  CHECK(fd >= 0);
  CHECK(fsetxattr(fd, "user.k", "v", 1, 0) == 0);
  char v[4] = {0};
  CHECK(getxattr(file, "user.k", v, sizeof v) == 1 && v[0] == 'v');
  CHECK(removexattr(file, "user.k") == 0);
  close(fd);
}

// open(2) O_PATH and Linux v6.6 fs/xattr.c: fdget rejects FMODE_PATH
// before copying names/values or checking inode permissions and namespaces.
static void path_descriptors_are_refused(void) {
  char file[512], dir[512], link[512];
  path(file, sizeof file, "path-file");
  path(dir, sizeof dir, "path-dir");
  path(link, sizeof link, "path-link");
  int control = open(file, O_RDWR | O_CREAT | O_EXCL, 0600);
  CHECK(control >= 0);
  CHECK(fsetxattr(control, "user.k", "v", 1, 0) == 0);
  CHECK(mkdir(dir, 0700) == 0 && symlink(file, link) == 0);
  const char* paths[] = {file, dir, link, "/dev/null"};
  const char* names[] = {"user.k", "user.missing", "security.selinux", "trusted.k"};
  for (size_t i = 0; i < sizeof paths / sizeof paths[0]; i++) {
    int fd = open(paths[i], O_PATH | O_NOFOLLOW);
    CHECK(fd >= 0);
    int copy = dup(fd);
    CHECK(copy >= 0 && close(fd) == 0);
    for (size_t j = 0; j < sizeof names / sizeof names[0]; j++) {
      for (size_t size = 0; size <= 1; size++) {
        char byte = 'w';
        errno = 0;
        CHECK(syscall(SYS_fgetxattr, copy, names[j], &byte, size) == -1 && errno == EBADF);
        errno = 0;
        CHECK(syscall(SYS_fsetxattr, copy, names[j], &byte, size, 0) == -1 && errno == EBADF);
        errno = 0;
        CHECK(syscall(SYS_flistxattr, copy, &byte, size) == -1 && errno == EBADF);
        errno = 0;
        CHECK(syscall(SYS_fremovexattr, copy, names[j]) == -1 && errno == EBADF);
      }
    }
    // Invalid pointers and flags must not override the descriptor error.
    const int calls[] = {SYS_fgetxattr, SYS_fsetxattr, SYS_flistxattr, SYS_fremovexattr};
    for (size_t j = 0; j < sizeof calls / sizeof calls[0]; j++) {
      errno = 0;
      CHECK(syscall(calls[j], copy, 0, 0, 1, -1) == -1 && errno == EBADF);
    }
    CHECK(close(copy) == 0);
  }
  char byte;
  CHECK(fgetxattr(control, "user.k", &byte, 1) == 1 && byte == 'v');
  CHECK(flistxattr(control, NULL, 0) >= (ssize_t)sizeof("user.k"));
  CHECK(getxattr(file, "user.k", &byte, 1) == 1 && byte == 'v');
  CHECK(fremovexattr(control, "user.k") == 0 && close(control) == 0);
  CHECK(unlink(file) == 0 && unlink(link) == 0 && rmdir(dir) == 0);
}

static void others_are_refused(void) {
  char file[512], fifo[512];
  path(file, sizeof file, "theirs");
  path(fifo, sizeof fifo, "fifo");
  int fd = open(file, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  CHECK(fd >= 0);
  CHECK(fsetxattr(fd, "user.k", "v", 1, 0) == 0);
  close(fd);
  unlink(fifo);
  CHECK(mkfifo(fifo, 0666) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (child == 0) {
    if (setresgid(10001, 10001, 10001) != 0) _exit(10);
    if (setresuid(10001, 10001, 10001) != 0) _exit(11);
    char v[4];
    // No read or write permission on the 0600 file.
    errno = 0;
    if (getxattr(file, "user.k", v, sizeof v) != -1 || errno != EACCES) _exit(12);
    errno = 0;
    if (setxattr(file, "user.k", "w", 1, 0) != -1 || errno != EACCES) _exit(13);
    errno = 0;
    if (removexattr(file, "user.k") != -1 || errno != EACCES) _exit(14);
    // user.* is not on a FIFO: reads find none, writes are refused.
    errno = 0;
    if (getxattr(fifo, "user.k", v, sizeof v) != -1 || errno != ENODATA) _exit(15);
    errno = 0;
    if (setxattr(fifo, "user.k", "w", 1, 0) != -1 || errno != EPERM) _exit(16);
    // trusted.* without CAP_SYS_ADMIN.
    errno = 0;
    if (getxattr(file, "trusted.k", v, sizeof v) != -1 || errno != ENODATA) _exit(17);
    errno = 0;
    if (setxattr(file, "trusted.k", "w", 1, 0) != -1 || errno != EPERM) _exit(18);
    _exit(0);
  }
  int status = 0;
  CHECK(waitpid(child, &status, 0) == child);
  if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
    printf("child exit %d\n", WIFEXITED(status) ? WEXITSTATUS(status) : -1);
  }
  CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

int main(int argc, char** argv) {
  snprintf(base, sizeof base, "%s/xattr-%d", argc > 1 ? argv[1] : "/data/local/tmp", getpid());
  mkdir(base, 0755);
  RUN(owner_reads_and_writes);
  RUN(path_descriptors_are_refused);
  RUN(others_are_refused);
  DONE();
}
