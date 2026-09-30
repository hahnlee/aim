// AF_UNIX stream/dgram/seqpacket, names, credentials, fd passing, options,
// and AF_INET passthrough. argv[1] is a writable directory.
#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <stddef.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <unistd.h>

#include "check.h"

static const char* dir;

static socklen_t path_addr(struct sockaddr_un* a, const char* name) {
  memset(a, 0, sizeof *a);
  a->sun_family = AF_UNIX;
  snprintf(a->sun_path, sizeof a->sun_path, "%s/%s", dir, name);
  unlink(a->sun_path);
  return offsetof(struct sockaddr_un, sun_path) + strlen(a->sun_path) + 1;
}

static socklen_t abstract_addr(struct sockaddr_un* a, const char* name) {
  memset(a, 0, sizeof *a);
  a->sun_family = AF_UNIX;
  memcpy(a->sun_path + 1, name, strlen(name));
  return offsetof(struct sockaddr_un, sun_path) + 1 + strlen(name);
}

static void seqpacket_pair_boundaries(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, sv) == 0);
  int type = 0;
  socklen_t len = sizeof type;
  CHECK(getsockopt(sv[0], SOL_SOCKET, SO_TYPE, &type, &len) == 0 && type == SOCK_SEQPACKET);
  CHECK(send(sv[0], "hello", 5, 0) == 5);
  CHECK(write(sv[0], "", 0) == 0);
  CHECK(send(sv[0], "world!", 6, 0) == 6);
  char buf[64];
  CHECK(recv(sv[1], buf, sizeof buf, 0) == 5 && memcmp(buf, "hello", 5) == 0);
  CHECK(recv(sv[1], buf, sizeof buf, 0) == 0);  // the empty message
  // Truncation drops the rest of the message.
  struct iovec iov = {buf, 3};
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1};
  CHECK(recvmsg(sv[1], &m, 0) == 3 && (m.msg_flags & MSG_TRUNC));
  CHECK(send(sv[0], "next", 4, 0) == 4);
  CHECK(recv(sv[1], buf, 2, MSG_PEEK) == 2 && memcmp(buf, "ne", 2) == 0);
  CHECK(recv(sv[1], buf, sizeof buf, MSG_TRUNC) == 4);
  CHECK(recv(sv[1], buf, sizeof buf, MSG_DONTWAIT) == -1 && errno == EAGAIN);
  // A large message arrives whole.
  static char big[100000];
  memset(big, 'x', sizeof big);
  CHECK(send(sv[0], big, sizeof big, 0) == sizeof big);
  static char got[200000];
  CHECK(recv(sv[1], got, sizeof got, 0) == sizeof big);
  close(sv[0]);
  CHECK(recv(sv[1], buf, sizeof buf, 0) == 0);  // EOF
  struct pollfd p = {.fd = sv[1], .events = POLLIN};
  CHECK(poll(&p, 1, 0) == 1 && (p.revents & POLLHUP));
  close(sv[1]);
}

static void seqpacket_listen_path(void) {
  struct sockaddr_un a;
  socklen_t alen = path_addr(&a, "seq.sock");
  int s = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
  CHECK(s >= 0);
  CHECK(bind(s, (struct sockaddr*)&a, alen) == 0);
  CHECK(listen(s, 4) == 0);
  struct stat st;
  CHECK(stat(a.sun_path, &st) == 0 && S_ISSOCK(st.st_mode));
  int c = socket(AF_UNIX, SOCK_SEQPACKET, 0);
  CHECK(connect(c, (struct sockaddr*)&a, alen) == 0);
  int d = accept4(s, NULL, NULL, SOCK_CLOEXEC);
  CHECK(d >= 0);
  CHECK((fcntl(d, F_GETFL) & O_NONBLOCK) == 0);
  CHECK((fcntl(d, F_GETFD) & FD_CLOEXEC) != 0);
  struct sockaddr_un got;
  socklen_t gl = sizeof got;
  CHECK(getsockname(s, (struct sockaddr*)&got, &gl) == 0 && strcmp(got.sun_path, a.sun_path) == 0);
  gl = sizeof got;
  CHECK(getpeername(c, (struct sockaddr*)&got, &gl) == 0 && strcmp(got.sun_path, a.sun_path) == 0);
  CHECK(send(c, "a", 1, 0) == 1 && send(c, "bc", 2, 0) == 2);
  char buf[8];
  CHECK(recv(d, buf, sizeof buf, 0) == 1 && recv(d, buf, sizeof buf, 0) == 2);
  int type;
  socklen_t tl = sizeof type;
  CHECK(getsockopt(d, SOL_SOCKET, SO_TYPE, &type, &tl) == 0 && type == SOCK_SEQPACKET);
  struct ucred cr;
  socklen_t cl = sizeof cr;
  CHECK(getsockopt(d, SOL_SOCKET, SO_PEERCRED, &cr, &cl) == 0 && cr.pid == getpid());
  CHECK(cr.uid == getuid() && cr.gid == getgid());
  close(c);
  close(d);
  close(s);
  unlink(a.sun_path);
}

