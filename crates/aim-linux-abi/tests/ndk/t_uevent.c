// NETLINK_KOBJECT_UEVENT and the sysfs device trees, with a display
// server's evdev devices present. The socket is opened the way libcutils'
// uevent_open_socket does, and messages are checked the way
// uevent_kernel_recv checks them (source port 0, a group, SCM_CREDENTIALS).
// Runs as root (all capabilities).
#include <fcntl.h>
#include <linux/input.h>
#include <linux/netlink.h>
#include <poll.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define MUST(cond)                                                                        \
  do {                                                                                    \
    if (!(cond)) {                                                                        \
      printf("FAIL %s:%d: %s (errno %d %s)\n", __func__, __LINE__, #cond, errno, strerror(errno)); \
      exit(1);                                                                            \
    }                                                                                     \
  } while (0)

static const char* ATTR = "/sys/devices/virtual/input/input0/event0/uevent";

// input_print_bitmap: unsigned long words, high first, no leading zeros.
static void bitmap(char* out, const unsigned long* bits, size_t count) {
  while (count > 1 && bits[count - 1] == 0) count--;
  int at = sprintf(out, "%lx", bits[--count]);
  while (count) at += sprintf(out + at, " %lx", bits[--count]);
}

static int uevent_socket(unsigned pid, unsigned groups) {
  int s = socket(AF_NETLINK, SOCK_DGRAM | SOCK_CLOEXEC, NETLINK_KOBJECT_UEVENT);
  int on = 1, sz = 256 * 1024;
  struct sockaddr_nl a = {.nl_family = AF_NETLINK, .nl_pid = pid, .nl_groups = groups};
  MUST(s >= 0);
  MUST(setsockopt(s, SOL_SOCKET, SO_RCVBUF, &sz, sizeof sz) == 0);
  MUST(setsockopt(s, SOL_SOCKET, SO_PASSCRED, &on, sizeof on) == 0);
  if (bind(s, (struct sockaddr*)&a, sizeof a) < 0) {
    close(s);
    return -1;
  }
  return s;
}

struct received {
  int n;
  struct sockaddr_nl from;
  int has_cred;
  struct ucred cred;
  char buf[4096];
};

// The next message within `ms`, or n = -1.
static struct received receive(int s, int ms) {
  struct received r = {.n = -1};
  struct pollfd p = {.fd = s, .events = POLLIN};
  if (poll(&p, 1, ms) != 1) return r;
  char control[CMSG_SPACE(sizeof(struct ucred))];
  struct iovec iov = {r.buf, sizeof r.buf - 1};
  struct msghdr h = {&r.from, sizeof r.from, &iov, 1, control, sizeof control, 0};
  r.n = recvmsg(s, &h, 0);
  struct cmsghdr* c = CMSG_FIRSTHDR(&h);
  if (c != NULL && c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_CREDENTIALS) {
    r.has_cred = 1;
    memcpy(&r.cred, CMSG_DATA(c), sizeof r.cred);
  }
  return r;
}

// The value of variable `key` (with its '=') in the NUL-separated
// variables of message `r`, or NULL.
static const char* value(const struct received* r, const char* key) {
  for (int at = strlen(r->buf) + 1; at < r->n; at += strlen(r->buf + at) + 1) {
    if (strncmp(r->buf + at, key, strlen(key)) == 0) return r->buf + at + strlen(key);
  }
  return NULL;
}

// Whether message `r` has the variable `var` (KEY=VALUE).
static int has(const struct received* r, const char* var) {
  const char* eq = strchr(var, '=');
  char key[64];
  snprintf(key, sizeof key, "%.*s", (int)(eq - var + 1), var);
  const char* v = value(r, key);
  return v != NULL && strcmp(v, eq + 1) == 0;
}

static void sockets(void) {
  int s = uevent_socket(0, 0xffffffff);
  CHECK(s >= 0);
  // Existing devices are not replayed when a process subscribes.
  CHECK(receive(s, 50).n < 0);
  struct sockaddr_nl a;
  socklen_t len = sizeof a;
  CHECK(getsockname(s, (struct sockaddr*)&a, &len) == 0);
  // The first uevent socket gets the pid, whatever other families hold.
  CHECK(a.nl_family == AF_NETLINK && a.nl_pid == (unsigned)getpid() && a.nl_groups == 0xffffffff);
  int proto = 0;
  len = sizeof proto;
  CHECK(getsockopt(s, SOL_SOCKET, SO_PROTOCOL, &proto, &len) == 0 &&
        proto == NETLINK_KOBJECT_UEVENT);
  // Port ids are per protocol: the pid is taken for uevents only.
  CHECK(uevent_socket(getpid(), 1) < 0 && errno == EADDRINUSE);
  int r = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE);
  struct sockaddr_nl ra = {.nl_family = AF_NETLINK, .nl_pid = getpid()};
  CHECK(r >= 0 && bind(r, (struct sockaddr*)&ra, sizeof ra) == 0);
  close(r);
  close(s);
}

