// IPv6 on eth0 (#267): the per-device configuration under
// /proc/sys/net/ipv6/conf, and address autoconfiguration: the link-local
// address, DAD, the IPv6 routes and their netlink messages. Runs as root
// (all capabilities).
#include <arpa/inet.h>
#include <dirent.h>
#include <fcntl.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <net/if.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

#include "check.h"

#define CONF "/proc/sys/net/ipv6/conf/"

// The file's contents, or "" with errno set.
static const char* get(const char* path) {
  static char buf[128];
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return "";
  ssize_t n = read(fd, buf, sizeof buf - 1);
  close(fd);
  buf[n < 0 ? 0 : n] = 0;
  return buf;
}

// write(2) of `value`: 0, or -errno.
static int put(const char* path, const char* value) {
  int fd = open(path, O_WRONLY | O_CLOEXEC);
  if (fd < 0) return -errno;
  ssize_t n = write(fd, value, strlen(value));
  int e = errno;
  close(fd);
  return n < 0 ? -e : 0;
}

static void sysctls(void) {
  DIR* d = opendir(CONF);
  CHECK(d);
  int all = 0, dflt = 0, lo = 0, eth0 = 0;
  for (struct dirent* e; (e = readdir(d));) {
    all |= strcmp(e->d_name, "all") == 0;
    dflt |= strcmp(e->d_name, "default") == 0;
    lo |= strcmp(e->d_name, "lo") == 0;
    eth0 |= strcmp(e->d_name, "eth0") == 0;
  }
  closedir(d);
  CHECK(all && dflt && lo && eth0);
  // ipv6_devconf_dflt's values; a device's mtu is its own.
  CHECK(strcmp(get(CONF "eth0/accept_ra"), "1\n") == 0);
  CHECK(strcmp(get(CONF "eth0/mtu"), "1500\n") == 0);
  CHECK(strcmp(get(CONF "all/accept_dad"), "0\n") == 0);
  CHECK(strcmp(get(CONF "default/accept_dad"), "1\n") == 0);
  struct stat st;
  CHECK(stat(CONF "eth0/accept_ra", &st) == 0 && S_ISREG(st.st_mode) &&
        (st.st_mode & 0777) == 0644);
  CHECK(stat(CONF "eth0/stable_secret", &st) == 0 && (st.st_mode & 0777) == 0600);
  CHECK(stat(CONF "eth0/no_such_knob", &st) < 0 && errno == ENOENT);
  // proc_dointvec's writes, with the handlers' ranges.
  CHECK(put(CONF "eth0/accept_ra", "2\n") == 0);
  CHECK(strcmp(get(CONF "eth0/accept_ra"), "2\n") == 0);
  CHECK(put(CONF "eth0/accept_ra", "two") == -EINVAL);
  CHECK(put(CONF "eth0/hop_limit", "0") == -EINVAL);
  CHECK(put(CONF "eth0/addr_gen_mode", "4") == -EINVAL);
  CHECK(put(CONF "eth0/mc_forwarding", "1") == -EACCES);
  // all's forwarding goes to default and every device.
  CHECK(put(CONF "all/forwarding", "1") == 0);
  CHECK(strcmp(get(CONF "eth0/forwarding"), "1\n") == 0);
  CHECK(strcmp(get(CONF "default/forwarding"), "1\n") == 0);
  CHECK(put(CONF "all/forwarding", "0") == 0);
  // The RFC 7217 secret: unset reads EIO; a write sets stable privacy.
  CHECK(open(CONF "eth0/stable_secret", O_RDONLY) < 0 && errno == EIO);
  CHECK(put(CONF "all/stable_secret", "2001:db8::1") == -EIO);
  CHECK(put(CONF "eth0/stable_secret", "bogus") == -EIO);
  CHECK(put(CONF "eth0/stable_secret", "2001:db8::1") == 0);
  CHECK(strcmp(get(CONF "eth0/stable_secret"), "2001:0db8:0000:0000:0000:0000:0000:0001\n") == 0);
  CHECK(strcmp(get(CONF "eth0/addr_gen_mode"), "2\n") == 0);
  CHECK(put(CONF "eth0/addr_gen_mode", "0") == 0);
  // The IPv4 and neighbour directories list the devices.
  CHECK(stat("/proc/sys/net/ipv4/conf/eth0", &st) == 0 && S_ISDIR(st.st_mode));
  CHECK(stat("/proc/sys/net/ipv6/neigh/eth0", &st) == 0 && S_ISDIR(st.st_mode));
}

