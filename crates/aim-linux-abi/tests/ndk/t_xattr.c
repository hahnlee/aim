// Extended attributes' permission rules as xattr(7) and fs/xattr.c's
// xattr_permission have them: user.* reads and writes need read and write
// permission on the file, and exist only on regular files and directories;
// trusted.* needs CAP_SYS_ADMIN.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/stat.h>
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
  RUN(others_are_refused);
  DONE();
}