static void synthetic(void) {
  int s = uevent_socket(0, 0xffffffff);
  CHECK(s >= 0);
  int fd = open(ATTR, O_RDONLY);
  char buf[128] = {0};
  CHECK(fd >= 0 && read(fd, buf, sizeof buf) > 0);
  close(fd);
  CHECK(strcmp(buf, "MAJOR=13\nMINOR=64\nDEVNAME=input/event0\n") == 0);

  fd = open(ATTR, O_WRONLY);
  CHECK(fd >= 0 && write(fd, "add\n", 4) == 4);
  CHECK(write(fd, "bogus\n", 6) < 0 && errno == EINVAL);
  close(fd);
  struct received m = receive(s, 2000);
  CHECK(m.n > 0);
  CHECK(strcmp(m.buf, "add@/devices/virtual/input/input0/event0") == 0);
  CHECK(has(&m, "ACTION=add") && has(&m, "DEVPATH=/devices/virtual/input/input0/event0"));
  CHECK(has(&m, "SUBSYSTEM=input") && has(&m, "SYNTH_UUID=0"));
  CHECK(has(&m, "MAJOR=13") && has(&m, "MINOR=64") && has(&m, "DEVNAME=input/event0"));
  CHECK(value(&m, "SEQNUM=") != NULL);
  // From the kernel, to group 1, with the kernel's credentials.
  CHECK(m.from.nl_pid == 0 && m.from.nl_groups == 1);
  CHECK(m.has_cred && m.cred.pid == 0 && m.cred.uid == 0 && m.cred.gid == 0);

  // A second announcement has the next sequence number.
  fd = open(ATTR, O_WRONLY);
  CHECK(fd >= 0 && write(fd, "change 12345678-9abc-def0-1234-56789abcdef0 A=1", 47) == 47);
  close(fd);
  struct received c = receive(s, 2000);
  CHECK(c.n > 0 && strcmp(c.buf, "change@/devices/virtual/input/input0/event0") == 0);
  CHECK(has(&c, "SYNTH_UUID=12345678-9abc-def0-1234-56789abcdef0") && has(&c, "SYNTH_ARG_A=1"));
  unsigned long a = strtoul(value(&m, "SEQNUM="), NULL, 10);
  unsigned long b = strtoul(value(&c, "SEQNUM="), NULL, 10);
  CHECK(b == a + 1);
  // /sys/kernel/uevent_seqnum shows the last one taken.
  fd = open("/sys/kernel/uevent_seqnum", O_RDONLY);
  char seq[32] = {0};
  CHECK(fd >= 0 && read(fd, seq, sizeof seq - 1) > 0);
  close(fd);
  CHECK(strtoul(seq, NULL, 10) == b && seq[strlen(seq) - 1] == '\n');
  CHECK(open("/sys/kernel/uevent_seqnum", O_WRONLY) < 0);
  close(s);
}