static void long_host_path(void) {
  // The guest path is short; the host path behind /data is not.
  char sub[256];
  snprintf(sub, sizeof sub, "%s/a-rather-long-directory-name-to-pass-the-sun-path-limit", dir);
  mkdir(sub, 0755);
  struct sockaddr_un a;
  memset(&a, 0, sizeof a);
  a.sun_family = AF_UNIX;
  snprintf(a.sun_path, sizeof a.sun_path, "%s/s", sub);
  unlink(a.sun_path);
  socklen_t alen = offsetof(struct sockaddr_un, sun_path) + strlen(a.sun_path) + 1;
  int s = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(bind(s, (struct sockaddr*)&a, alen) == 0);
  CHECK(listen(s, 1) == 0);
  int c = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(connect(c, (struct sockaddr*)&a, alen) == 0);
  int d = accept(s, NULL, NULL);
  CHECK(d >= 0 && write(c, "L", 1) == 1);
  char b;
  CHECK(read(d, &b, 1) == 1 && b == 'L');
  struct sockaddr_un got;
  socklen_t gl = sizeof got;
  CHECK(getsockname(s, (struct sockaddr*)&got, &gl) == 0 && strcmp(got.sun_path, a.sun_path) == 0);
  close(c);
  close(d);
  close(s);
  unlink(a.sun_path);
  rmdir(sub);
}

static void abstract_names(void) {
  struct sockaddr_un a;
  socklen_t alen = abstract_addr(&a, "sysio-test/abstract");
  int s = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(bind(s, (struct sockaddr*)&a, alen) == 0);
  CHECK(listen(s, 1) == 0);
  int s2 = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(bind(s2, (struct sockaddr*)&a, alen) == -1 && errno == EADDRINUSE);
  close(s2);
  int c = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(connect(c, (struct sockaddr*)&a, alen) == 0);
  struct sockaddr_un got;
  socklen_t gl = sizeof got;
  CHECK(getsockname(s, (struct sockaddr*)&got, &gl) == 0 && gl == alen);
  CHECK(got.sun_path[0] == 0 && memcmp(got.sun_path + 1, "sysio-test/abstract", 19) == 0);
  close(c);
  close(s);
  // The name is free again once its socket is closed.
  s = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(bind(s, (struct sockaddr*)&a, alen) == 0);
  close(s);
  struct sockaddr_un none;
  socklen_t nlen = abstract_addr(&none, "sysio-test/nobody");
  c = socket(AF_UNIX, SOCK_STREAM, 0);
  CHECK(connect(c, (struct sockaddr*)&none, nlen) == -1 && errno == ECONNREFUSED);
  close(c);
}

