// The kernel's network devices: getifaddrs and if_nametoindex, the
// interface ioctls, NETLINK_ROUTE dumps, changes and group announcements
// (also to another process), SO_MARK and SO_BINDTODEVICE, and an AF_PACKET
// DHCP exchange with eth0's virtual router. Runs as root (all capabilities).
#include <arpa/inet.h>
#include <ifaddrs.h>
#include <linux/filter.h>
#include <linux/if_packet.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <linux/if_ether.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static void interfaces(void) {
  CHECK(if_nametoindex("lo") == 1 && if_nametoindex("eth0") == 2);
  CHECK(if_nametoindex("wlan9") == 0);
  char name[IF_NAMESIZE];
  CHECK(if_indextoname(2, name) && strcmp(name, "eth0") == 0);
  struct ifaddrs* ifa;
  CHECK(getifaddrs(&ifa) == 0);
  int lo4 = 0, lo6 = 0, eth_link = 0;
  for (struct ifaddrs* i = ifa; i; i = i->ifa_next) {
    if (!i->ifa_addr) continue;
    if (strcmp(i->ifa_name, "lo") == 0 && i->ifa_addr->sa_family == AF_INET &&
        ((struct sockaddr_in*)i->ifa_addr)->sin_addr.s_addr == htonl(INADDR_LOOPBACK)) {
      lo4 = 1;
    }
    if (strcmp(i->ifa_name, "lo") == 0 && i->ifa_addr->sa_family == AF_INET6) lo6 = 1;
    if (strcmp(i->ifa_name, "eth0") == 0 && i->ifa_addr->sa_family == AF_PACKET) {
      struct sockaddr_ll* ll = (struct sockaddr_ll*)i->ifa_addr;
      eth_link = ll->sll_halen == 6 && ll->sll_addr[0] == 0x02 && (i->ifa_flags & IFF_BROADCAST);
    }
  }
  freeifaddrs(ifa);
  CHECK(lo4 && lo6 && eth_link);
}

static void ioctls(void) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(s >= 0);
  struct ifreq r;
  memset(&r, 0, sizeof r);
  strcpy(r.ifr_name, "eth0");
  CHECK(ioctl(s, SIOCGIFHWADDR, &r) == 0 && r.ifr_hwaddr.sa_family == 1 &&
        (unsigned char)r.ifr_hwaddr.sa_data[0] == 0x02);
  CHECK(ioctl(s, SIOCGIFMTU, &r) == 0 && r.ifr_mtu == 1500);
  CHECK(ioctl(s, SIOCGIFINDEX, &r) == 0 && r.ifr_ifindex == 2);
  strcpy(r.ifr_name, "lo");
  CHECK(ioctl(s, SIOCGIFFLAGS, &r) == 0 && (r.ifr_flags & (IFF_UP | IFF_LOOPBACK | IFF_RUNNING)) ==
                                               (IFF_UP | IFF_LOOPBACK | IFF_RUNNING));
  CHECK(ioctl(s, SIOCGIFADDR, &r) == 0 &&
        ((struct sockaddr_in*)&r.ifr_addr)->sin_addr.s_addr == htonl(INADDR_LOOPBACK));
  strcpy(r.ifr_name, "nope0");
  CHECK(ioctl(s, SIOCGIFFLAGS, &r) == -1 && errno == ENODEV);
  char buf[10 * sizeof(struct ifreq)];
  struct ifconf c = {.ifc_len = sizeof buf, .ifc_buf = buf};
  CHECK(ioctl(s, SIOCGIFCONF, &c) == 0 && c.ifc_len >= (int)sizeof(struct ifreq) &&
        strcmp(c.ifc_req[0].ifr_name, "lo") == 0);
  close(s);
}

// A NETLINK_ROUTE socket in `groups`.
static int rtnl(unsigned groups) {
  int s = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE);
  if (s < 0) return -1;
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_groups = groups};
  if (bind(s, (struct sockaddr*)&a, sizeof a) != 0) return -1;
  return s;
}

// Wait for a message of `type` on `s` (up to 2 s).
static int wait_msg(int s, int type, char* buf, int len) {
  struct pollfd p = {.fd = s, .events = POLLIN};
  while (poll(&p, 1, 2000) == 1) {
    int n = recv(s, buf, len, 0);
    if (n < (int)sizeof(struct nlmsghdr)) return -1;
    if (((struct nlmsghdr*)buf)->nlmsg_type == type) return n;
  }
  return -1;
}

