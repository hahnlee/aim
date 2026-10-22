// IP and socket receive options iputils and DnsResolver use: IP_RECVTTL,
// IP_RECVTOS and SO_TIMESTAMP(NS) control messages (on UDP and ping
// sockets), the IP_RETOPTS flag, and TCP_MAXSEG before a connection.
#include <arpa/inet.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

static int on = 1;

// A UDP socket bound to the loopback; its address in `a`.
static int udp(struct sockaddr_in* a) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  *a = (struct sockaddr_in){.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  socklen_t l = sizeof *a;
  if (s < 0 || bind(s, (struct sockaddr*)a, l) != 0 || getsockname(s, (struct sockaddr*)a, &l) != 0)
    return -1;
  return s;
}

struct seen {
  int ttl, tos, stamp, stampns, count;
  struct timeval tv;
  struct timespec ts;
};

static ssize_t receive(int s, struct seen* seen) {
  char data[256], control[256];
  struct iovec v = {data, sizeof data};
  struct msghdr m = {.msg_iov = &v, .msg_iovlen = 1, .msg_control = control,
                     .msg_controllen = sizeof control};
  ssize_t n = recvmsg(s, &m, 0);
  *seen = (struct seen){.ttl = -1, .tos = -1};
  for (struct cmsghdr* c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c)) {
    seen->count++;
    if (c->cmsg_level == SOL_IP && c->cmsg_type == IP_TTL) seen->ttl = *(int*)CMSG_DATA(c);
    if (c->cmsg_level == SOL_IP && c->cmsg_type == IP_TOS) seen->tos = *CMSG_DATA(c);
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_TIMESTAMP) {
      seen->stamp = 1;
      memcpy(&seen->tv, CMSG_DATA(c), sizeof seen->tv);
    }
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_TIMESTAMPNS) {
      seen->stampns = 1;
      memcpy(&seen->ts, CMSG_DATA(c), sizeof seen->ts);
    }
  }
  return n;
}

static void udp_cmsgs(void) {
  struct sockaddr_in a, b;
  int r = udp(&a), w = udp(&b);
  CHECK(r >= 0 && w >= 0);
  CHECK(setsockopt(r, IPPROTO_IP, IP_RECVTTL, &on, sizeof on) == 0);
  CHECK(setsockopt(r, IPPROTO_IP, IP_RECVTOS, &on, sizeof on) == 0);
  CHECK(setsockopt(r, SOL_SOCKET, SO_TIMESTAMP, &on, sizeof on) == 0);
  int v = 0;
  socklen_t l = sizeof v;
  CHECK(getsockopt(r, IPPROTO_IP, IP_RECVTTL, &v, &l) == 0 && v == 1);
  CHECK(getsockopt(r, SOL_SOCKET, SO_TIMESTAMP, &v, &l) == 0 && v == 1);
  int ttl = 33, tos = 0x10;
  CHECK(setsockopt(w, IPPROTO_IP, IP_TTL, &ttl, sizeof ttl) == 0);
  CHECK(setsockopt(w, IPPROTO_IP, IP_TOS, &tos, sizeof tos) == 0);
  struct timeval before;
  gettimeofday(&before, NULL);
  CHECK(sendto(w, "x", 1, 0, (struct sockaddr*)&a, sizeof a) == 1);
  struct seen s;
  CHECK(receive(r, &s) == 1);
  CHECK(s.count == 3 && s.ttl == 33 && s.tos == 0x10 && s.stamp && !s.stampns);
  CHECK(s.tv.tv_sec >= before.tv_sec && s.tv.tv_sec <= before.tv_sec + 5);
  // SO_TIMESTAMPNS replaces SO_TIMESTAMP.
  CHECK(setsockopt(r, SOL_SOCKET, SO_TIMESTAMPNS, &on, sizeof on) == 0);
  CHECK(getsockopt(r, SOL_SOCKET, SO_TIMESTAMP, &v, &l) == 0 && v == 0);
  CHECK(getsockopt(r, SOL_SOCKET, SO_TIMESTAMPNS, &v, &l) == 0 && v == 1);
  CHECK(sendto(w, "y", 1, 0, (struct sockaddr*)&a, sizeof a) == 1);
  CHECK(receive(r, &s) == 1);
  CHECK(s.stampns && !s.stamp && s.ts.tv_sec >= before.tv_sec && s.ts.tv_nsec < 1000000000);
  // Off: no cmsgs.
  int off = 0;
  CHECK(setsockopt(r, SOL_SOCKET, SO_TIMESTAMPNS, &off, sizeof off) == 0);
  CHECK(setsockopt(r, IPPROTO_IP, IP_RECVTTL, &off, sizeof off) == 0);
  CHECK(setsockopt(r, IPPROTO_IP, IP_RECVTOS, &off, sizeof off) == 0);
  CHECK(sendto(w, "z", 1, 0, (struct sockaddr*)&a, sizeof a) == 1);
  CHECK(receive(r, &s) == 1 && s.count == 0);
  close(r);
  close(w);
}

