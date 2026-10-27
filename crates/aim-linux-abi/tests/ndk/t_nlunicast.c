// netlink(7), Linux netlink_connect/netlink_getsockbyportid/netlink_sendmsg:
// opaque user datagrams, protocol-local port ids, connected peers and errors.
#include <fcntl.h>
#include <linux/netlink.h>
#include <poll.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

static volatile sig_atomic_t alarm_seen;
static void alarm_handler(int sig) { (void)sig; alarm_seen = 1; }

static const char* program;

static int nl_port(int proto, int type, unsigned portid) {
  int fd = socket(AF_NETLINK, type, proto);
  if (fd < 0) return -1;
  struct timeval timeout = {.tv_sec = 2};
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_pid = portid};
  if (setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout) ||
      setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof timeout) ||
      bind(fd, (struct sockaddr*)&a, sizeof a)) {
    close(fd);
    return -1;
  }
  return fd;
}

static int nl(int proto, int type) {
  return nl_port(proto, type, 0);
}

static struct sockaddr_nl local(int fd) {
  struct sockaddr_nl a = {0};
  socklen_t len = sizeof a;
  if (getsockname(fd, (struct sockaddr*)&a, &len) || len != sizeof a) a.nl_pid = 0;
  return a;
}

static int to(int fd, unsigned id) {
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_pid = id};
  return connect(fd, (struct sockaddr*)&a, sizeof a);
}

static ssize_t put(int fd, unsigned id, const void* data, size_t size, int flags) {
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_pid = id};
  return sendto(fd, data, size, flags, (struct sockaddr*)&a, sizeof a);
}