static void dgram_credentials(void) {
  struct sockaddr_un a;
  socklen_t alen = path_addr(&a, "dgram.sock");
  int r = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(bind(r, (struct sockaddr*)&a, alen) == 0);
  int on = 1;
  CHECK(setsockopt(r, SOL_SOCKET, SO_PASSCRED, &on, sizeof on) == 0);
  int w = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
  CHECK(connect(w, (struct sockaddr*)&a, alen) == 0);
  // logd-sized datagrams, as liblog writes them.
  static char msg[4096];
  memset(msg, 'm', sizeof msg);
  struct iovec parts[2] = {{msg, 20}, {msg + 20, sizeof msg - 20}};
  CHECK(writev(w, parts, 2) == sizeof msg);
  CHECK(sendto(w, "tiny", 4, 0, NULL, 0) == 4);
  char buf[8192];
  union {
    struct cmsghdr h;
    char b[CMSG_SPACE(sizeof(struct ucred))];
  } ctl;
  struct iovec iov = {buf, sizeof buf};
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &ctl, .msg_controllen = sizeof ctl};
  CHECK(recvmsg(r, &m, 0) == sizeof msg);
  struct cmsghdr* c = CMSG_FIRSTHDR(&m);
  CHECK(c && c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_CREDENTIALS);
  struct ucred* cr = (struct ucred*)CMSG_DATA(c);
  CHECK(cr->pid == getpid() && cr->uid == getuid() && cr->gid == getgid());
  CHECK(memcmp(buf, msg, sizeof msg) == 0);
  CHECK(recv(r, buf, sizeof buf, 0) == 4 && memcmp(buf, "tiny", 4) == 0);
  // Without SO_PASSCRED no credentials are attached.
  on = 0;
  setsockopt(r, SOL_SOCKET, SO_PASSCRED, &on, sizeof on);
  CHECK(send(w, "x", 1, 0) == 1);
  m.msg_controllen = sizeof ctl;
  CHECK(recvmsg(r, &m, 0) == 1 && m.msg_controllen == 0);
  close(w);
  close(r);
  unlink(a.sun_path);
}

static void scm_rights(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
  int pfd[2];
  CHECK(pipe(pfd) == 0);
  union {
    struct cmsghdr h;
    char b[CMSG_SPACE(2 * sizeof(int))];
  } ctl;
  memset(&ctl, 0, sizeof ctl);
  struct iovec iov = {"F", 1};
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &ctl, .msg_controllen = sizeof ctl};
  struct cmsghdr* c = CMSG_FIRSTHDR(&m);
  c->cmsg_level = SOL_SOCKET;
  c->cmsg_type = SCM_RIGHTS;
  c->cmsg_len = CMSG_LEN(2 * sizeof(int));
  memcpy(CMSG_DATA(c), pfd, sizeof pfd);
  CHECK(sendmsg(sv[0], &m, 0) == 1);
  char b;
  union {
    struct cmsghdr h;
    char b[CMSG_SPACE(2 * sizeof(int))];
  } in;
  struct iovec riov = {&b, 1};
  struct msghdr rm = {.msg_iov = &riov, .msg_iovlen = 1, .msg_control = &in, .msg_controllen = sizeof in};
  CHECK(recvmsg(sv[1], &rm, MSG_CMSG_CLOEXEC) == 1 && b == 'F');
  c = CMSG_FIRSTHDR(&rm);
  CHECK(c && c->cmsg_type == SCM_RIGHTS && c->cmsg_len == CMSG_LEN(2 * sizeof(int)));
  int got[2];
  memcpy(got, CMSG_DATA(c), sizeof got);
  CHECK((fcntl(got[0], F_GETFD) & FD_CLOEXEC) != 0);
  CHECK(write(got[1], "z", 1) == 1);
  CHECK(read(pfd[0], &b, 1) == 1 && b == 'z');
  close(got[0]);
  close(got[1]);
  close(pfd[0]);
  close(pfd[1]);
  // A SEQPACKET socket passed over a socket keeps its boundaries.
  int q[2];
  CHECK(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, q) == 0);
  memset(&ctl, 0, sizeof ctl);
  m.msg_control = &ctl;
  m.msg_controllen = CMSG_SPACE(sizeof(int));
  c = CMSG_FIRSTHDR(&m);
  c->cmsg_level = SOL_SOCKET;
  c->cmsg_type = SCM_RIGHTS;
  c->cmsg_len = CMSG_LEN(sizeof(int));
  memcpy(CMSG_DATA(c), &q[1], sizeof(int));
  CHECK(sendmsg(sv[0], &m, 0) == 1);
  close(q[1]);
  rm.msg_controllen = sizeof in;
  CHECK(recvmsg(sv[1], &rm, 0) == 1);
  int moved;
  memcpy(&moved, CMSG_DATA(CMSG_FIRSTHDR(&rm)), sizeof moved);
  CHECK(send(q[0], "one", 3, 0) == 3 && send(q[0], "two", 3, 0) == 3);
  char buf[8];
  CHECK(recv(moved, buf, sizeof buf, 0) == 3 && recv(moved, buf, sizeof buf, 0) == 3);
  close(moved);
  close(q[0]);
  close(sv[0]);
  close(sv[1]);
}