// A NETLINK_ROUTE socket in `groups`.
static int rtnl(unsigned groups) {
  int s = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE);
  if (s < 0) return -1;
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_groups = groups};
  if (bind(s, (struct sockaddr*)&a, sizeof a) != 0) return -1;
  return s;
}

// Wait for a message of `type` on `s` (up to 2 s) that `want` accepts.
static int wait_msg(int s, int type, char* buf, int len, int (*want)(struct nlmsghdr*)) {
  struct pollfd p = {.fd = s, .events = POLLIN};
  while (poll(&p, 1, 2000) == 1) {
    int n = recv(s, buf, len, 0);
    if (n < (int)sizeof(struct nlmsghdr)) return -1;
    struct nlmsghdr* h = (struct nlmsghdr*)buf;
    if (h->nlmsg_type == type && (!want || want(h))) return n;
  }
  return -1;
}

// Send `req` and return the NLMSG_ERROR code of its ack.
static int ack(int s, struct nlmsghdr* req) {
  req->nlmsg_flags |= NLM_F_REQUEST | NLM_F_ACK;
  if (send(s, req, req->nlmsg_len, 0) != (int)req->nlmsg_len) return -1000;
  char buf[4096];
  if (wait_msg(s, NLMSG_ERROR, buf, sizeof buf, NULL) < 0) return -1001;
  return ((struct nlmsgerr*)NLMSG_DATA(buf))->error;
}

struct v6addr {
  struct in6_addr a;
  int plen, flags, scope;
};

// eth0's IPv6 addresses (RTM_GETADDR dump): how many, into `out`.
static int addrs6(struct v6addr* out, int max) {
  int s = rtnl(0);
  struct {
    struct nlmsghdr h;
    struct ifaddrmsg a;
  } d = {{sizeof d, RTM_GETADDR, NLM_F_REQUEST | NLM_F_DUMP, 3, 0}, {.ifa_family = AF_INET6}};
  send(s, &d, sizeof d, 0);
  char buf[16384];
  int n_out = 0, done = 0;
  while (!done) {
    int n = recv(s, buf, sizeof buf, 0);
    if (n <= 0) break;
    for (struct nlmsghdr* h = (struct nlmsghdr*)buf; NLMSG_OK(h, n); h = NLMSG_NEXT(h, n)) {
      if (h->nlmsg_type == NLMSG_DONE) done = 1;
      if (h->nlmsg_type != RTM_NEWADDR) continue;
      struct ifaddrmsg* am = NLMSG_DATA(h);
      if (am->ifa_index != 2 || n_out == max) continue;
      int len = IFA_PAYLOAD(h);
      struct v6addr v = {.plen = am->ifa_prefixlen, .flags = am->ifa_flags, .scope = am->ifa_scope};
      for (struct rtattr* ra = IFA_RTA(am); RTA_OK(ra, len); ra = RTA_NEXT(ra, len)) {
        if (ra->rta_type == IFA_ADDRESS) memcpy(&v.a, RTA_DATA(ra), 16);
        if (ra->rta_type == IFA_FLAGS) v.flags = *(unsigned*)RTA_DATA(ra);
      }
      out[n_out++] = v;
    }
  }
  close(s);
  return n_out;
}

struct v6route {
  struct in6_addr dst, gw;
  int plen, table, proto, type, metric, oif, has_gw;
};

// The IPv6 routes (RTM_GETROUTE dump): how many, into `out`.
static int routes6(struct v6route* out, int max) {
  int s = rtnl(0);
  struct {
    struct nlmsghdr h;
    struct rtmsg r;
  } d = {{sizeof d, RTM_GETROUTE, NLM_F_REQUEST | NLM_F_DUMP, 4, 0}, {.rtm_family = AF_INET6}};
  send(s, &d, sizeof d, 0);
  char buf[16384];
  int n_out = 0, done = 0;
  while (!done) {
    int n = recv(s, buf, sizeof buf, 0);
    if (n <= 0) break;
    for (struct nlmsghdr* h = (struct nlmsghdr*)buf; NLMSG_OK(h, n); h = NLMSG_NEXT(h, n)) {
      if (h->nlmsg_type == NLMSG_DONE) done = 1;
      if (h->nlmsg_type != RTM_NEWROUTE || n_out == max) continue;
      struct rtmsg* rm = NLMSG_DATA(h);
      struct v6route v = {.plen = rm->rtm_dst_len, .table = rm->rtm_table,
                          .proto = rm->rtm_protocol, .type = rm->rtm_type};
      int len = RTM_PAYLOAD(h);
      for (struct rtattr* ra = RTM_RTA(rm); RTA_OK(ra, len); ra = RTA_NEXT(ra, len)) {
        if (ra->rta_type == RTA_DST) memcpy(&v.dst, RTA_DATA(ra), 16);
        if (ra->rta_type == RTA_GATEWAY) memcpy(&v.gw, RTA_DATA(ra), 16), v.has_gw = 1;
        if (ra->rta_type == RTA_TABLE) v.table = *(int*)RTA_DATA(ra);
        if (ra->rta_type == RTA_PRIORITY) v.metric = *(int*)RTA_DATA(ra);
        if (ra->rta_type == RTA_OIF) v.oif = *(int*)RTA_DATA(ra);
      }
      out[n_out++] = v;
    }
  }
  close(s);
  return n_out;
}

