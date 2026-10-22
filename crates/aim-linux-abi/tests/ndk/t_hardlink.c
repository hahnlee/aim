// fs.protected_hardlinks (fs/namei.c may_linkat): with it, a caller links
// only a file it owns or a safe source, a regular file that is not setuid
// or executable setgid and that it can read and write. Run as
// `t_hardlink DIR VALUE` with /proc/sys/fs/protected_hardlinks holding
// VALUE.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static char base[96];
static int protected_links;

static void make(const char *name, uid_t uid, mode_t mode) {
  char path[128];
  snprintf(path, sizeof path, "%s/%s", base, name);
  unlink(path);
  int fd = open(path, O_CREAT | O_WRONLY | O_CLOEXEC, 0600);
  if (fd >= 0) close(fd);
  chown(path, uid, uid);
  chmod(path, mode);
}

// Links `name` to `name.l` as uid 10001; the link's errno, 0 for success.
static int link_as_app(const char *name) {
  char from[128], to[128];
  snprintf(from, sizeof from, "%s/%s", base, name);
  snprintf(to, sizeof to, "%s/%s.l", base, name);
  unlink(to);
  pid_t pid = fork();
  if (pid == 0) {
    if (setresgid(10001, 10001, 10001) != 0 || setresuid(10001, 10001, 10001) != 0) _exit(100);
    _exit(link(from, to) == 0 ? 0 : errno);
  }
  int status;
  if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) return -1;
  unlink(to);
  return WEXITSTATUS(status);
}

static void links(void) {
  int refused = protected_links ? EPERM : 0;
  make("own", 10001, 0600);
  CHECK(link_as_app("own") == 0);
  make("shared", 0, 0666);
  CHECK(link_as_app("shared") == 0);
  make("readonly", 0, 0644);
  CHECK(link_as_app("readonly") == refused);
  make("setuid", 0, 04777);
  CHECK(link_as_app("setuid") == refused);
  make("setgid", 0, 02777);
  CHECK(link_as_app("setgid") == refused);
}

int main(int argc, char **argv) {
  if (argc < 3) return 2;
  protected_links = atoi(argv[2]);
  snprintf(base, sizeof base, "%s/hardlink", argv[1]);
  mkdir(base, 0777);
  chmod(base, 0777);
  RUN(links);
  DONE();
}
