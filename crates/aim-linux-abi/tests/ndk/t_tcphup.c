// A TCP connection's shutdowns as tcp_poll reports them through poll(2)
// and epoll: a receive shutdown (the peer's FIN, or our SHUT_RD) is IN |
// RDHUP and the socket stays writable; our SHUT_WR is OUT; HUP only once
// both directions are shut.
#include <arpa/inet.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>

#include "check.h"

static int pair(int* a, int* b) {
  int l = socket(AF_INET, SOCK_STREAM, 0);
  struct sockaddr_in s = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  socklen_t n = sizeof s;
  if (l < 0 || bind(l, (struct sockaddr*)&s, sizeof s) || getsockname(l, (struct sockaddr*)&s, &n) ||
      listen(l, 1)) {
    return -1;
  }
  *a = socket(AF_INET, SOCK_STREAM, 0);
  if (connect(*a, (struct sockaddr*)&s, sizeof s)) {
    return -1;
  }
  *b = accept(l, NULL, NULL);
  close(l);
  return *b < 0 ? -1 : 0;
}

static short polled(int fd) {
  struct pollfd p = {fd, POLLIN | POLLOUT | POLLRDHUP, 0};
  return poll(&p, 1, 1000) == 1 ? p.revents : 0;
}

static unsigned epolled(int fd) {
  int ep = epoll_create1(0);
  struct epoll_event e = {.events = EPOLLIN | EPOLLOUT | EPOLLRDHUP, .data.fd = fd};
  epoll_ctl(ep, EPOLL_CTL_ADD, fd, &e);
  struct epoll_event out[4] = {0};
  int n = epoll_wait(ep, out, 4, 1000);
  close(ep);
  return n == 1 ? out[0].events : 0;
}

static void peer_fin(void) {
  int a, b;
  CHECK(pair(&a, &b) == 0);
  CHECK(write(a, "x", 1) == 1 && shutdown(a, SHUT_WR) == 0);
  usleep(50000);
  short p = polled(b);
  CHECK((p & (POLLIN | POLLRDHUP | POLLOUT)) == (POLLIN | POLLRDHUP | POLLOUT));
  CHECK((p & POLLHUP) == 0);
  unsigned e = epolled(b);
  CHECK((e & (EPOLLIN | EPOLLRDHUP | EPOLLOUT)) == (EPOLLIN | EPOLLRDHUP | EPOLLOUT));
  CHECK((e & EPOLLHUP) == 0);
  // The queued byte drains, then EOF; the reply still goes through.
  char c;
  CHECK(read(b, &c, 1) == 1 && read(b, &c, 1) == 0);
  CHECK(write(b, "y", 1) == 1);
  CHECK(read(a, &c, 1) == 1 && c == 'y');
  close(a);
  close(b);
}

static void own_shut_wr(void) {
  int a, b;
  CHECK(pair(&a, &b) == 0);
  CHECK(shutdown(b, SHUT_WR) == 0);
  usleep(50000);
  short p = polled(b);
  CHECK((p & POLLOUT) && !(p & (POLLHUP | POLLIN | POLLRDHUP)));
  unsigned e = epolled(b);
  CHECK((e & EPOLLOUT) && !(e & (EPOLLHUP | EPOLLIN | EPOLLRDHUP)));
  close(a);
  close(b);
}

static void both(void) {
  int a, b;
  CHECK(pair(&a, &b) == 0);
  CHECK(shutdown(a, SHUT_WR) == 0 && shutdown(b, SHUT_WR) == 0);
  usleep(50000);
  short p = polled(b);
  CHECK((p & (POLLHUP | POLLIN | POLLRDHUP)) == (POLLHUP | POLLIN | POLLRDHUP));
  unsigned e = epolled(b);
  CHECK((e & (EPOLLHUP | EPOLLIN | EPOLLRDHUP)) == (EPOLLHUP | EPOLLIN | EPOLLRDHUP));
  close(a);
  close(b);
}

int main(void) {
  RUN(peer_fin);
  RUN(own_shut_wr);
  RUN(both);
  DONE();
}
