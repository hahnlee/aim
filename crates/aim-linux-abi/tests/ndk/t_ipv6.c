// IPv6 on eth0 (#267): the per-device configuration under
// /proc/sys/net/ipv6/conf. Runs as root (all capabilities).
#include <dirent.h>
#include <fcntl.h>
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
  // The IPv4 and neighbour directories list the devices.
  CHECK(stat("/proc/sys/net/ipv4/conf/eth0", &st) == 0 && S_ISDIR(st.st_mode));
  CHECK(stat("/proc/sys/net/ipv6/neigh/eth0", &st) == 0 && S_ISDIR(st.st_mode));
}

int main(void) {
  RUN(sysctls);
  DONE();
}
