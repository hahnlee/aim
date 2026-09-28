// eth0 follows the Mac's network. The boot's test hook (argv[1], mapped
// to `<runtime>/net/simulate`) takes the network away, gives it back and
// changes it, without touching the Mac: netlink listeners see eth0 lose
// and regain carrier (once, however many processes watch), the virtual
// router leases the new values, and DHCP renewals over UDP are answered
// by the router, not the real network. Runs as root (all capabilities).
#include <arpa/inet.h>
#include <fcntl.h>
#include <linux/filter.h>
#include <linux/if_ether.h>
#include <linux/if_packet.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

// CHECK for helpers that return a value: a failure ends the program.
#define MUST(cond)                                                                        \
  do {                                                                                    \
    if (!(cond)) {                                                                        \
      printf("FAIL %s:%d: %s (errno %d %s)\n", __func__, __LINE__, #cond, errno, strerror(errno)); \
      exit(1);                                                                            \
    }                                                                                     \
  } while (0)

static const char* hook_path;
static const unsigned char mac[6] = {0x02, 0x61, 0x69, 0x6d, 0, 2};

static void hook(const char* text) {
  int f = open(hook_path, O_WRONLY | O_TRUNC | O_CREAT | O_CLOEXEC, 0644);
  CHECK(f >= 0 && write(f, text, strlen(text)) == (ssize_t)strlen(text));
  close(f);
}

static int rtnl_link(void) {
  int s = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE);
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_groups = RTMGRP_LINK};
  MUST(s >= 0 && bind(s, (struct sockaddr*)&a, sizeof a) == 0);
  return s;
}

// The flags of the next RTM_NEWLINK of eth0 within `ms`, or -1.
static int link_event(int s, int ms) {
  char buf[8192];
  struct pollfd p = {.fd = s, .events = POLLIN};
  while (poll(&p, 1, ms) == 1) {
    int n = recv(s, buf, sizeof buf, 0);
    struct nlmsghdr* h = (struct nlmsghdr*)buf;
    if (n < (int)NLMSG_LENGTH(sizeof(struct ifinfomsg)) || h->nlmsg_type != RTM_NEWLINK) continue;
    struct ifinfomsg* li = NLMSG_DATA(h);
    if (li->ifi_index == 2) return (int)li->ifi_flags;
  }
  return -1;
}

#define CARRIER (IFF_RUNNING | IFF_LOWER_UP)

static int ifflags(void) {
  int i = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  struct ifreq r;
  memset(&r, 0, sizeof r);
  strcpy(r.ifr_name, "eth0");
  MUST(ioctl(i, SIOCGIFFLAGS, &r) == 0);
  close(i);
  return (unsigned short)r.ifr_flags;
}

// A DHCP client message of `type` (BOOTP and options) into `b`: its length.
static int bootp(unsigned char* b, int type, in_addr_t ciaddr) {
  memset(b, 0, 244);
  b[0] = 1, b[1] = 1, b[2] = 6;
  memcpy(b + 4, "\x12\x34\x56\x78", 4);
  memcpy(b + 12, &ciaddr, 4);
  memcpy(b + 28, mac, 6);
  memcpy(b + 236, "\x63\x82\x53\x63", 4);
  b[240] = 53, b[241] = 1, b[242] = type, b[243] = 0xff;
  return 244;
}

// NetworkStack's DHCP filter (attachDhcpFilter): UDP to port 68.
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

static int packet_socket(void) {
  int p = socket(AF_PACKET, SOCK_RAW | SOCK_CLOEXEC, 0);
  MUST(p >= 0);
  struct sock_fprog prog = {sizeof dhcp_filter / sizeof dhcp_filter[0], dhcp_filter};
  MUST(setsockopt(p, SOL_SOCKET, SO_ATTACH_FILTER, &prog, sizeof prog) == 0);
  struct sockaddr_ll ll = {.sll_family = AF_PACKET, .sll_protocol = htons(ETH_P_IP), .sll_ifindex = 2};
  MUST(bind(p, (struct sockaddr*)&ll, sizeof ll) == 0);
  return p;
}