// Send `req` and return the NLMSG_ERROR code of its ack.
static int ack(int s, struct nlmsghdr* req) {
  req->nlmsg_flags |= NLM_F_REQUEST | NLM_F_ACK;
  if (send(s, req, req->nlmsg_len, 0) != (int)req->nlmsg_len) return -1000;
  char buf[4096];
  if (wait_msg(s, NLMSG_ERROR, buf, sizeof buf) < 0) return -1001;
  return ((struct nlmsgerr*)NLMSG_DATA(buf))->error;
}

struct addr_req {
  struct nlmsghdr h;
  struct ifaddrmsg a;
  struct rtattr ra;
  unsigned char ip[4];
};

static void addr(struct addr_req* r, int type, const char* ip, int prefix) {
  memset(r, 0, sizeof *r);
  r->h.nlmsg_len = sizeof *r;
  r->h.nlmsg_type = type;
  r->h.nlmsg_seq = 7;
  r->a.ifa_family = AF_INET;
  r->a.ifa_prefixlen = prefix;
  r->a.ifa_index = 2;
  r->ra.rta_len = RTA_LENGTH(4);
  r->ra.rta_type = IFA_LOCAL;
  inet_pton(AF_INET, ip, r->ip);
}

static void netlink(void) {
  int ev = rtnl(RTMGRP_LINK | RTMGRP_IPV4_IFADDR);
  int s = rtnl(0);
  CHECK(ev >= 0 && s >= 0);
  struct sockaddr_nl me;
  socklen_t ml = sizeof me;
  CHECK(getsockname(ev, (struct sockaddr*)&me, &ml) == 0 && me.nl_family == AF_NETLINK &&
        me.nl_groups == (RTMGRP_LINK | RTMGRP_IPV4_IFADDR) && me.nl_pid != 0);
  // eth0 up: an RTM_NEWLINK to the link group.
  int i = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  struct ifreq r;
  memset(&r, 0, sizeof r);
  strcpy(r.ifr_name, "eth0");
  CHECK(ioctl(i, SIOCGIFFLAGS, &r) == 0);
  r.ifr_flags |= IFF_UP;
  CHECK(ioctl(i, SIOCSIFFLAGS, &r) == 0);
  char buf[8192];
  CHECK(wait_msg(ev, RTM_NEWLINK, buf, sizeof buf) > 0);
  struct ifinfomsg* li = NLMSG_DATA(buf);
  CHECK(li->ifi_index == 2 && (li->ifi_flags & IFF_UP));
  // An address by netlink: announced, dumped, and EEXIST again.
  struct addr_req a;
  addr(&a, RTM_NEWADDR, "10.0.2.15", 24);
  a.h.nlmsg_flags = NLM_F_CREATE | NLM_F_EXCL;
  CHECK(ack(s, &a.h) == 0);
  CHECK(wait_msg(ev, RTM_NEWADDR, buf, sizeof buf) > 0);
  struct ifaddrmsg* am = NLMSG_DATA(buf);
  CHECK(am->ifa_index == 2 && am->ifa_prefixlen == 24);
  CHECK(ack(s, &a.h) == -EEXIST);
  struct {
    struct nlmsghdr h;
    struct ifaddrmsg a;
  } dump = {{sizeof dump, RTM_GETADDR, NLM_F_REQUEST | NLM_F_DUMP, 9, 0}, {.ifa_family = AF_INET}};
  CHECK(send(s, &dump, sizeof dump, 0) == sizeof dump);
  int found = 0, done = 0;
  while (!done) {
    int n = recv(s, buf, sizeof buf, MSG_DONTWAIT);
    CHECK(n > 0);
    for (struct nlmsghdr* h = (struct nlmsghdr*)buf; NLMSG_OK(h, n); h = NLMSG_NEXT(h, n)) {
      CHECK(h->nlmsg_seq == 9);
      if (h->nlmsg_type == NLMSG_DONE) done = 1;
      if (h->nlmsg_type == RTM_NEWADDR && ((struct ifaddrmsg*)NLMSG_DATA(h))->ifa_index == 2) {
        found = 1;
      }
    }
  }
  CHECK(found);
  // The ioctls see it; deleting it twice is EADDRNOTAVAIL.
  CHECK(ioctl(i, SIOCGIFADDR, &r) == 0 &&
        ((struct sockaddr_in*)&r.ifr_addr)->sin_addr.s_addr == inet_addr("10.0.2.15"));
  addr(&a, RTM_DELADDR, "10.0.2.15", 24);
  CHECK(ack(s, &a.h) == 0);
  CHECK(wait_msg(ev, RTM_DELADDR, buf, sizeof buf) > 0);
  CHECK(ack(s, &a.h) == -EADDRNOTAVAIL);
  // Unknown requests are EOPNOTSUPP; other netlink families do not exist.
  struct nlmsghdr bad = {sizeof bad, RTM_NEWQDISC, 0, 1, 0};
  CHECK(ack(s, &bad) == -EOPNOTSUPP);
  CHECK(socket(AF_NETLINK, SOCK_RAW, NETLINK_AUDIT) == -1 && errno == EPROTONOSUPPORT);
  close(i);
  close(s);
  close(ev);
}