static void ping_ttl(void) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, IPPROTO_ICMP);
  if (s < 0 && (errno == EACCES || errno == EPERM || errno == EPROTONOSUPPORT)) {
    printf("skip %s: no ping sockets on the host\n", __func__);
    skipped = 1;
    return;
  }
  CHECK(s >= 0);
  CHECK(setsockopt(s, IPPROTO_IP, IP_RECVTTL, &on, sizeof on) == 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_TIMESTAMP, &on, sizeof on) == 0);
  // iputils ping asks for the options of each reply's reply, a flag; a
  // packet without IP options gives none.
  CHECK(setsockopt(s, IPPROTO_IP, IP_RETOPTS, &on, sizeof on) == 0);
  int v = 0;
  socklen_t l = sizeof v;
  CHECK(getsockopt(s, IPPROTO_IP, IP_RETOPTS, &v, &l) == 0 && v == 1);
  unsigned char echo[16] = {8, 0, 0, 0, 0x12, 0x34, 0, 1, 'a', 'i', 'm'};
  unsigned sum = 0;
  for (int i = 0; i < 16; i += 2) sum += echo[i] << 8 | echo[i + 1];
  while (sum >> 16) sum = (sum & 0xffff) + (sum >> 16);
  echo[2] = ~sum >> 8, echo[3] = ~sum & 0xff;
  struct sockaddr_in lo = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  CHECK(sendto(s, echo, sizeof echo, 0, (struct sockaddr*)&lo, sizeof lo) == sizeof echo);
  struct pollfd p = {.fd = s, .events = POLLIN};
  struct seen seen;
  int found = 0;
  while (!found && poll(&p, 1, 2000) == 1 && receive(s, &seen) > 0) {
    found = seen.ttl > 0;
  }
  CHECK(found && seen.ttl == 64 && seen.stamp && seen.count == 2);
  close(s);
}

static void maxseg(void) {
  int s = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  CHECK(s >= 0);
  int v = 0;
  socklen_t l = sizeof v;
  CHECK(getsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &v, &l) == 0 && v == 536);
  // DnsResolver sets it before connecting.
  int mss = 1400;
  CHECK(setsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &mss, sizeof mss) == 0);
  CHECK(getsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &v, &l) == 0 && v == 1400);
  mss = 87;
  CHECK(setsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &mss, sizeof mss) < 0 && errno == EINVAL);
  mss = 32768;
  CHECK(setsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &mss, sizeof mss) < 0 && errno == EINVAL);
  mss = 0;
  CHECK(setsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &mss, sizeof mss) == 0);
  CHECK(getsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &v, &l) == 0 && v == 536);
  // A connected socket reports the segment size in use.
  int ls = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  struct sockaddr_in a = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  socklen_t al = sizeof a;
  CHECK(bind(ls, (struct sockaddr*)&a, al) == 0 && listen(ls, 1) == 0);
  CHECK(getsockname(ls, (struct sockaddr*)&a, &al) == 0);
  mss = 1200;
  CHECK(setsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &mss, sizeof mss) == 0);
  CHECK(connect(s, (struct sockaddr*)&a, al) == 0);
  CHECK(getsockopt(s, IPPROTO_TCP, TCP_MAXSEG, &v, &l) == 0 && v > 0 && v <= 1200);
  close(s);
  close(ls);
  int s6 = socket(AF_INET6, SOCK_STREAM | SOCK_CLOEXEC, 0);
  mss = 1220;
  CHECK(s6 >= 0 && setsockopt(s6, IPPROTO_TCP, TCP_MAXSEG, &mss, sizeof mss) == 0);
  close(s6);
}

int main(void) {
  RUN(udp_cmsgs);
  RUN(ping_ttl);
  RUN(maxseg);
  DONE();
}