static void stream_passcred_and_shutdown(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
  int on = 1;
  CHECK(setsockopt(sv[1], SOL_SOCKET, SO_PASSCRED, &on, sizeof on) == 0);
  int v = 0;
  socklen_t vl = sizeof v;
  CHECK(getsockopt(sv[1], SOL_SOCKET, SO_PASSCRED, &v, &vl) == 0 && v == 1);
  CHECK(write(sv[0], "abc", 3) == 3);
  char buf[8];
  union {
    struct cmsghdr h;
    char b[CMSG_SPACE(sizeof(struct ucred))];
  } ctl;
  struct iovec iov = {buf, sizeof buf};
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &ctl, .msg_controllen = sizeof ctl};
  CHECK(recvmsg(sv[1], &m, 0) == 3);
  struct cmsghdr* c = CMSG_FIRSTHDR(&m);
  CHECK(c && c->cmsg_type == SCM_CREDENTIALS && ((struct ucred*)CMSG_DATA(c))->pid == getpid());
  CHECK(shutdown(sv[0], SHUT_WR) == 0);
  CHECK(read(sv[1], buf, sizeof buf) == 0);
  CHECK(send(sv[1], "q", 1, MSG_NOSIGNAL) == 1);
  close(sv[1]);
  CHECK(send(sv[0], "q", 1, MSG_NOSIGNAL) == -1 && errno == EPIPE);
  close(sv[0]);
}

static void options(void) {
  int s = socket(AF_UNIX, SOCK_STREAM, 0);
  int sz = 65536;
  CHECK(setsockopt(s, SOL_SOCKET, SO_SNDBUF, &sz, sizeof sz) == 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_RCVBUF, &sz, sizeof sz) == 0);
  int got = 0;
  socklen_t gl = sizeof got;
  CHECK(getsockopt(s, SOL_SOCKET, SO_RCVBUF, &got, &gl) == 0 && got >= sz);
  struct timeval tv = {1, 500000};
  CHECK(setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv) == 0);
  struct timeval tg;
  gl = sizeof tg;
  CHECK(getsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tg, &gl) == 0 && tg.tv_sec == 1 && tg.tv_usec == 500000);
  int on = 1;
  CHECK(setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  int dom = 0;
  gl = sizeof dom;
  CHECK(getsockopt(s, SOL_SOCKET, SO_DOMAIN, &dom, &gl) == 0 && dom == AF_UNIX);
  int err = -1;
  gl = sizeof err;
  CHECK(getsockopt(s, SOL_SOCKET, SO_ERROR, &err, &gl) == 0 && err == 0);
  close(s);
  CHECK(socket(AF_BLUETOOTH, SOCK_RAW, 0) == -1 && errno == EAFNOSUPPORT);
  int one = 1;
  CHECK(setsockopt(0, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one) == -1);
}

