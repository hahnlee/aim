// Socket buffer sizes as socket(7) describes them: SO_SNDBUF/SO_RCVBUF are
// capped at wmem_max/rmem_max, doubled, at least the kernel's minimum, and
// reported doubled; the FORCE forms need CAP_NET_ADMIN and skip the cap.
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static int get(int s, int opt) {
  int v = -1;
  socklen_t l = sizeof v;
  return getsockopt(s, SOL_SOCKET, opt, &v, &l) == 0 ? v : -1;
}

static int set(int s, int opt, int v) { return setsockopt(s, SOL_SOCKET, opt, &v, sizeof v); }

static void doubled_and_capped(void) {
  int types[] = {SOCK_STREAM, SOCK_DGRAM};
  for (int i = 0; i < 2; i++) {
    int s = socket(AF_INET, types[i] | SOCK_CLOEXEC, 0);
    CHECK(s >= 0);
    CHECK(set(s, SO_RCVBUF, 65536) == 0);
    CHECK(get(s, SO_RCVBUF) == 131072);
    CHECK(set(s, SO_SNDBUF, 65536) == 0);
    CHECK(get(s, SO_SNDBUF) == 131072);
    // rmem_max and wmem_max (212992), doubled.
    CHECK(set(s, SO_RCVBUF, 1 << 24) == 0);
    CHECK(get(s, SO_RCVBUF) == 425984);
    CHECK(set(s, SO_SNDBUF, -1) == 0);
    CHECK(get(s, SO_SNDBUF) == 425984);
    // The minimums, SOCK_MIN_RCVBUF and SOCK_MIN_SNDBUF.
    CHECK(set(s, SO_RCVBUF, 0) == 0);
    CHECK(get(s, SO_RCVBUF) == 2304);
    CHECK(set(s, SO_SNDBUF, 1) == 0);
    CHECK(get(s, SO_SNDBUF) == 4608);
    close(s);
  }
  int u = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(u >= 0);
  CHECK(set(u, SO_SNDBUF, 100000) == 0);
  CHECK(get(u, SO_SNDBUF) == 200000);
  close(u);
}

static void forced(void) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(s >= 0);
  // Past rmem_max without the cap, as root (CAP_NET_ADMIN).
  CHECK(set(s, SO_RCVBUFFORCE, 1 << 20) == 0);
  CHECK(get(s, SO_RCVBUF) == 2 << 20);
  CHECK(set(s, SO_SNDBUFFORCE, -5) == 0);
  CHECK(get(s, SO_SNDBUF) == 4608);
  pid_t pid = fork();
  if (pid == 0) {
    // Without CAP_NET_ADMIN: EPERM, and the size stays.
    if (setresuid(10000, 10000, 10000) != 0) _exit(2);
    errno = 0;
    int r = set(s, SO_RCVBUFFORCE, 65536);
    _exit(r == -1 && errno == EPERM && get(s, SO_RCVBUF) == 2 << 20 ? 0 : 1);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid);
  CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  close(s);
}

int main(void) {
  RUN(doubled_and_capped);
  RUN(forced);
  DONE();
}