// Broadcast a DHCPDISCOVER on the packet socket: sendto's result.
static int discover(int p) {
  unsigned char f[14 + 20 + 8 + 244];
  memset(f, 0, sizeof f);
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
  udp[1] = 68, udp[3] = 67, udp[5] = 8 + 244;
  bootp(udp + 8, 1, 0);
  struct sockaddr_ll ll = {.sll_family = AF_PACKET, .sll_protocol = htons(ETH_P_IP), .sll_ifindex = 2, .sll_halen = 6};
  memset(ll.sll_addr, 0xff, 6);
  return sendto(p, f, sizeof f, 0, (struct sockaddr*)&ll, sizeof ll);
}

// The router's next reply on the packet socket (its BOOTP message).
static unsigned char reply[2048];
static unsigned char* router_reply(int p) {
  struct pollfd pf = {.fd = p, .events = POLLIN};
  MUST(poll(&pf, 1, 2000) == 1);
  int n = recv(p, reply, sizeof reply, 0);
  MUST(n > 14 + 20 + 8 + 240 && memcmp(reply + 6, "\x02\x61\x69\x6d\x00\x01", 6) == 0);
  return reply + 14 + 20 + 8;
}

// Option `code` of a BOOTP message (its value), or NULL.
static unsigned char* option(unsigned char* b, int code) {
  for (int at = 240; at < 1400 && b[at] != 0xff; at += 2 + b[at + 1]) {
    if (b[at] == code) return b + at + 2;
  }
  return NULL;
}

static void carrier(void) {
  int ev = rtnl_link();
  int i = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  struct ifreq r;
  memset(&r, 0, sizeof r);
  strcpy(r.ifr_name, "eth0");
  r.ifr_flags = IFF_UP | IFF_BROADCAST | IFF_MULTICAST;
  CHECK(ioctl(i, SIOCSIFFLAGS, &r) == 0);
  close(i);
  int f = link_event(ev, 5000);
  CHECK(f >= 0 && (f & IFF_UP));
  if (!(f & CARRIER)) {
    printf("skip %s: the host has no IPv4 default route\n", __func__);
    skipped = 1;
    return;
  }
  // Another process watches too: the loss is still announced once.
  int ready[2], done[2];
  CHECK(pipe(ready) == 0 && pipe(done) == 0);
  FORK_OR_SKIP(child);
  if (child == 0) {
    int s = rtnl_link();
    char c = 1;
    write(ready[1], &c, 1);
    read(done[0], &c, 1);
    close(s);
    _exit(0);
  }
  char c;
  CHECK(read(ready[0], &c, 1) == 1);
  hook("down\n");
  f = link_event(ev, 10000);
  CHECK(f >= 0 && (f & IFF_UP) && !(f & CARRIER));
  CHECK(link_event(ev, 5000) == -1);
  CHECK((ifflags() & (IFF_UP | IFF_RUNNING)) == IFF_UP);
  int p = packet_socket();
  CHECK(discover(p) == -1 && errno == ENETDOWN);
  write(done[1], &c, 1);
  int st;
  CHECK(waitpid(child, &st, 0) == child && WIFEXITED(st));
  // Back.
  hook("");
  f = link_event(ev, 10000);
  CHECK(f >= 0 && (f & CARRIER) == CARRIER);
  CHECK(ifflags() & IFF_RUNNING);
  CHECK(discover(p) > 0);
  unsigned char* b = router_reply(p);
  CHECK(b[242] == 2);  // OFFER
  // Another network: carrier goes and comes back, and the lease is new.
  hook("addr 10.11.12.13/24\ngateway 10.11.12.1\ndns 9.9.9.9\nlease 120\n");
  f = link_event(ev, 10000);
  CHECK(f >= 0 && !(f & CARRIER));
  f = link_event(ev, 1000);
  CHECK(f >= 0 && (f & CARRIER) == CARRIER);
  CHECK(discover(p) > 0);
  b = router_reply(p);
  unsigned char* o;
  CHECK(memcmp(b + 16, "\x0a\x0b\x0c\x0d", 4) == 0);  // yiaddr
  CHECK((o = option(b, 3)) && memcmp(o, "\x0a\x0b\x0c\x01", 4) == 0);
  CHECK((o = option(b, 6)) && memcmp(o, "\x09\x09\x09\x09", 4) == 0);
  CHECK((o = option(b, 51)) && memcmp(o, "\x00\x00\x00\x78", 4) == 0);
  close(p);
  close(ev);
}