static void inet_loopback(void) {
  socklen_t gl;
  int s = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  CHECK(s >= 0);
  int on = 1;
  CHECK(setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  struct sockaddr_in a = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  CHECK(bind(s, (struct sockaddr*)&a, sizeof a) == 0);
  socklen_t al = sizeof a;
  CHECK(getsockname(s, (struct sockaddr*)&a, &al) == 0 && a.sin_family == AF_INET && a.sin_port != 0);
  CHECK(listen(s, 1) == 0);
  int c = socket(AF_INET, SOCK_STREAM, 0);
  CHECK(connect(c, (struct sockaddr*)&a, sizeof a) == 0);
  CHECK(setsockopt(c, IPPROTO_TCP, TCP_NODELAY, &on, sizeof on) == 0);
  // tcp(7): TCP_USER_TIMEOUT reads back as set, in milliseconds.
  unsigned ut = 0;
  socklen_t utl = sizeof ut;
  CHECK(getsockopt(c, IPPROTO_TCP, TCP_USER_TIMEOUT, &ut, &utl) == 0 && ut == 0);
  ut = 1500;
  CHECK(setsockopt(c, IPPROTO_TCP, TCP_USER_TIMEOUT, &ut, sizeof ut) == 0);
  ut = 0;
  CHECK(getsockopt(c, IPPROTO_TCP, TCP_USER_TIMEOUT, &ut, &utl) == 0 && ut == 1500);
  int neg = -1;
  CHECK(setsockopt(c, IPPROTO_TCP, TCP_USER_TIMEOUT, &neg, sizeof neg) == -1 && errno == EINVAL);
  struct sockaddr_in peer;
  socklen_t pl = sizeof peer;
  int d = accept(s, (struct sockaddr*)&peer, &pl);
  CHECK(d >= 0 && pl == sizeof peer && peer.sin_family == AF_INET && peer.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
  CHECK(write(c, "tcp", 3) == 3);
  char buf[4];
  CHECK(read(d, buf, 3) == 3 && memcmp(buf, "tcp", 3) == 0);
  close(c);
  close(d);
  close(s);
  int u = socket(AF_INET6, SOCK_DGRAM, 0);
  CHECK(u >= 0);
  // libcore's DatagramSocket clears IP_MULTICAST_ALL, an IPv4-level
  // option Linux takes on an IPv6 socket too (ip(7)).
  int all = -1;
  gl = sizeof all;
  CHECK(getsockopt(u, IPPROTO_IP, IP_MULTICAST_ALL, &all, &gl) == 0 && all == 1);
  int off = 0, two = 2;
  CHECK(setsockopt(u, IPPROTO_IP, IP_MULTICAST_ALL, &off, sizeof off) == 0);
  CHECK(getsockopt(u, IPPROTO_IP, IP_MULTICAST_ALL, &all, &gl) == 0 && all == 0);
  CHECK(setsockopt(u, IPPROTO_IP, IP_MULTICAST_ALL, &two, sizeof two) == -1 && errno == EINVAL);
  struct sockaddr_in6 a6 = {.sin6_family = AF_INET6, .sin6_addr = IN6ADDR_LOOPBACK_INIT};
  CHECK(bind(u, (struct sockaddr*)&a6, sizeof a6) == 0);
  socklen_t l6 = sizeof a6;
  CHECK(getsockname(u, (struct sockaddr*)&a6, &l6) == 0 && a6.sin6_family == AF_INET6);
  CHECK(sendto(u, "u6", 2, 0, (struct sockaddr*)&a6, sizeof a6) == 2);
  struct sockaddr_in6 from;
  socklen_t fl = sizeof from;
  CHECK(recvfrom(u, buf, sizeof buf, 0, (struct sockaddr*)&from, &fl) == 2 && from.sin6_family == AF_INET6);
  close(u);
  // Datagram sockets that all set SO_REUSEADDR share a port (socket(7)).
  int d1 = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0), d2 = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(setsockopt(d1, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  CHECK(setsockopt(d2, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  struct sockaddr_in any = {.sin_family = AF_INET};
  CHECK(bind(d1, (struct sockaddr*)&any, sizeof any) == 0);
  al = sizeof any;
  CHECK(getsockname(d1, (struct sockaddr*)&any, &al) == 0 && any.sin_port != 0);
  CHECK(bind(d2, (struct sockaddr*)&any, sizeof any) == 0);
  close(d1);
  close(d2);
}

int main(int argc, char** argv) {
  dir = argc > 1 ? argv[1] : "/data/local/tmp";
  RUN(seqpacket_pair_boundaries);
  RUN(seqpacket_listen_path);
  RUN(long_host_path);
  RUN(abstract_names);
  RUN(dgram_credentials);
  RUN(scm_rights);
  RUN(stream_passcred_and_shutdown);
  RUN(options);
  RUN(inet_loopback);
  DONE();
}