// SIOCSIFADDR in one process is announced to a listener in another.
static void across_processes(void) {
  int ev = rtnl(RTMGRP_IPV4_IFADDR);
  CHECK(ev >= 0);
  FORK_OR_SKIP(p);
  if (p == 0) {
    int i = socket(AF_INET, SOCK_DGRAM, 0);
    struct ifreq r;
    memset(&r, 0, sizeof r);
    strcpy(r.ifr_name, "eth0");
    struct sockaddr_in* sin = (struct sockaddr_in*)&r.ifr_addr;
    sin->sin_family = AF_INET;
    sin->sin_addr.s_addr = inet_addr("192.168.7.9");
    _exit(ioctl(i, SIOCSIFADDR, &r) == 0 ? 0 : 1);
  }
  int st;
  CHECK(waitpid(p, &st, 0) == p && WIFEXITED(st) && WEXITSTATUS(st) == 0);
  char buf[4096];
  CHECK(wait_msg(ev, RTM_NEWADDR, buf, sizeof buf) > 0);
  struct ifaddrmsg* am = NLMSG_DATA(buf);
  CHECK(am->ifa_index == 2 && am->ifa_prefixlen == 24);  // classful
  close(ev);
}

static void socket_options(void) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(s >= 0);
  unsigned mark = 0x10064, got = 0;
  socklen_t l = sizeof got;
  CHECK(getsockopt(s, SOL_SOCKET, SO_MARK, &got, &l) == 0 && got == 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_MARK, &mark, sizeof mark) == 0);
  CHECK(getsockopt(s, SOL_SOCKET, SO_MARK, &got, &l) == 0 && got == mark);
  int d = dup(s);
  CHECK(getsockopt(d, SOL_SOCKET, SO_MARK, &got, &l) == 0 && got == mark);
  CHECK(setsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, "lo", 3) == 0);
  char name[IFNAMSIZ] = "";
  l = sizeof name;
  CHECK(getsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, name, &l) == 0 && strcmp(name, "lo") == 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, "nope0", 6) == -1 && errno == ENODEV);
  // Bound to lo, the socket still reaches the loopback.
  struct sockaddr_in a = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  CHECK(bind(s, (struct sockaddr*)&a, sizeof a) == 0);
  l = sizeof a;
  CHECK(getsockname(s, (struct sockaddr*)&a, &l) == 0);
  CHECK(sendto(d, "x", 1, 0, (struct sockaddr*)&a, sizeof a) == 1);
  char c;
  CHECK(recv(s, &c, 1, 0) == 1 && c == 'x');
  close(d);
  close(s);
  // DhcpClient's socket: bound to eth0, a zero receive buffer (raised to
  // the minimum), then the client port.
  s = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
  CHECK(setsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, "eth0", 5) == 0);
  int on = 1, zero = 0;
  CHECK(setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_BROADCAST, &on, sizeof on) == 0);
  CHECK(setsockopt(s, SOL_SOCKET, SO_RCVBUF, &zero, sizeof zero) == 0);
  // libcore tries a v4-mapped IPv6 address first and falls back to
  // AF_INET on EAFNOSUPPORT.
  struct sockaddr_in6 mapped = {.sin6_family = AF_INET6, .sin6_port = htons(68)};
  mapped.sin6_addr.s6_addr[10] = mapped.sin6_addr.s6_addr[11] = 0xff;
  CHECK(bind(s, (struct sockaddr*)&mapped, sizeof mapped) == -1 && errno == EAFNOSUPPORT);
  CHECK(connect(s, (struct sockaddr*)&mapped, sizeof mapped) == -1 && errno == EAFNOSUPPORT);
  CHECK(sendto(s, "x", 1, 0, (struct sockaddr*)&mapped, sizeof mapped) == -1 && errno == EAFNOSUPPORT);
  struct sockaddr_in any = {.sin_family = AF_INET, .sin_port = htons(68)};
  CHECK(bind(s, (struct sockaddr*)&any, sizeof any) == 0);
  CHECK(bind(s, (struct sockaddr*)&any, sizeof any) == -1 && errno == EINVAL);
  struct sockaddr_in bound = {0};
  l = sizeof bound;
  CHECK(getsockname(s, (struct sockaddr*)&bound, &l) == 0 && bound.sin_family == AF_INET &&
        bound.sin_port == htons(68) && bound.sin_addr.s_addr == INADDR_ANY);
  close(s);
  // The port is eth0's, not the Mac's, which the test holds: a socket
  // without SO_REUSEADDR takes it all the same.
  s = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
  CHECK(setsockopt(s, SOL_SOCKET, SO_BINDTODEVICE, "eth0", 5) == 0);
  CHECK(bind(s, (struct sockaddr*)&any, sizeof any) == 0);
  close(s);
  int proto = -1;
  l = sizeof proto;
  s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  CHECK(getsockopt(s, SOL_SOCKET, SO_PROTOCOL, &proto, &l) == 0 && proto == IPPROTO_UDP);
  close(s);
  s = socket(AF_INET6, SOCK_STREAM | SOCK_CLOEXEC, 0);
  CHECK(getsockopt(s, SOL_SOCKET, SO_PROTOCOL, &proto, &l) == 0 && proto == IPPROTO_TCP);
  close(s);
  // The "have IPv4" probe: a datagram socket connects to port 0, and the
  // route chose a source address.
  s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  struct sockaddr_in lo = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  CHECK(connect(s, (struct sockaddr*)&lo, sizeof lo) == 0);
  struct sockaddr_in got_a;
  l = sizeof got_a;
  CHECK(getpeername(s, (struct sockaddr*)&got_a, &l) == 0 && got_a.sin_port == 0 &&
        got_a.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
  l = sizeof got_a;
  CHECK(getsockname(s, (struct sockaddr*)&got_a, &l) == 0 &&
        got_a.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
  close(s);
}

// An IPv4 ping socket receives the ICMP message without the IP header.
static void ping_socket(void) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, IPPROTO_ICMP);
  if (s < 0 && (errno == EACCES || errno == EPERM || errno == EPROTONOSUPPORT)) {
    printf("skip %s: no ping sockets on the host\n", __func__);
    skipped = 1;
    return;
  }
  CHECK(s >= 0);
  unsigned char echo[16] = {8, 0, 0, 0, 0x12, 0x34, 0, 1, 'a', 'i', 'm'};
  unsigned sum = 0;
  for (int i = 0; i < 16; i += 2) sum += echo[i] << 8 | echo[i + 1];
  while (sum >> 16) sum = (sum & 0xffff) + (sum >> 16);
  echo[2] = ~sum >> 8, echo[3] = ~sum & 0xff;
  struct sockaddr_in lo = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
  CHECK(sendto(s, echo, sizeof echo, 0, (struct sockaddr*)&lo, sizeof lo) == sizeof echo);
  struct pollfd p = {.fd = s, .events = POLLIN};
  unsigned char r[128];
  int n = 0;
  // Skip our own request, which the loopback may also deliver.
  while (poll(&p, 1, 2000) == 1 && (n = recv(s, r, sizeof r, 0)) > 0 && r[0] != 0) {
  }
  CHECK(n == sizeof echo && r[0] == 0 && memcmp(r + 8, "aim", 3) == 0);
  close(s);
}