// DhcpClient's renewals: a UDP socket bound to eth0 and port 68, connected
// to the server (RENEWING), or broadcasting (REBINDING).
static void renewal(void) {
  hook("addr 10.11.12.13/24\ngateway 10.11.12.1\nlease 120\n");
  int p = packet_socket();
  int u = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, IPPROTO_UDP);
  int on = 1;
  CHECK(setsockopt(u, SOL_SOCKET, SO_BINDTODEVICE, "eth0", 5) == 0);
  CHECK(setsockopt(u, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  CHECK(setsockopt(u, SOL_SOCKET, SO_BROADCAST, &on, sizeof on) == 0);
  struct sockaddr_in any = {.sin_family = AF_INET, .sin_port = htons(68)};
  CHECK(bind(u, (struct sockaddr*)&any, sizeof any) == 0);
  struct sockaddr_in server = {.sin_family = AF_INET, .sin_port = htons(67)};
  server.sin_addr.s_addr = inet_addr("10.11.12.1");
  CHECK(connect(u, (struct sockaddr*)&server, sizeof server) == 0);
  struct sockaddr_in peer;
  socklen_t pl = sizeof peer;
  CHECK(getpeername(u, (struct sockaddr*)&peer, &pl) == 0 && peer.sin_port == htons(67) &&
        peer.sin_addr.s_addr == server.sin_addr.s_addr);
  unsigned char m[244];
  int n = bootp(m, 3, inet_addr("10.11.12.13"));
  CHECK(write(u, m, n) == n);
  unsigned char* b = router_reply(p);
  unsigned char* o;
  CHECK(b[242] == 5 && memcmp(b + 16, "\x0a\x0b\x0c\x0d", 4) == 0);  // ACK
  CHECK((o = option(b, 51)) && memcmp(o, "\x00\x00\x00\x78", 4) == 0);
  // The Mac moved on: the old address is NAKed, by broadcast too.
  hook("addr 10.11.12.14/24\ngateway 10.11.12.1\n");
  CHECK(write(u, m, n) == n);
  b = router_reply(p);
  CHECK(b[242] == 6);  // NAK
  int r = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, IPPROTO_UDP);
  CHECK(setsockopt(r, SOL_SOCKET, SO_BINDTODEVICE, "eth0", 5) == 0);
  CHECK(setsockopt(r, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on) == 0);
  CHECK(setsockopt(r, SOL_SOCKET, SO_BROADCAST, &on, sizeof on) == 0);
  CHECK(bind(r, (struct sockaddr*)&any, sizeof any) == 0);
  struct sockaddr_in all = {.sin_family = AF_INET, .sin_port = htons(67), .sin_addr.s_addr = INADDR_BROADCAST};
  n = bootp(m, 3, inet_addr("10.11.12.14"));
  CHECK(sendto(r, m, n, 0, (struct sockaddr*)&all, sizeof all) == n);
  b = router_reply(p);
  CHECK(b[242] == 5 && memcmp(b + 16, "\x0a\x0b\x0c\x0e", 4) == 0);
  // Without a network there is no route to the server.
  hook("down\n");
  CHECK(write(u, m, n) == -1 && errno == ENETUNREACH);
  hook("");
  close(r);
  close(u);
  close(p);
}

int main(int argc, char** argv) {
  if (argc != 2) return 2;
  hook_path = argv[1];
  RUN(carrier);
  RUN(renewal);
  DONE();
}
