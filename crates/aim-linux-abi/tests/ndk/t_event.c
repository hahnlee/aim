// epoll, poll/ppoll/pselect, eventfd and timerfd.
#include <fcntl.h>
#include <poll.h>
#include <net/if.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/ioctl.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

static int64_t now_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

static void eventfd_counter(void) {
  int fd = eventfd(3, EFD_NONBLOCK | EFD_CLOEXEC);
  CHECK(fd >= 0);
  CHECK((fcntl(fd, F_GETFD) & FD_CLOEXEC) != 0);
  uint64_t v = 4;
  CHECK(write(fd, &v, 8) == 8);
  v = 0;
  CHECK(read(fd, &v, 8) == 8 && v == 7);
  CHECK(read(fd, &v, 8) == -1 && errno == EAGAIN);
  CHECK(read(fd, &v, 4) == -1 && errno == EINVAL);
  v = UINT64_MAX;
  CHECK(write(fd, &v, 8) == -1 && errno == EINVAL);
  // Many writes without a read keep one total.
  for (int i = 0; i < 5000; i++) {
    v = 1;
    CHECK(write(fd, &v, 8) == 8);
  }
  CHECK(read(fd, &v, 8) == 8 && v == 5000);
  close(fd);
}

static void eventfd_semaphore(void) {
  int fd = eventfd(2, EFD_SEMAPHORE | EFD_NONBLOCK);
  CHECK(fd >= 0);
  uint64_t v;
  CHECK(read(fd, &v, 8) == 8 && v == 1);
  CHECK(read(fd, &v, 8) == 8 && v == 1);
  CHECK(read(fd, &v, 8) == -1 && errno == EAGAIN);
  close(fd);
}

static void eventfd_blocking_wakeup(void) {
  int fd = eventfd(0, 0);
  CHECK(fd >= 0);
  FORK_OR_SKIP(p);
  if (p == 0) {
    usleep(50000);
    uint64_t v = 9;
    write(fd, &v, 8);
    _exit(0);
  }
  uint64_t v = 0;
  CHECK(read(fd, &v, 8) == 8 && v == 9);  // shared across fork
  int st;
  waitpid(p, &st, 0);
  close(fd);
}

static void epoll_level_edge_oneshot(void) {
  int ep = epoll_create1(EPOLL_CLOEXEC);
  CHECK(ep >= 0);
  int efd = eventfd(0, EFD_NONBLOCK);
  struct epoll_event ev = {.events = EPOLLIN, .data.u64 = 0x1122334455667788ull};
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, efd, &ev) == 0);
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, efd, &ev) == -1 && errno == EEXIST);
  struct epoll_event out[4];
  CHECK(epoll_wait(ep, out, 4, 0) == 0);
  uint64_t v = 1;
  write(efd, &v, 8);
  // Level-triggered: reported until drained.
  CHECK(epoll_wait(ep, out, 4, 0) == 1 && out[0].data.u64 == 0x1122334455667788ull &&
        out[0].events == EPOLLIN);
  CHECK(epoll_wait(ep, out, 4, 0) == 1);
  // Edge-triggered: once per change.
  ev.events = EPOLLIN | EPOLLET;
  CHECK(epoll_ctl(ep, EPOLL_CTL_MOD, efd, &ev) == 0);
  CHECK(epoll_wait(ep, out, 4, 0) == 1);
  CHECK(epoll_wait(ep, out, 4, 0) == 0);
  write(efd, &v, 8);
  CHECK(epoll_wait(ep, out, 4, 0) == 1);
  // One-shot: once until re-armed.
  ev.events = EPOLLIN | EPOLLONESHOT;
  CHECK(epoll_ctl(ep, EPOLL_CTL_MOD, efd, &ev) == 0);
  CHECK(epoll_wait(ep, out, 4, 0) == 1);
  CHECK(epoll_wait(ep, out, 4, 0) == 0);
  CHECK(epoll_ctl(ep, EPOLL_CTL_MOD, efd, &ev) == 0);
  CHECK(epoll_wait(ep, out, 4, 0) == 1);
  CHECK(epoll_ctl(ep, EPOLL_CTL_DEL, efd, NULL) == 0);
  CHECK(epoll_ctl(ep, EPOLL_CTL_DEL, efd, NULL) == -1 && errno == ENOENT);
  // Regular files are refused, as on Linux.
  int reg = open("/system/build.prop", O_RDONLY);
  CHECK(reg >= 0);
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, reg, &ev) == -1 && errno == EPERM);
  close(reg);
  // A closed and reused fd number can be added again.
  close(efd);
  efd = eventfd(0, 0);
  ev.events = EPOLLIN;
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, efd, &ev) == 0);
  close(efd);
  close(ep);
}

