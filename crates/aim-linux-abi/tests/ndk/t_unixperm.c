// Path sockets' permissions as unix(7) describes them: a bound socket file
// belongs to its binder with mode 0777 & ~umask, binding needs write and
// search on the directory, and connecting or sending to a socket needs
// write permission on it.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static const char *dir;

static struct sockaddr_un at(const char *name) {
  struct sockaddr_un a = {.sun_family = AF_UNIX};
  snprintf(a.sun_path, sizeof a.sun_path, "%s/%s", dir, name);
  unlink(a.sun_path);
  return a;
}

static int bound(int type, struct sockaddr_un *a) {
  int s = socket(AF_UNIX, type | SOCK_CLOEXEC, 0);
  if (s < 0 || bind(s, (struct sockaddr *)a, sizeof *a) != 0) return -1;
  if (type == SOCK_STREAM && listen(s, 1) != 0) return -1;
  return s;
}

// Runs `f` as uid 10001 in a child; its exit status.
static int as_app(int (*f)(struct sockaddr_un *), struct sockaddr_un *a) {
  pid_t pid = fork();
  if (pid == 0) {
    if (setresgid(10001, 10001, 10001) != 0 || setresuid(10001, 10001, 10001) != 0) _exit(100);
    _exit(f(a));
  }
  int status;
  if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) return -1;
  return WEXITSTATUS(status);
}

static int bind_own(struct sockaddr_un *a) {
  umask(077);
  int s = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  if (s < 0 || bind(s, (struct sockaddr *)a, sizeof *a) != 0) return 1;
  struct stat st;
  if (stat(a->sun_path, &st) != 0) return 2;
  return st.st_uid == 10001 && st.st_gid == 10001 && S_ISSOCK(st.st_mode) &&
                 (st.st_mode & 07777) == 0700
             ? 0
             : 3;
}

static void bound_socket_is_its_binders(void) {
  umask(022);
  struct sockaddr_un a = at("root.sock");
  int s = bound(SOCK_STREAM, &a);
  CHECK(s >= 0);
  struct stat st;
  CHECK(stat(a.sun_path, &st) == 0);
  CHECK(S_ISSOCK(st.st_mode));
  CHECK(st.st_uid == 0 && st.st_gid == 0);
  CHECK((st.st_mode & 07777) == 0755);
  close(s);
  unlink(a.sun_path);
  char sub[96];
  snprintf(sub, sizeof sub, "%s/unixown", dir);
  rmdir(sub);
  CHECK(mkdir(sub, 0777) == 0);
  CHECK(chmod(sub, 0777) == 0);
  struct sockaddr_un b = {.sun_family = AF_UNIX};
  snprintf(b.sun_path, sizeof b.sun_path, "%s/app.sock", sub);
  CHECK(as_app(bind_own, &b) == 0);
  unlink(b.sun_path);
  rmdir(sub);
}

static int connect_errno(struct sockaddr_un *a) {
  int s = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  errno = 0;
  return connect(s, (struct sockaddr *)a, sizeof *a) == 0 ? 0 : errno;
}

static int sendto_errno(struct sockaddr_un *a) {
  int s = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  errno = 0;
  return sendto(s, "x", 1, 0, (struct sockaddr *)a, sizeof *a) == 1 ? 0 : errno;
}

static void connect_and_send_need_write(void) {
  struct sockaddr_un a = at("stream.sock");
  int s = bound(SOCK_STREAM, &a);
  CHECK(s >= 0);
  CHECK(chmod(a.sun_path, 0755) == 0);
  CHECK(as_app(connect_errno, &a) == EACCES);
  CHECK(chmod(a.sun_path, 0757) == 0);
  CHECK(as_app(connect_errno, &a) == 0);
  close(s);
  unlink(a.sun_path);
  struct sockaddr_un d = at("dgram.sock");
  int t = bound(SOCK_DGRAM, &d);
  CHECK(t >= 0);
  CHECK(chmod(d.sun_path, 0700) == 0);
  CHECK(as_app(sendto_errno, &d) == EACCES);
  CHECK(chmod(d.sun_path, 0702) == 0);
  CHECK(as_app(sendto_errno, &d) == 0);
  close(t);
  unlink(d.sun_path);
}

static int bind_errno(struct sockaddr_un *a) {
  int s = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  errno = 0;
  return bind(s, (struct sockaddr *)a, sizeof *a) == 0 ? 0 : errno;
}

static void bind_needs_directory_write(void) {
  char sub[108];
  snprintf(sub, sizeof sub, "%s/unixperm", dir);
  rmdir(sub);
  CHECK(mkdir(sub, 0755) == 0);
  struct sockaddr_un a = {.sun_family = AF_UNIX};
  snprintf(a.sun_path, sizeof a.sun_path, "%s/s", sub);
  CHECK(as_app(bind_errno, &a) == EACCES);
  CHECK(chmod(sub, 0777) == 0);
  CHECK(as_app(bind_errno, &a) == 0);
  unlink(a.sun_path);
  rmdir(sub);
}

int main(int argc, char **argv) {
  dir = argc > 1 ? argv[1] : "/data/local/tmp";
  RUN(bound_socket_is_its_binders);
  RUN(connect_and_send_need_write);
  RUN(bind_needs_directory_write);
  DONE();
}
