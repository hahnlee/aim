// The derived image's lmkd (daemons/lmkd) as ActivityManager meets it:
// started with its socket from "init", it answers the control protocol
// over SOCK_SEQPACKET, and its host-call module reports the Mac's memory.
// argv[1] is the lmkd binary.
#include <arpa/inet.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static const char* lmkd_path;
static const char* sock_path = "/data/local/tmp/lmkd.sock";

// aim_hostcall::memory::Memory.
struct memory {
  uint32_t level, reserved;
  uint64_t total, free, file;
};

static void host_memory(void) {
  struct memory m = {0};
  // The host-call syscall: module 10 (memory), FN_READ.
  CHECK(syscall(0x48430000, 10, 1, &m, sizeof m) == 0);
  CHECK(m.level == 1 || m.level == 2 || m.level == 4);
  CHECK(m.total > (1ull << 30) && m.free > 0 && m.free < m.total && m.file < m.total);
  CHECK(syscall(0x48430000, 10, 1, &m, 8) == -1 && errno == EINVAL);
  CHECK(syscall(0x48430000, 10, 0, NULL, 0) == 1);
}

static void put(int fd, const int32_t* words, int n) {
  int32_t be[16];
  for (int i = 0; i < n; i++) be[i] = htonl(words[i]);
  send(fd, be, n * 4, 0);
}

static int get(int fd, int32_t* words) {
  int32_t be[16];
  ssize_t n = recv(fd, be, sizeof be, 0);
  for (int i = 0; i < n / 4; i++) words[i] = ntohl(be[i]);
  return n / 4;
}

static void protocol(void) {
  int listener = socket(AF_UNIX, SOCK_SEQPACKET, 0);
  CHECK(listener >= 0);
  struct sockaddr_un a = {.sun_family = AF_UNIX};
  strcpy(a.sun_path, sock_path);
  unlink(sock_path);
  CHECK(bind(listener, (struct sockaddr*)&a, sizeof a) == 0);

  // A process for lmkd to know about.
  FORK_OR_SKIP(app);
  if (app == 0) {
    pause();
    _exit(0);
  }
  FORK_OR_SKIP(lmkd);
  if (lmkd == 0) {
    char env[32];
    snprintf(env, sizeof env, "ANDROID_SOCKET_lmkd=%d", listener);
    char* envp[] = {env, NULL};
    char* argv[] = {(char*)lmkd_path, NULL};
    execve(lmkd_path, argv, envp);
    _exit(127);
  }
  close(listener);

  int am = socket(AF_UNIX, SOCK_SEQPACKET, 0);
  int connected = -1;
  for (int i = 0; i < 100 && connected != 0; i++) {
    connected = connect(am, (struct sockaddr*)&a, sizeof a);
    if (connected != 0) usleep(20000);
  }
  CHECK(connected == 0);
  // ProcessList.onLmkdConnect, with minfree levels that never trigger.
  int32_t purge[] = {3};
  int32_t target[] = {0, 1, 0, 1, 100, 1, 200, 1, 250, 1, 900, 1, 950};
  int32_t sub_kill[] = {5, 0}, sub_stat[] = {5, 1};
  put(am, purge, 1);
  put(am, target, 13);
  put(am, sub_kill, 2);
  put(am, sub_stat, 2);
  // The foreground app: never a victim of pressure alone.
  int32_t prio[] = {1, app, 10123, 0, 0};
  put(am, prio, 5);

  int32_t r[16];
  int32_t killcnt[] = {4, 0, 1000};
  put(am, killcnt, 3);
  CHECK(get(am, r) == 2 && r[0] == 4 && r[1] == 0);
  int32_t props[] = {7};
  put(am, props, 1);
  CHECK(get(am, r) == 2 && r[0] == 7 && r[1] == 0);
  int32_t boot[] = {10};
  put(am, boot, 1);
  CHECK(get(am, r) == 2 && r[0] == 10 && r[1] == 0);
  put(am, boot, 1);
  CHECK(get(am, r) == 2 && r[0] == 10 && r[1] == 1);
  int32_t remove[] = {2, app};
  put(am, remove, 2);
  put(am, killcnt, 3);
  CHECK(get(am, r) == 2 && r[1] == 0);

  CHECK(kill(app, 0) == 0);
  kill(app, SIGKILL);
  kill(lmkd, SIGKILL);
  int st;
  CHECK(waitpid(lmkd, &st, 0) == lmkd && WIFSIGNALED(st) && WTERMSIG(st) == SIGKILL);
  waitpid(app, &st, 0);
  close(am);
  unlink(sock_path);
}

int main(int argc, char** argv) {
  if (argc < 2) return 2;
  lmkd_path = argv[1];
  RUN(host_memory);
  RUN(protocol);
  DONE();
}