static void epoll_hup_and_out(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
  int ep = epoll_create1(0);
  struct epoll_event ev = {.events = EPOLLIN | EPOLLOUT | EPOLLRDHUP, .data.fd = sv[0]};
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, sv[0], &ev) == 0);
  struct epoll_event out[2];
  CHECK(epoll_wait(ep, out, 2, 0) == 1 && out[0].events == EPOLLOUT);
  close(sv[1]);
  CHECK(epoll_wait(ep, out, 2, 100) == 1);
  CHECK((out[0].events & (EPOLLHUP | EPOLLRDHUP | EPOLLIN)) == (EPOLLHUP | EPOLLRDHUP | EPOLLIN));
  close(sv[0]);
  close(ep);
}

static void epoll_nested_and_poll(void) {
  int inner = epoll_create1(0), outer = epoll_create1(0);
  int efd = eventfd(0, EFD_NONBLOCK);
  struct epoll_event ev = {.events = EPOLLIN, .data.u32 = 7};
  CHECK(epoll_ctl(inner, EPOLL_CTL_ADD, efd, &ev) == 0);
  ev.data.u32 = 8;
  CHECK(epoll_ctl(outer, EPOLL_CTL_ADD, inner, &ev) == 0);
  struct epoll_event out[2];
  CHECK(epoll_wait(outer, out, 2, 0) == 0);
  struct pollfd p = {.fd = inner, .events = POLLIN};
  CHECK(poll(&p, 1, 0) == 0);
  uint64_t v = 1;
  write(efd, &v, 8);
  CHECK(epoll_wait(outer, out, 2, 100) == 1 && out[0].data.u32 == 8);
  CHECK(poll(&p, 1, 0) == 1 && (p.revents & POLLIN));
  CHECK(epoll_wait(inner, out, 2, 0) == 1 && out[0].data.u32 == 7);
  close(efd);
  close(inner);
  close(outer);
}

static void epoll_survives_fork(void) {
  int ep = epoll_create1(0);
  int efd = eventfd(0, EFD_NONBLOCK);
  struct epoll_event ev = {.events = EPOLLIN, .data.u32 = 5};
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, efd, &ev) == 0);
  FORK_OR_SKIP(p);
  if (p == 0) {
    uint64_t v = 1;
    write(efd, &v, 8);
    struct epoll_event out;
    int n = epoll_wait(ep, &out, 1, 1000);
    _exit(n == 1 && out.data.u32 == 5 ? 0 : 1);
  }
  int st = 0;
  CHECK(waitpid(p, &st, 0) == p);
  CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0);
  close(efd);
  close(ep);
}

static void epoll_timeout(void) {
  int ep = epoll_create1(0);
  struct epoll_event out;
  int64_t t0 = now_ms();
  CHECK(epoll_wait(ep, &out, 1, 50) == 0);
  int64_t dt = now_ms() - t0;
  CHECK(dt >= 45 && dt < 1000);
  CHECK(epoll_wait(ep, &out, 0, 0) == -1 && errno == EINVAL);
  close(ep);
}

static void poll_ppoll_pselect(void) {
  int pfd[2];
  CHECK(pipe2(pfd, O_CLOEXEC) == 0);
  struct pollfd p[2] = {{.fd = pfd[0], .events = POLLIN}, {.fd = pfd[1], .events = POLLOUT}};
  CHECK(poll(p, 2, 0) == 1 && p[1].revents == POLLOUT && p[0].revents == 0);
  write(pfd[1], "x", 1);
  struct timespec ts = {0, 1000000};
  CHECK(ppoll(p, 2, &ts, NULL) == 2 && (p[0].revents & POLLIN));
  fd_set r, w;
  FD_ZERO(&r);
  FD_ZERO(&w);
  FD_SET(pfd[0], &r);
  FD_SET(pfd[1], &w);
  struct timespec t2 = {1, 0};
  CHECK(pselect(pfd[1] + 1, &r, &w, NULL, &t2, NULL) == 2);
  CHECK(FD_ISSET(pfd[0], &r) && FD_ISSET(pfd[1], &w));
  close(pfd[1]);
  char c;
  read(pfd[0], &c, 1);
  p[0].events = POLLIN;
  CHECK(poll(p, 1, 100) == 1 && (p[0].revents & POLLHUP));
  close(pfd[0]);
  struct pollfd bad = {.fd = 999, .events = POLLIN};
  CHECK(poll(&bad, 1, 0) == 1 && bad.revents == POLLNVAL);
}

static void timerfd_relative_and_periodic(void) {
  int t = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
  CHECK(t >= 0);
  uint64_t n;
  CHECK(read(t, &n, 8) == -1 && errno == EAGAIN);
  struct itimerspec its = {.it_value = {0, 20000000}, .it_interval = {0, 10000000}};
  CHECK(timerfd_settime(t, 0, &its, NULL) == 0);
  struct itimerspec cur;
  CHECK(timerfd_gettime(t, &cur) == 0 && cur.it_interval.tv_nsec == 10000000);
  CHECK(cur.it_value.tv_sec == 0 && cur.it_value.tv_nsec > 0);
  struct pollfd p = {.fd = t, .events = POLLIN};
  CHECK(poll(&p, 1, 1000) == 1);
  usleep(50000);
  CHECK(read(t, &n, 8) == 8 && n >= 4);
  // Disarm.
  memset(&its, 0, sizeof its);
  CHECK(timerfd_settime(t, 0, &its, NULL) == 0);
  CHECK(read(t, &n, 8) == -1 && errno == EAGAIN);
  close(t);
}