// NetworkStack's DHCP filter (attachDhcpFilter).
static struct sock_filter dhcp_filter[] = {
    BPF_STMT(BPF_LD | BPF_B | BPF_ABS, 23),
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, IPPROTO_UDP, 0, 6),
    BPF_STMT(BPF_LD | BPF_H | BPF_ABS, 20),
    BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, 0x1fff, 4, 0),
    BPF_STMT(BPF_LDX | BPF_B | BPF_MSH, 14),
    BPF_STMT(BPF_LD | BPF_H | BPF_IND, 16),
    BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 68, 0, 1),
    BPF_STMT(BPF_RET | BPF_K, 0xffff),
    BPF_STMT(BPF_RET | BPF_K, 0),
};

static void dhcp(void) {
  int i = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  struct ifreq r0;
  memset(&r0, 0, sizeof r0);
  strcpy(r0.ifr_name, "eth0");
  r0.ifr_flags = IFF_UP | IFF_BROADCAST | IFF_MULTICAST;
  CHECK(ioctl(i, SIOCSIFFLAGS, &r0) == 0);
  close(i);
  int p = socket(AF_PACKET, SOCK_RAW | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
  CHECK(p >= 0);
  struct sock_fprog prog = {sizeof dhcp_filter / sizeof dhcp_filter[0], dhcp_filter};
  CHECK(setsockopt(p, SOL_SOCKET, SO_ATTACH_FILTER, &prog, sizeof prog) == 0);
  struct sockaddr_ll ll = {.sll_family = AF_PACKET, .sll_protocol = htons(ETH_P_IP), .sll_ifindex = 2};
  CHECK(bind(p, (struct sockaddr*)&ll, sizeof ll) == 0);
  // A DHCPDISCOVER from eth0's address, broadcast.
  unsigned char f[14 + 20 + 8 + 244];
  memset(f, 0, sizeof f);
  unsigned char mac[6] = {0x02, 0x61, 0x69, 0x6d, 0, 2};
  memset(f, 0xff, 6);
  memcpy(f + 6, mac, 6);
  f[12] = 8;
  unsigned char* ip = f + 14;
  ip[0] = 0x45;
  ip[2] = (20 + 8 + 244) >> 8;
  ip[3] = (20 + 8 + 244) & 0xff;
  ip[8] = 64;
  ip[9] = 17;
  memset(ip + 16, 0xff, 4);
  unsigned char* udp = ip + 20;
  udp[1] = 68;
  udp[3] = 67;
  udp[5] = 8 + 244;
  unsigned char* b = udp + 8;
  b[0] = 1, b[1] = 1, b[2] = 6;
  memcpy(b + 4, "\x12\x34\x56\x78", 4);
  memcpy(b + 28, mac, 6);
  memcpy(b + 236, "\x63\x82\x53\x63", 4);
  memcpy(b + 240, "\x35\x01\x01\xff", 4);
  ll.sll_halen = 6;
  memset(ll.sll_addr, 0xff, 6);
  if (sendto(p, f, sizeof f, 0, (struct sockaddr*)&ll, sizeof ll) == -1 && errno == ENETDOWN) {
    printf("skip %s: the host has no IPv4 default route\n", __func__);
    skipped = 1;
    close(p);
    return;
  }
  unsigned char r[2048];
  struct sockaddr_ll from;
  socklen_t fl = sizeof from;
  int n = recvfrom(p, r, sizeof r, 0, (struct sockaddr*)&from, &fl);
  CHECK(n > 14 + 20 + 8 + 240);
  CHECK(memcmp(r, mac, 6) == 0 && from.sll_ifindex == 2 && from.sll_protocol == htons(ETH_P_IP) &&
        from.sll_pkttype == PACKET_HOST);
  unsigned char* rb = r + 14 + 20 + 8;
  CHECK(rb[0] == 2 && memcmp(rb + 4, "\x12\x34\x56\x78", 4) == 0 && rb[16] != 0);  // yiaddr
  CHECK(rb[240] == 53 && rb[242] == 2);                                             // OFFER
  close(p);
}

int main(void) {
  RUN(interfaces);
  RUN(ioctls);
  RUN(netlink);
  RUN(across_processes);
  RUN(socket_options);
  RUN(ping_socket);
  RUN(dhcp);
  DONE();
}
