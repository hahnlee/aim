// Sockets created by guest-init and inherited at fixed fds, as logd gets
// them: fd 3 is /dev/socket/logdr (seqpacket, created as a host stream
// socket), fd 4 is /dev/socket/logdw (dgram+passcred).
#include <stddef.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include "check.h"

static void seqpacket_listener(void) {
  int type = 0;
  socklen_t tl = sizeof type;
  CHECK(getsockopt(3, SOL_SOCKET, SO_TYPE, &type, &tl) == 0 && type == SOCK_SEQPACKET);
  struct sockaddr_un a;
  socklen_t al = sizeof a;
  CHECK(getsockname(3, (struct sockaddr*)&a, &al) == 0 && strcmp(a.sun_path, "/dev/socket/logdr") == 0);
  int c = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0);
  CHECK(connect(c, (struct sockaddr*)&a, al) == 0);
  int d = accept4(3, NULL, NULL, SOCK_CLOEXEC);
  CHECK(d >= 0);
  CHECK(write(c, "dumpAndClose", 12) == 12 && write(c, "x", 1) == 1);
  char buf[64];
  CHECK(read(d, buf, sizeof buf) == 12 && read(d, buf, sizeof buf) == 1);
  close(c);
  close(d);
}

static void dgram_passcred(void) {
  struct sockaddr_un a = {.sun_family = AF_UNIX, .sun_path = "/dev/socket/logdw"};
  int w = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
  CHECK(connect(w, (struct sockaddr*)&a, sizeof a) == 0);
  CHECK(write(w, "log", 3) == 3);
  char buf[16];
  union {
    struct cmsghdr h;
    char b[CMSG_SPACE(sizeof(struct ucred))];
  } ctl;
  struct iovec iov = {buf, sizeof buf};
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &ctl, .msg_controllen = sizeof ctl};
  // The linker may have logged a warning to logdw first; find ours.
  ssize_t n = -1;
  for (int i = 0; i < 8 && n != 3; i++) {
    m.msg_controllen = sizeof ctl;
    n = recvmsg(4, &m, 0);
  }
  CHECK(n == 3 && memcmp(buf, "log", 3) == 0);
  struct cmsghdr* c = CMSG_FIRSTHDR(&m);
  CHECK(c && c->cmsg_type == SCM_CREDENTIALS && ((struct ucred*)CMSG_DATA(c))->pid == getpid());
  close(w);
}

int main(void) {
  RUN(seqpacket_listener);
  RUN(dgram_passcred);
  DONE();
}