static void timerfd_abstime_and_epoll(void) {
  int t = timerfd_create(CLOCK_REALTIME, 0);
  CHECK(t >= 0);
  struct timespec now;
  clock_gettime(CLOCK_REALTIME, &now);
  struct itimerspec its = {.it_value = now};
  its.it_value.tv_nsec += 30000000;
  if (its.it_value.tv_nsec >= 1000000000) {
    its.it_value.tv_sec++;
    its.it_value.tv_nsec -= 1000000000;
  }
  CHECK(timerfd_settime(t, TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET, &its, NULL) == 0);
  int ep = epoll_create1(0);
  struct epoll_event ev = {.events = EPOLLIN, .data.fd = t};
  CHECK(epoll_ctl(ep, EPOLL_CTL_ADD, t, &ev) == 0);
  int64_t t0 = now_ms();
  CHECK(epoll_wait(ep, &ev, 1, 2000) == 1 && ev.data.fd == t);
  CHECK(now_ms() - t0 >= 20);
  uint64_t n = 0;
  CHECK(read(t, &n, 8) == 8 && n == 1);  // blocking read, already expired
  int b = timerfd_create(CLOCK_BOOTTIME, 0);
  CHECK(b >= 0);
  close(b);
  CHECK(timerfd_create(12345, 0) == -1 && errno == EINVAL);
  close(ep);
  close(t);
}

// Every socket call on a file that is not a socket fails with ENOTSOCK
// (socket(7)); the interface ioctls are not its ioctls.
static int socket_calls_fail(int fd) {
  char b[8] = {0};
  int v;
  socklen_t n = sizeof(v);
  struct sockaddr_storage sa;
  socklen_t sl = sizeof(sa);
  struct iovec io = {b, sizeof(b)};
  struct msghdr m = {.msg_iov = &io, .msg_iovlen = 1};
  struct mmsghdr mm = {.msg_hdr = m};
  struct ifreq ifr = {0};
#define NOTSOCK(call) ((call) == -1 && errno == ENOTSOCK)
  return NOTSOCK(send(fd, b, 1, MSG_DONTWAIT)) && NOTSOCK(recv(fd, b, 1, MSG_DONTWAIT)) &&
         NOTSOCK(sendto(fd, b, 1, 0, NULL, 0)) && NOTSOCK(recvfrom(fd, b, 1, 0, NULL, NULL)) &&
         NOTSOCK(sendmsg(fd, &m, 0)) && NOTSOCK(recvmsg(fd, &m, MSG_DONTWAIT)) &&
         NOTSOCK(sendmmsg(fd, &mm, 1, 0)) && NOTSOCK(recvmmsg(fd, &mm, 1, MSG_DONTWAIT, NULL)) &&
         NOTSOCK(getsockopt(fd, SOL_SOCKET, SO_TYPE, &v, &n)) &&
         NOTSOCK(setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &v, sizeof(v))) &&
         NOTSOCK(getsockname(fd, (struct sockaddr*)&sa, &sl)) &&
         NOTSOCK(getpeername(fd, (struct sockaddr*)&sa, &sl)) && NOTSOCK(shutdown(fd, SHUT_RD)) &&
         NOTSOCK(listen(fd, 1)) && NOTSOCK(accept4(fd, NULL, NULL, 0)) &&
         NOTSOCK(bind(fd, (struct sockaddr*)&sa, sizeof(struct sockaddr))) &&
         NOTSOCK(connect(fd, (struct sockaddr*)&sa, sizeof(struct sockaddr))) &&
         ioctl(fd, SIOCGIFINDEX, &ifr) == -1 && errno == ENOTTY;
#undef NOTSOCK
}

static void not_sockets(void) {
  int e = eventfd(0, EFD_NONBLOCK);
  int t = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK);
  int p[2];
  CHECK(e >= 0 && t >= 0 && pipe(p) == 0);
  CHECK(socket_calls_fail(e));
  CHECK(socket_calls_fail(t));
  CHECK(socket_calls_fail(p[0]));
  // They still work as what they are.
  uint64_t v = 1;
  CHECK(write(e, &v, sizeof(v)) == sizeof(v) && read(e, &v, sizeof(v)) == sizeof(v) && v == 1);
  close(e);
  close(t);
  close(p[0]);
  close(p[1]);
}

int main(void) {
  RUN(eventfd_counter);
  RUN(eventfd_semaphore);
  RUN(eventfd_blocking_wakeup);
  RUN(epoll_level_edge_oneshot);
  RUN(epoll_hup_and_out);
  RUN(epoll_nested_and_poll);
  RUN(epoll_survives_fork);
  RUN(epoll_timeout);
  RUN(poll_ppoll_pselect);
  RUN(timerfd_relative_and_periodic);
  RUN(timerfd_abstime_and_epoll);
  RUN(not_sockets);
  DONE();
}
