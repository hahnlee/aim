// FIONREAD on sockets as Linux answers it: AF_UNIX's unix_inq_len (the
// next datagram for SOCK_DGRAM, every queued record for SOCK_SEQPACKET and
// SOCK_STREAM, EINVAL while listening) and ENOTTY on netlink; SO_ACCEPTCONN
// of AF_UNIX sockets.
#include <linux/netlink.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include "check.h"

static int inq(int s) {
  int n = -1;
  return ioctl(s, FIONREAD, &n) == 0 ? n : -1;
}

static char data[64];

static void datagrams(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, sv) == 0);
  CHECK(inq(sv[1]) == 0);
  CHECK(send(sv[0], data, 10, 0) == 10);
  CHECK(send(sv[0], data, 20, 0) == 20);
  CHECK(inq(sv[1]) == 10);
  CHECK(recv(sv[1], data, sizeof data, 0) == 10);
  CHECK(inq(sv[1]) == 20);
  CHECK(recv(sv[1], data, sizeof data, 0) == 20);
  CHECK(inq(sv[1]) == 0);
  close(sv[0]);
  close(sv[1]);
}

static void records(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) == 0);
  CHECK(send(sv[0], data, 10, 0) == 10);
  CHECK(send(sv[0], data, 20, 0) == 20);
  CHECK(inq(sv[1]) == 30);
  CHECK(recv(sv[1], data, sizeof data, 0) == 10);
  CHECK(inq(sv[1]) == 20);
  close(sv[0]);
  close(sv[1]);
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
  CHECK(send(sv[0], data, 10, 0) == 10);
  CHECK(send(sv[0], data, 20, 0) == 20);
  CHECK(inq(sv[1]) == 30);
  close(sv[0]);
  close(sv[1]);
}

static void listening(void) {
  int types[] = {SOCK_SEQPACKET, SOCK_STREAM};
  for (int i = 0; i < 2; i++) {
    int s = socket(AF_UNIX, types[i], 0);
    CHECK(s >= 0);
    struct sockaddr_un a = {.sun_family = AF_UNIX};
    // An abstract name, so nothing is left behind.
    memcpy(a.sun_path + 1, i ? "t_fionread_s" : "t_fionread_q", 12);
    CHECK(bind(s, (struct sockaddr*)&a, sizeof(sa_family_t) + 13) == 0);
    int on = -1;
    socklen_t l = sizeof on;
    CHECK(getsockopt(s, SOL_SOCKET, SO_ACCEPTCONN, &on, &l) == 0 && on == 0);
    CHECK(listen(s, 1) == 0);
    CHECK(getsockopt(s, SOL_SOCKET, SO_ACCEPTCONN, &on, &l) == 0 && on == 1);
    int n;
    errno = 0;
    CHECK(ioctl(s, FIONREAD, &n) == -1 && errno == EINVAL);
    close(s);
  }
}

static void netlink(void) {
  int s = socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE);
  CHECK(s >= 0);
  int n;
  errno = 0;
  CHECK(ioctl(s, FIONREAD, &n) == -1 && errno == ENOTTY);
  close(s);
}

int main(void) {
  RUN(datagrams);
  RUN(records);
  RUN(listening);
  RUN(netlink);
  DONE();
}