static int has_route(struct v6route* r, int n, const char* dst, int plen, int table, int type) {
  struct in6_addr a;
  inet_pton(AF_INET6, dst, &a);
  for (int i = 0; i < n; i++) {
    if (r[i].plen == plen && !memcmp(&r[i].dst, &a, plen ? 16 : 0) && r[i].table == table &&
        r[i].type == type)
      return 1;
  }
  return 0;
}

static int set_up(int up) {
  int s = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  struct ifreq r;
  memset(&r, 0, sizeof r);
  strcpy(r.ifr_name, "eth0");
  int rc = ioctl(s, SIOCGIFFLAGS, &r);
  if (up) {
    r.ifr_flags |= IFF_UP;
  } else {
    r.ifr_flags &= ~IFF_UP;
  }
  if (rc == 0) rc = ioctl(s, SIOCSIFFLAGS, &r);
  close(s);
  return rc;
}

static int is_v6(struct nlmsghdr* h) {
  return ((struct ifaddrmsg*)NLMSG_DATA(h))->ifa_family == AF_INET6;
}

struct addr6_req {
  struct nlmsghdr h;
  struct ifaddrmsg a;
  struct rtattr ra;
  unsigned char ip[16];
  struct rtattr fa;
  unsigned flags;
};

static void addr6(struct addr6_req* q, int type, const char* ip, unsigned flags) {
  memset(q, 0, sizeof *q);
  q->h.nlmsg_len = sizeof *q;
  q->h.nlmsg_type = type;
  q->h.nlmsg_flags = type == RTM_NEWADDR ? NLM_F_CREATE | NLM_F_EXCL : 0;
  q->a.ifa_family = AF_INET6;
  q->a.ifa_prefixlen = 64;
  q->a.ifa_index = 2;
  q->ra.rta_len = RTA_LENGTH(16);
  q->ra.rta_type = IFA_ADDRESS;
  inet_pton(AF_INET6, ip, q->ip);
  q->fa.rta_len = RTA_LENGTH(4);
  q->fa.rta_type = IFA_FLAGS;
  q->flags = flags;
}

static int has_prefix_route(const char* prefix) {
  struct v6route r[32];
  int n = routes6(r, 32);
  struct in6_addr p;
  inet_pton(AF_INET6, prefix, &p);
  for (int i = 0; i < n; i++) {
    if (r[i].plen == 64 && !memcmp(&r[i].dst, &p, 16)) return 1;
  }
  return 0;
}