static int child_ok(pid_t child) {
  int status;
  if (waitpid(child, &status, 0) != child) return 0;
  if (status) printf("child status %#x\n", status);
  return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

static void opaque_and_credentials(void) {
  int a = nl(NETLINK_USERSOCK, SOCK_RAW), b = nl(NETLINK_USERSOCK, SOCK_DGRAM);
  CHECK(a >= 0 && b >= 0);
  struct sockaddr_nl source = local(a), dst = local(b);
  CHECK(source.nl_pid && dst.nl_pid && source.nl_pid != dst.nl_pid);
  int one = 1;
  CHECK(setsockopt(b, SOL_SOCKET, SO_PASSCRED, &one, sizeof one) == 0);
  // Not a valid nlmsghdr: unicast must neither parse nor acknowledge it.
  const char data[] = "opaque\0bytes";
  struct iovec parts[] = {{.iov_base = (void*)data, .iov_len = 3},
                         {.iov_base = (void*)(data + 3), .iov_len = sizeof data - 3}};
  struct msghdr msg = {.msg_name = &dst, .msg_namelen = sizeof dst,
                      .msg_iov = parts, .msg_iovlen = 2};
  CHECK(sendmsg(a, &msg, 0) == sizeof data);
  struct pollfd p = {.fd = b, .events = POLLIN};
  CHECK(poll(&p, 1, 1000) == 1 && (p.revents & POLLIN));
  char buf[32], control[CMSG_SPACE(sizeof(struct ucred))];
  struct sockaddr_nl from = {0};
  struct iovec iov = {.iov_base = buf, .iov_len = sizeof buf};
  msg = (struct msghdr){.msg_name = &from, .msg_namelen = sizeof from,
                       .msg_iov = &iov, .msg_iovlen = 1,
                       .msg_control = control, .msg_controllen = sizeof control};
  CHECK(recvmsg(b, &msg, MSG_PEEK) == sizeof data && !memcmp(buf, data, sizeof data));
  CHECK(from.nl_family == AF_NETLINK && from.nl_pid == source.nl_pid && !from.nl_groups);
  struct cmsghdr* c = CMSG_FIRSTHDR(&msg);
  CHECK(c && c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_CREDENTIALS);
  struct ucred cred;
  memcpy(&cred, CMSG_DATA(c), sizeof cred);
  CHECK(cred.pid == getpid() && cred.uid == getuid() && cred.gid == getgid());
  CHECK(recv(b, buf, sizeof buf, 0) == sizeof data && !memcmp(buf, data, sizeof data));
  CHECK(recv(a, buf, sizeof buf, MSG_DONTWAIT) == -1 && errno == EAGAIN);
  CHECK(put(b, source.nl_pid, "r", 1, 0) == 1 && recv(a, buf, sizeof buf, 0) == 1 && buf[0] == 'r');
  CHECK(put(a, source.nl_pid, "s", 1, 0) == 1 && recv(a, buf, sizeof buf, 0) == 1 && buf[0] == 's');
  close(a);
  close(b);
}

static void connected_peers(void) {
  int a = nl(NETLINK_USERSOCK, SOCK_DGRAM), b = nl(NETLINK_USERSOCK, SOCK_RAW);
  int c = nl(NETLINK_USERSOCK, SOCK_DGRAM);
  CHECK(a >= 0 && b >= 0 && c >= 0);
  unsigned aid = local(a).nl_pid, bid = local(b).nl_pid, cid = local(c).nl_pid;
  CHECK(to(a, bid) == 0 && to(b, aid) == 0);
  struct sockaddr_nl peer = {0};
  socklen_t len = sizeof peer;
  CHECK(getpeername(a, (struct sockaddr*)&peer, &len) == 0 && peer.nl_pid == bid && !peer.nl_groups);
  char buf[8];
  CHECK(send(a, "a", 1, 0) == 1 && recv(b, buf, sizeof buf, 0) == 1 && buf[0] == 'a');
  CHECK(put(c, bid, "c", 1, 0) == -1 && errno == ECONNREFUSED);
  // An explicit destination overrides the connected default.
  CHECK(put(a, cid, "c", 1, 0) == 1 && recv(c, buf, sizeof buf, 0) == 1 && buf[0] == 'c');
  CHECK(getpeername(a, (struct sockaddr*)&peer, &len) == 0 && peer.nl_pid == bid);
  struct sockaddr unspec = {.sa_family = AF_UNSPEC};
  CHECK(connect(b, &unspec, sizeof unspec.sa_family) == 0);
  CHECK(put(c, bid, "c", 1, 0) == 1 && recv(b, buf, sizeof buf, 0) == 1);
  CHECK(to(b, 0) == 0);
  CHECK(put(a, bid, "a", 1, 0) == -1 && errno == ECONNREFUSED);
  peer = (struct sockaddr_nl){.nl_family = AF_NETLINK, .nl_pid = cid, .nl_groups = 12};
  CHECK(connect(a, (struct sockaddr*)&peer, sizeof peer) == 0);
  CHECK(getpeername(a, (struct sockaddr*)&peer, &len) == 0 && peer.nl_pid == cid && peer.nl_groups == 4);
  close(c);
  CHECK(to(a, cid) == 0); // connect does not check liveness.
  CHECK(send(a, "x", 1, 0) == -1 && errno == ECONNREFUSED);
  CHECK(put(a, cid, "x", 1, 0) == -1 && errno == ECONNREFUSED);
  CHECK(send(a, "", 0, 0) == -1 && errno == ENODATA);
  CHECK(send(a, "x", 1, MSG_OOB) == -1 && errno == EOPNOTSUPP);
  CHECK(to(a, 0) == 0 && send(a, "x", 1, 0) == -1 && errno == ECONNREFUSED);
  close(a);
  close(b);
}

static void protocol_isolation(void) {
  int a = nl(NETLINK_ROUTE, SOCK_RAW), b = nl_port(NETLINK_GENERIC, SOCK_RAW, 0x71234567);
  CHECK(a >= 0 && b >= 0);
  struct sockaddr_nl dst = {.nl_family = AF_NETLINK, .nl_pid = 0x71234567};
  CHECK(put(a, dst.nl_pid, "x", 1, 0) == -1 && errno == ECONNREFUSED);
  close(a);
  a = nl(NETLINK_GENERIC, SOCK_DGRAM);
  CHECK(a >= 0 && put(a, dst.nl_pid, "x", 1, 0) == 1);
  char buf[4];
  CHECK(recv(b, buf, sizeof buf, 0) == 1 && buf[0] == 'x');
  close(a);
  close(b);
}

static void cross_process_and_permissions(void) {
  int b = nl(NETLINK_USERSOCK, SOCK_RAW), route = nl(NETLINK_ROUTE, SOCK_RAW);
  int a = nl(NETLINK_ROUTE, SOCK_RAW);
  CHECK(b >= 0 && route >= 0 && a >= 0);
  unsigned bid = local(b).nl_pid, rid = local(route).nl_pid;
  CHECK(to(a, rid) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (!child) {
    if (setgid(2000) || setuid(2000)) _exit(2);
    int u = nl(NETLINK_USERSOCK, SOCK_DGRAM);
    if (u < 0 || to(u, bid) || send(u, "user", 5, 0) != 5) _exit(3);
    struct sockaddr_nl group = {.nl_family = AF_NETLINK, .nl_pid = local(u).nl_pid, .nl_groups = 1};
    if (bind(u, (struct sockaddr*)&group, sizeof group) != -1 || errno != EPERM) _exit(6);
    // Explicit user addresses and connects need CAP_NET_ADMIN here.
    for (int p = 0; p < 3; p++) {
      int proto[] = {NETLINK_ROUTE, NETLINK_GENERIC, NETLINK_KOBJECT_UEVENT};
      int fd = nl(proto[p], SOCK_RAW);
      if (fd < 0 || to(fd, rid) != -1 || errno != EPERM ||
          put(fd, rid, "x", 1, 0) != -1 || errno != EPERM) _exit(4);
      close(fd);
    }
    // Linux checks the destination permission at connect, not again on
    // a send without an explicit address after credentials have changed.
    if (send(a, "route", 6, 0) != 6) _exit(5);
    close(u);
    _exit(0);
  }
  CHECK(child_ok(child));
  char buf[16];
  CHECK(recv(b, buf, sizeof buf, 0) == 5 && !memcmp(buf, "user", 5));
  CHECK(recv(route, buf, sizeof buf, 0) == 6 && !memcmp(buf, "route", 6));
  close(a);
  close(b);
  close(route);
}

static void fork_and_exec(void) {
  int a = nl(NETLINK_USERSOCK, SOCK_RAW), b = nl(NETLINK_USERSOCK, SOCK_RAW);
  int c = nl(NETLINK_USERSOCK, SOCK_DGRAM);
  CHECK(a >= 0 && b >= 0 && c >= 0);
  unsigned cid = local(c).nl_pid;
  CHECK(to(a, local(b).nl_pid) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (!child) {
    if (to(a, cid)) _exit(2);
    char afd[24], id[24];
    snprintf(afd, sizeof afd, "%d", a);
    snprintf(id, sizeof id, "%u", cid);
    execl(program, program, "--exec", afd, id, NULL);
    perror("execl");
    _exit(3);
  }
  CHECK(child_ok(child));
  struct sockaddr_nl peer;
  socklen_t len = sizeof peer;
  CHECK(getpeername(a, (struct sockaddr*)&peer, &len) == 0 && peer.nl_pid == cid);
  char buf[16];
  CHECK(recv(c, buf, sizeof buf, 0) == 5 && !memcmp(buf, "exec", 5));
  CHECK(send(a, "parent", 7, 0) == 7 && recv(c, buf, sizeof buf, 0) == 7 && !memcmp(buf, "parent", 7));
  close(a);
  close(b);
  close(c);
}

static void queue_errors(void) {
  int a = nl(NETLINK_USERSOCK, SOCK_DGRAM), b = nl(NETLINK_USERSOCK, SOCK_RAW);
  CHECK(a >= 0 && b >= 0 && to(a, local(b).nl_pid) == 0);
  int size = 4096;
  CHECK(setsockopt(b, SOL_SOCKET, SO_RCVBUF, &size, sizeof size) == 0);
  char data[1024] = {0};
  int n;
  for (n = 0; n < 4096; n++) {
    if (send(a, data, sizeof data, MSG_DONTWAIT) < 0) break;
  }
  CHECK(n > 0 && n < 4096 && errno == EAGAIN);
  CHECK(fcntl(a, F_SETFL, O_NONBLOCK) == 0);
  CHECK(send(a, data, sizeof data, 0) == -1 && errno == EAGAIN);
  CHECK(recv(b, data, sizeof data, 0) == sizeof data);
  CHECK(send(a, data, sizeof data, 0) == sizeof data);
  CHECK(fcntl(a, F_SETFL, 0) == 0);
  struct timeval timeout = {.tv_usec = 30000};
  CHECK(setsockopt(a, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof timeout) == 0);
  struct timespec before, after;
  CHECK(clock_gettime(CLOCK_MONOTONIC, &before) == 0);
  CHECK(send(a, data, sizeof data, 0) == -1 && errno == EAGAIN);
  CHECK(clock_gettime(CLOCK_MONOTONIC, &after) == 0);
  double elapsed = after.tv_sec - before.tv_sec + (after.tv_nsec - before.tv_nsec) / 1e9;
  CHECK(elapsed >= 0.015 && elapsed < 1);
  // The signal must arrive well before the send timeout, even on a loaded host.
  timeout.tv_sec = 2;
  timeout.tv_usec = 0;
  CHECK(setsockopt(a, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof timeout) == 0);
  struct sigaction action = {.sa_handler = alarm_handler};
  CHECK(sigemptyset(&action.sa_mask) == 0 && sigaction(SIGALRM, &action, NULL) == 0);
  struct itimerval timer = {.it_value = {.tv_usec = 10000}};
  CHECK(setitimer(ITIMER_REAL, &timer, NULL) == 0);
  CHECK(send(a, data, sizeof data, 0) == -1 && errno == EINTR && alarm_seen);
  timer.it_value.tv_usec = 0;
  CHECK(setitimer(ITIMER_REAL, &timer, NULL) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (!child) {
    usleep(30000);
    _exit(recv(b, data, sizeof data, 0) == sizeof data ? 0 : 1);
  }
  CHECK(send(a, data, sizeof data, 0) == sizeof data);
  CHECK(child_ok(child));
  close(a);
  close(b);
}

static void group_and_unicast(void) {
  int a = nl(NETLINK_USERSOCK, SOCK_RAW);
  int b = nl_port(NETLINK_USERSOCK, SOCK_DGRAM, 0x71234568);
  int c = nl(NETLINK_USERSOCK, SOCK_RAW);
  CHECK(a >= 0 && b >= 0 && c >= 0);
  int group = 1;
  CHECK(setsockopt(b, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, &group, sizeof group) == 0);
  CHECK(setsockopt(c, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, &group, sizeof group) == 0);
  struct sockaddr_nl dst = local(b);
  dst.nl_groups = 3; // Linux sends only to the lowest selected group.
  CHECK(sendto(a, "g", 1, 0, (struct sockaddr*)&dst, sizeof dst) == 1);
  struct sockaddr_nl from;
  socklen_t len = sizeof from;
  char data[4];
  CHECK(recvfrom(b, data, sizeof data, 0, (struct sockaddr*)&from, &len) == 1);
  CHECK(from.nl_pid == local(a).nl_pid && from.nl_groups == 1);
  CHECK(recv(b, data, sizeof data, MSG_DONTWAIT) == -1 && errno == EAGAIN);
  CHECK(recvfrom(c, data, sizeof data, 0, (struct sockaddr*)&from, &len) == 1);
  CHECK(from.nl_pid == local(a).nl_pid && from.nl_groups == 1);
  close(a);
  close(b);
  close(c);
}

int main(int argc, char** argv) {
  program = argv[0];
  if (argc == 4 && !strcmp(argv[1], "--exec")) {
    int fd = atoi(argv[2]);
    struct sockaddr_nl peer;
    socklen_t len = sizeof peer;
    if (getpeername(fd, (struct sockaddr*)&peer, &len)) { perror("exec getpeername"); return 1; }
    if (peer.nl_pid != strtoul(argv[3], NULL, 10)) { printf("exec peer %u expected %s\n", peer.nl_pid, argv[3]); return 2; }
    if (send(fd, "exec", 5, 0) != 5) { perror("exec send"); return 3; }
    return 0;
  }
  RUN(opaque_and_credentials);
  RUN(connected_peers);
  RUN(protocol_isolation);
  RUN(cross_process_and_permissions);
  RUN(fork_and_exec);
  RUN(queue_errors);
  RUN(group_and_unicast);
  DONE();
}