// The input core's attributes describe the same live capabilities as evdev;
// absent event types have no corresponding variable (not an empty bitmap).
static void input_attributes(void) {
  for (int i = 0; i < 3; i++) {
    char path[128];
    snprintf(path, sizeof path, "/dev/input/event%d", i);
    int dev = open(path, O_RDONLY);
    MUST(dev >= 0);
    struct input_id id;
    char name[80];
    MUST(ioctl(dev, EVIOCGID, &id) == 0 && ioctl(dev, EVIOCGNAME(sizeof name), name) > 0);
    snprintf(path, sizeof path, "/sys/class/input/input%d/uevent", i);
    int fd = open(path, O_RDONLY);
    MUST(fd >= 0);
    struct received attr = {.n = 1};
    int n = read(fd, attr.buf + 1, sizeof attr.buf - 2);
    MUST(n > 0);
    close(fd);
    attr.n += n;
    // Use the message variable reader on the newline-separated attribute.
    for (int j = 1; j <= n; j++) if (attr.buf[j] == '\n') attr.buf[j] = 0;
    char want[1024];
    snprintf(want, sizeof want, "PRODUCT=%x/%x/%x/%x", id.bustype, id.vendor, id.product, id.version);
    CHECK(has(&attr, want));
    snprintf(want, sizeof want, "NAME=\"%s\"", name);
    CHECK(has(&attr, want));
    unsigned long ev = 0, prop = 0;
    MUST(ioctl(dev, EVIOCGBIT(0, sizeof ev), &ev) > 0);
    MUST(ioctl(dev, EVIOCGPROP(sizeof prop), &prop) > 0);
    snprintf(want, sizeof want, "PROP=%lx", prop);
    CHECK(has(&attr, want));
    snprintf(want, sizeof want, "EV=%lx", ev);
    CHECK(has(&attr, want));
    const int types[] = {EV_KEY, EV_REL, EV_ABS, EV_MSC, EV_LED, EV_SND, EV_FF, EV_SW};
    const char* keys[] = {"KEY=", "REL=", "ABS=", "MSC=", "LED=", "SND=", "FF=", "SW="};
    for (int j = 0; j < 8; j++) {
      const char* v = value(&attr, keys[j]);
      CHECK((v != NULL) == !!(ev & (1UL << types[j])));
      if (v) {
        unsigned long bits[KEY_CNT / (8 * sizeof(long))] = {0};
        int size = ioctl(dev, EVIOCGBIT(types[j], sizeof bits), bits);
        MUST(size > 0);
        bitmap(want, bits, (size + sizeof(long) - 1) / sizeof(long));
        CHECK(strcmp(v, want) == 0);
      }
    }
    CHECK(value(&attr, "MAJOR=") == NULL && value(&attr, "DEVNAME=") == NULL);
    CHECK(value(&attr, "PHYS=") == NULL && value(&attr, "UNIQ=") == NULL);
    const char* modalias = value(&attr, "MODALIAS=");
    CHECK(modalias && strncmp(modalias, "input:b0006v0000p0000e0001-e0,", 28) == 0);
    close(dev);

    int s = uevent_socket(0, 1);
    MUST(s >= 0);
    fd = open(path, O_WRONLY);
    MUST(fd >= 0 && write(fd, "add\n", 4) == 4);
    close(fd);
    struct received m = receive(s, 2000);
    CHECK(m.n > 0 && has(&m, "SYNTH_UUID=0"));
    for (int at = 1; at < attr.n; at += strlen(attr.buf + at) + 1) {
      CHECK(has(&m, attr.buf + at));
    }
    close(s);
  }
}

