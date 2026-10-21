// NETLINK_AUDIT and NETLINK_GENERIC: the audit daemon's registration,
// status and userspace records (as logd's LogAudit uses them), and the
// generic netlink controller's family lookups (as wificond and libnl make
// them).
#include <linux/audit.h>
#include <linux/genetlink.h>
#include <linux/netlink.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

struct reply {
  struct nlmsghdr h;
  char body[8192];
};

static int nl(int proto) {
  int fd = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, proto);
  if (fd < 0) return -1;
  struct sockaddr_nl a = {.nl_family = AF_NETLINK};
  return bind(fd, (struct sockaddr*)&a, sizeof a) == 0 ? fd : -1;
}

static int send_msg(int fd, int type, int flags, const void* body, size_t len) {
  struct {
    struct nlmsghdr h;
    char body[512];
  } m = {.h = {.nlmsg_len = NLMSG_LENGTH(len), .nlmsg_type = type,
               .nlmsg_flags = NLM_F_REQUEST | flags, .nlmsg_seq = 7}};
  memcpy(m.body, body, len);
  struct sockaddr_nl k = {.nl_family = AF_NETLINK};
  return sendto(fd, &m, m.h.nlmsg_len, 0, (struct sockaddr*)&k, sizeof k) == (ssize_t)m.h.nlmsg_len
             ? 0
             : -1;
}

// The error of an NLMSG_ERROR reply (0 for an ack); -1000 for another type.
static int ack(int fd) {
  struct reply r;
  if (recv(fd, &r, sizeof r, 0) < (ssize_t)NLMSG_LENGTH(4)) return -1000;
  if (r.h.nlmsg_type != NLMSG_ERROR) return -1000;
  return ((struct nlmsgerr*)NLMSG_DATA(&r.h))->error;
}

static int audit_set_pid(int fd, unsigned pid) {
  struct audit_status s = {.mask = AUDIT_STATUS_PID, .pid = pid};
  if (send_msg(fd, AUDIT_SET, NLM_F_ACK, &s, sizeof s) != 0) return -1000;
  return ack(fd);
}

static void audit(void) {
  int fd = nl(NETLINK_AUDIT);
  CHECK(fd >= 0);
  struct audit_status s = {0};
  CHECK(send_msg(fd, AUDIT_GET, 0, &s, sizeof s) == 0);
  struct reply r;
  CHECK(recv(fd, &r, sizeof r, 0) > 0 && r.h.nlmsg_type == AUDIT_GET);
  struct audit_status* got = NLMSG_DATA(&r.h);
  CHECK(got->enabled == 0 && got->failure == 1 && got->pid == 0 && got->backlog_limit == 64);
  // Register as the audit daemon, as logd does.
  CHECK(audit_set_pid(fd, getpid()) == 0);
  CHECK(send_msg(fd, AUDIT_GET, 0, &s, sizeof s) == 0);
  CHECK(recv(fd, &r, sizeof r, 0) > 0 && got->pid == (unsigned)getpid());
  // Another process may not take over.
  FORK_OR_SKIP(child);
  if (child == 0) {
    int other = nl(NETLINK_AUDIT);
    _exit(other >= 0 && audit_set_pid(other, getpid()) == -EEXIST &&
                  audit_set_pid(other, 0) == -EACCES
              ? 0
              : 1);
  }
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  // A userspace AVC record reaches the daemon.
  const char text[] = "avc: denied { read } for test";
  CHECK(send_msg(fd, AUDIT_USER_AVC, 0, text, sizeof text) == 0);
  ssize_t n = recv(fd, &r, sizeof r, 0);
  CHECK(n > 0 && r.h.nlmsg_type == AUDIT_USER_AVC);
  CHECK(strstr(NLMSG_DATA(&r.h), "audit(") && strstr(NLMSG_DATA(&r.h), " msg='avc: denied { read } for test'"));
  // Obsolete and unknown requests.
  CHECK(send_msg(fd, AUDIT_LIST, NLM_F_ACK, &s, sizeof s) == 0 && ack(fd) == -EOPNOTSUPP);
  CHECK(send_msg(fd, 1300, NLM_F_ACK, &s, sizeof s) == 0 && ack(fd) == -EINVAL);
  CHECK(audit_set_pid(fd, 0) == 0);
  close(fd);
}

static void audit_permissions(void) {
  FORK_OR_SKIP(child);
  if (child == 0) {
    if (setgid(2000) != 0 || setuid(2000) != 0) _exit(2);
    int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_AUDIT);
    struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_groups = AUDIT_NLGRP_READLOG};
    int refused = bind(fd, (struct sockaddr*)&a, sizeof a) < 0 && errno == EPERM;
    a.nl_groups = 0;
    struct audit_status s = {0};
    _exit(refused && bind(fd, (struct sockaddr*)&a, sizeof a) == 0 &&
                  send_msg(fd, AUDIT_GET, NLM_F_ACK, &s, sizeof s) == 0 && ack(fd) == -EPERM
              ? 0
              : 1);
  }
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

// CTRL_CMD_GETFAMILY by name; the reply's family id, or -errno.
static int family(int fd, const char* name) {
  struct {
    struct genlmsghdr g;
    struct nlattr a;
    char name[32];
  } q = {.g = {.cmd = CTRL_CMD_GETFAMILY, .version = 1},
         .a = {.nla_len = NLA_HDRLEN + strlen(name) + 1, .nla_type = CTRL_ATTR_FAMILY_NAME}};
  strcpy(q.name, name);
  if (send_msg(fd, GENL_ID_CTRL, 0, &q, NLA_ALIGN(GENL_HDRLEN + q.a.nla_len)) != 0) return -1000;
  struct reply r;
  if (recv(fd, &r, sizeof r, 0) <= 0) return -1000;
  if (r.h.nlmsg_type == NLMSG_ERROR) return ((struct nlmsgerr*)NLMSG_DATA(&r.h))->error;
  if (r.h.nlmsg_type != GENL_ID_CTRL) return -1000;
  char* at = (char*)NLMSG_DATA(&r.h) + GENL_HDRLEN;
  char* end = (char*)&r + r.h.nlmsg_len;
  while (at + NLA_HDRLEN <= end) {
    struct nlattr* a = (struct nlattr*)at;
    if ((a->nla_type & NLA_TYPE_MASK) == CTRL_ATTR_FAMILY_ID) return *(unsigned short*)(at + NLA_HDRLEN);
    at += NLA_ALIGN(a->nla_len);
  }
  return -1000;
}

static void generic(void) {
  int fd = nl(NETLINK_GENERIC);
  CHECK(fd >= 0);
  CHECK(family(fd, "nlctrl") == GENL_ID_CTRL);
  // No Wi-Fi driver: no nl80211.
  CHECK(family(fd, "nl80211") == -ENOENT);
  close(fd);
}

int main(void) {
  RUN(audit);
  RUN(audit_permissions);
  RUN(generic);
  DONE();
}