static void autoconf(void) {
  struct v6addr a[8];
  struct v6route r[32];
  struct in6_addr eui;
  inet_pton(AF_INET6, "fe80::61:69ff:fe6d:2", &eui);
  CHECK(addrs6(a, 8) == 0);
  int ev = rtnl(RTMGRP_IPV6_IFADDR | RTMGRP_IPV6_ROUTE);
  CHECK(ev >= 0);
  // Up with carrier: the EUI-64 link-local address, tentative while DAD
  // runs (a second), announced, with fe80::/64.
  CHECK(set_up(1) == 0);
  char buf[8192];
  CHECK(wait_msg(ev, RTM_NEWADDR, buf, sizeof buf, is_v6) > 0);
  CHECK(wait_msg(ev, RTM_NEWROUTE, buf, sizeof buf, NULL) > 0);
  CHECK(addrs6(a, 8) == 1 && !memcmp(&a[0].a, &eui, 16) && a[0].plen == 64 &&
        a[0].scope == RT_SCOPE_LINK && (a[0].flags & IFA_F_TENTATIVE) &&
        (a[0].flags & IFA_F_PERMANENT));
  int n = routes6(r, 32);
  CHECK(has_route(r, n, "fe80::", 64, RT_TABLE_MAIN, RTN_UNICAST));
  CHECK(has_route(r, n, "::1", 128, RT_TABLE_MAIN, RTN_UNICAST));
  CHECK(has_route(r, n, "::1", 128, RT_TABLE_LOCAL, RTN_LOCAL));
  CHECK(has_route(r, n, "ff00::", 8, RT_TABLE_LOCAL, RTN_MULTICAST));
  CHECK(!has_route(r, n, "fe80::61:69ff:fe6d:2", 128, RT_TABLE_LOCAL, RTN_LOCAL));
  usleep(1200 * 1000);
  CHECK(addrs6(a, 8) == 1 && !(a[0].flags & IFA_F_TENTATIVE));
  n = routes6(r, 32);
  CHECK(has_route(r, n, "fe80::61:69ff:fe6d:2", 128, RT_TABLE_LOCAL, RTN_LOCAL));
  // disable_ipv6 clears eth0's IPv6; enabling it configures it again,
  // now with a stable privacy address in the per-device table.
  CHECK(put(CONF "eth0/disable_ipv6", "1") == 0);
  CHECK(addrs6(a, 8) == 0);
  n = routes6(r, 32);
  CHECK(!has_route(r, n, "fe80::", 64, RT_TABLE_MAIN, RTN_UNICAST));
  CHECK(put(CONF "eth0/stable_secret", "2001:db8::7") == 0);
  CHECK(put(CONF "eth0/accept_ra_rt_table", "-1000") == 0);
  CHECK(put(CONF "eth0/dad_transmits", "0") == 0);
  CHECK(put(CONF "eth0/disable_ipv6", "0") == 0);
  CHECK(addrs6(a, 8) == 1 && memcmp(&a[0].a, &eui, 16) && a[0].a.s6_addr[0] == 0xfe &&
        (a[0].flags & IFA_F_STABLE_PRIVACY) && !(a[0].flags & IFA_F_TENTATIVE));
  struct v6addr stable = a[0];
  n = routes6(r, 32);
  int found = 0;
  for (int i = 0; i < n; i++) found |= r[i].table == 1002 && r[i].plen == 64 && r[i].oif == 2;
  CHECK(found);
  // The same secret and prefix give the same address again.
  CHECK(put(CONF "eth0/disable_ipv6", "1") == 0);
  CHECK(put(CONF "eth0/disable_ipv6", "0") == 0);
  CHECK(addrs6(a, 8) == 1 && !memcmp(&a[0].a, &stable.a, 16));
  // An address by netlink gets its prefix route; IFA_F_NOPREFIXROUTE none.
  int s = rtnl(0);
  struct addr6_req q;
  addr6(&q, RTM_NEWADDR, "2001:db8:5::5", 0);
  CHECK(ack(s, &q.h) == 0);
  CHECK(has_prefix_route("2001:db8:5::"));
  addr6(&q, RTM_DELADDR, "2001:db8:5::5", 0);
  CHECK(ack(s, &q.h) == 0);
  CHECK(!has_prefix_route("2001:db8:5::"));
  addr6(&q, RTM_NEWADDR, "2001:db8:5::5", IFA_F_NOPREFIXROUTE);
  CHECK(ack(s, &q.h) == 0);
  CHECK(!has_prefix_route("2001:db8:5::"));
  // With IPv6 disabled, an address is EACCES.
  CHECK(put(CONF "eth0/disable_ipv6", "1") == 0);
  addr6(&q, RTM_NEWADDR, "2001:db8:5::6", 0);
  CHECK(ack(s, &q.h) == -EACCES);
  CHECK(put(CONF "eth0/disable_ipv6", "0") == 0);
  // Down: the IPv6 addresses and routes go.
  CHECK(set_up(0) == 0);
  CHECK(addrs6(a, 8) == 0);
  n = routes6(r, 32);
  found = 0;
  for (int i = 0; i < n; i++) found |= r[i].oif == 2;
  CHECK(!found);
  CHECK(put(CONF "eth0/dad_transmits", "1") == 0);
  CHECK(put(CONF "eth0/accept_ra_rt_table", "0") == 0);
  CHECK(put(CONF "eth0/addr_gen_mode", "0") == 0);
  close(s);
  close(ev);
}

int main(void) {
  RUN(sysctls);
  RUN(autoconf);
  DONE();
}