// Both processes must receive each owner transition exactly once with the
// same sequence numbers, including devices created and removed before a scan.
static void hotplug_messages(int s, unsigned long* seq, int count) {
  for (int j = 0; j < count; j++) {
    int added = j < 6;
    int index = (j % 6) / 2;
    int event = added ? j % 2 : !(j % 2);
    char path[128], want[160];
    snprintf(path, sizeof path, "/devices/virtual/input/input%d", index);
    if (event) snprintf(path + strlen(path), sizeof path - strlen(path), "/event%d", index);
    snprintf(want, sizeof want, "%s@%s", added ? "add" : "remove", path);
    struct received m = receive(s, 5000);
    MUST(m.n > 0 && strcmp(m.buf, want) == 0);
    MUST(m.from.nl_pid == 0 && m.from.nl_groups == 1);
    MUST(m.has_cred && m.cred.pid == 0 && m.cred.uid == 0 && m.cred.gid == 0);
    MUST(has(&m, "SUBSYSTEM=input") && value(&m, "SYNTH_UUID=") == NULL);
    if (event) {
      MUST(has(&m, "MAJOR=13"));
      snprintf(want, sizeof want, "MINOR=%d", 64 + index);
      MUST(has(&m, want));
      snprintf(want, sizeof want, "DEVNAME=input/event%d", index);
      MUST(has(&m, want));
    } else {
      MUST(has(&m, "PRODUCT=6/0/0/1") && value(&m, "NAME=") && value(&m, "EV="));
    }
    MUST(value(&m, "SEQNUM=") != NULL);
    seq[j] = strtoul(value(&m, "SEQNUM="), NULL, 10);
    if (j) MUST(seq[j] == seq[j - 1] + 1);
  }
}

static void hotplug(void) {
  int s = uevent_socket(0, 1), p[2];
  MUST(s >= 0 && pipe(p) == 0);
  pid_t child = fork();
  MUST(child >= 0);
  if (child == 0) {
    close(p[0]);
    close(s);
    s = uevent_socket(0, 1);
    MUST(s >= 0 && write(p[1], "!", 1) == 1);
    unsigned long seq[24];
    hotplug_messages(s, seq, 12);
    hotplug_messages(s, seq + 12, 12);
    MUST(write(p[1], seq, sizeof seq) == sizeof seq);
    close(s);
    _exit(0);
  }
  close(p[1]);
  char ack;
  MUST(read(p[0], &ack, 1) == 1);
  // fd 3 is a private control channel to the host test's Devices owner.
  MUST(write(3, "a", 1) == 1 && read(3, &ack, 1) == 1);
  unsigned long seq[24], other[24];
  // Ask for removal before consuming: add records still retain identity.
  MUST(write(3, "r", 1) == 1 && read(3, &ack, 1) == 1);
  hotplug_messages(s, seq, 12);
  MUST(write(3, "q", 1) == 1 && read(3, &ack, 1) == 1);
  hotplug_messages(s, seq + 12, 12);
  MUST(receive(s, 100).n < 0);
  MUST(read(p[0], other, sizeof other) == sizeof other);
  CHECK(memcmp(seq, other, sizeof seq) == 0);
  int fd = open("/sys/kernel/uevent_seqnum", O_RDONLY);
  char last[32] = {0};
  MUST(fd >= 0 && read(fd, last, sizeof last - 1) > 0);
  CHECK(strtoul(last, NULL, 10) == seq[23]);
  close(fd);
  int status;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  close(p[0]);
  close(s);
  CHECK(open(ATTR, O_RDONLY) < 0 && errno == ENOENT);
  CHECK(open("/sys/class/input/input0/uevent", O_RDONLY) < 0 && errno == ENOENT);
  close(3);
}

// A root process may send to the group; members other than the sender
// receive it from the sender's port with its credentials.
static void multicast(void) {
  int listener = uevent_socket(0, 1);
  int sender = uevent_socket(0, 1);
  CHECK(listener >= 0 && sender >= 0);
  struct sockaddr_nl own;
  socklen_t len = sizeof own;
  CHECK(getsockname(sender, (struct sockaddr*)&own, &len) == 0);
  struct sockaddr_nl group = {.nl_family = AF_NETLINK, .nl_groups = 1};
  CHECK(sendto(sender, "hello", 6, 0, (struct sockaddr*)&group, sizeof group) == 6);
  struct received m = receive(listener, 2000);
  CHECK(m.n == 6 && strcmp(m.buf, "hello") == 0);
  CHECK(m.from.nl_pid == own.nl_pid && m.from.nl_groups == 1);
  CHECK(m.has_cred && m.cred.pid == getpid() && m.cred.uid == getuid());
  CHECK(receive(sender, 100).n < 0);
  close(listener);
  close(sender);
}

