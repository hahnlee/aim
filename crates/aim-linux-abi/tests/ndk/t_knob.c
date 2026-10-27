// fork(2)'s inherited open file descriptions retain their kernel handlers.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static int slurp(const char* path, char* buf, size_t size) {
  int fd = open(path, O_RDONLY | O_CLOEXEC);
  if (fd < 0) return -1;
  ssize_t n = read(fd, buf, size - 1);
  close(fd);
  if (n < 0) return -1;
  buf[n] = 0;
  return n;
}

static void ipv6_sysctl(void) {
  const char* path = "/proc/sys/net/ipv6/conf/default/hop_limit";
  char original[32];
  CHECK(slurp(path, original, sizeof original) > 0);
  int fd = open(path, O_WRONLY | O_CLOEXEC);
  CHECK(fd >= 0);
  int alias = dup(fd);
  CHECK(alias >= 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    close(fd);
    errno = 0;
    if (pwrite(alias, "0", 1, 0) != -1 || errno != EINVAL) _exit(1);
    struct iovec v[] = {{"12", 2}, {"3\n", 2}};
    if (writev(alias, v, 2) != 4) _exit(2);
    _exit(0);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  char value[32];
  CHECK(slurp(path, value, sizeof value) > 0 && !strcmp(value, "123\n"));
  CHECK(pwrite(fd, original, strlen(original), 0) == (ssize_t)strlen(original));
  close(alias);
  close(fd);
}

static void selinux_transaction(void) {
  int fd = open("/sys/fs/selinux/access", O_RDWR | O_CLOEXEC);
  CHECK(fd >= 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    const char* req = "u:r:servicemanager:s0 u:object_r:service_manager:s0 2 2";
    if (write(fd, req, strlen(req)) != (ssize_t)strlen(req)) _exit(1);
    char reply[64] = {0};
    unsigned allowed, decided;
    if (read(fd, reply, sizeof reply - 1) <= 0 ||
        sscanf(reply, "%x %x", &allowed, &decided) != 2 ||
        allowed != ~0u || decided != ~0u) _exit(2);
    _exit(0);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  // The reply is in the original shared file, with a shared offset too.
  char reply[64] = {0};
  CHECK(pread(fd, reply, sizeof reply - 1, 0) > 0 && !strncmp(reply, "ffffffff ", 9));
  CHECK(read(fd, reply, sizeof reply) == 0);
  close(fd);
}

static void proc_targets(void) {
  int oom = open("/proc/self/oom_score_adj", O_WRONLY | O_CLOEXEC);
  int comm = open("/proc/self/comm", O_WRONLY | O_CLOEXEC);
  int attr = open("/proc/self/attr/current", O_WRONLY | O_CLOEXEC);
  CHECK(oom >= 0 && comm >= 0 && attr >= 0);
  char original[32];
  CHECK(slurp("/proc/self/oom_score_adj", original, sizeof original) > 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    // The inherited fd still names the parent, not the child's /proc/self.
    errno = 0;
    if (write(oom, "1001", 4) != -1 || errno != EINVAL) _exit(1);
    if (write(oom, "123", 3) != 3) _exit(2);
    // proc_pid_comm requires the target in the writer's thread group.
    errno = 0;
    if (write(comm, "child", 5) != -1 || errno != EINVAL) _exit(3);
    errno = 0;
    if (write(attr, "u:r:child:s0", 12) != -1 || errno != EPERM) _exit(4);
    _exit(0);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  char value[32];
  CHECK(slurp("/proc/self/oom_score_adj", value, sizeof value) > 0 && atoi(value) == 123);
  CHECK(write(oom, original, strlen(original)) == (ssize_t)strlen(original));
  close(attr);
  close(comm);
  close(oom);
}

int main(void) {
  RUN(ipv6_sysctl);
  RUN(selinux_transaction);
  RUN(proc_targets);
  DONE();
}