// A uevent sent to the kernel (uevent_net_rcv): the kernel announces it
// to every member, with a sequence number, from port 0 and with the
// sender's credentials.
static void injected(void) {
  int s = uevent_socket(0, 1);
  CHECK(s >= 0);
  static const char body[] = "add@/devices/x\0ACTION=add\0DEVPATH=/devices/x\0SUBSYSTEM=x";
  struct {
    struct nlmsghdr h;
    char body[sizeof body];
  } req = {{NLMSG_LENGTH(sizeof body), 16, NLM_F_REQUEST, 1, 0}, {0}};
  memcpy(req.body, body, sizeof body);
  struct sockaddr_nl kernel = {.nl_family = AF_NETLINK};
  CHECK(sendto(s, &req, req.h.nlmsg_len, 0, (struct sockaddr*)&kernel, sizeof kernel) ==
        (ssize_t)req.h.nlmsg_len);
  struct received m = receive(s, 2000);
  CHECK(m.n > (int)sizeof body && memcmp(m.buf, body, sizeof body) == 0);
  CHECK(strncmp(m.buf + sizeof body, "SEQNUM=", 7) == 0);
  CHECK(m.from.nl_pid == 0 && m.from.nl_groups == 1);
  CHECK(m.has_cred && m.cred.pid == getpid());
  // A message that is not a request is not handled.
  CHECK(sendto(s, "x", 1, 0, (struct sockaddr*)&kernel, sizeof kernel) == 1);
  CHECK(receive(s, 100).n < 0);
  close(s);
}

// A device attribute opened before fork still synthesizes a kernel event.
static void inherited_attribute(void) {
  int s = uevent_socket(0, 1);
  int fd = open(ATTR, O_WRONLY | O_CLOEXEC);
  CHECK(s >= 0 && fd >= 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    errno = 0;
    if (pwrite(fd, "bogus", 5, 0) != -1 || errno != EINVAL) _exit(1);
    if (write(fd, "change", 6) != 6) _exit(2);
    _exit(0);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  struct received m = receive(s, 2000);
  CHECK(m.n > 0 && !strcmp(m.buf, "change@/devices/virtual/input/input0/event0"));
  CHECK(has(&m, "ACTION=change") && has(&m, "SUBSYSTEM=input"));
  CHECK(has(&m, "MAJOR=13") && has(&m, "DEVNAME=input/event0"));
  CHECK(m.from.nl_pid == 0 && m.has_cred && m.cred.pid == 0);
  close(fd);
  close(s);
}

// sysfs's device trees hold the modeled devices only, and create nothing.
static void device_trees(void) {
  struct stat st;
  // A value init wrote for a device that does not exist.
  CHECK(stat("/sys/class/android_usb", &st) < 0 && errno == ENOENT);
  CHECK(open("/sys/class/android_usb/android0/enable", O_WRONLY | O_CREAT, 0600) < 0 &&
        errno == ENOENT);
  CHECK(open("/sys/devices/virtual/input/input0/event0/x", O_WRONLY | O_CREAT, 0600) < 0 &&
        errno == EACCES);
  CHECK(open("/sys/devices/virtual/input/input0/event0/dev", O_WRONLY) < 0 && errno == EACCES);
  CHECK(open("/sys/devices/virtual/input/input9/event9/uevent", O_WRONLY) < 0 && errno == ENOENT);
  CHECK(stat(ATTR, &st) == 0 && S_ISREG(st.st_mode));
}

int main(int argc, char** argv) {
  if (argc == 2 && strcmp(argv[1], "hotplug") == 0) {
    RUN(hotplug);
    DONE();
  }
  RUN(sockets);
  RUN(synthetic);
  RUN(input_attributes);
  RUN(multicast);
  RUN(injected);
  RUN(inherited_attribute);
  RUN(device_trees);
  DONE();
}
