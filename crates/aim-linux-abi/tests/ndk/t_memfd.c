// memfds as Chromium's shared memory uses them: reopened through
// /proc/self/fd with another access mode, sealed, and passed to other
// processes by fork and SCM_RIGHTS. Seals belong to the memfd, not to the
// fd or the process.
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define PAGE 16384

static int reopen(int fd, int flags) {
  char path[64];
  snprintf(path, sizeof path, "/proc/self/fd/%d", fd);
  return open(path, flags | O_CLOEXEC);
}

static int link_is(int fd, const char* want) {
  char path[64], link[128];
  snprintf(path, sizeof path, "/proc/self/fd/%d", fd);
  ssize_t n = readlink(path, link, sizeof link - 1);
  if (n < 0) return 0;
  link[n] = 0;
  return !strcmp(link, want);
}

static int access_mode(int fd) { return fcntl(fd, F_GETFL) & O_ACCMODE; }

static void reopen_proc_self_fd(void) {
  int fd = memfd_create("chromium-ashmem", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0);
  CHECK(ftruncate(fd, PAGE) == 0);
  CHECK(link_is(fd, "/memfd:chromium-ashmem (deleted)"));
  char* w = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(w != MAP_FAILED);
  strcpy(w, "region");
  int ro = reopen(fd, O_RDONLY);
  CHECK(ro >= 0 && ro != fd);
  CHECK(access_mode(ro) == O_RDONLY && access_mode(fd) == O_RDWR);
  CHECK(link_is(ro, "/memfd:chromium-ashmem (deleted)"));
  // The same memory, through a description with its own offset.
  char b[7] = {0};
  CHECK(read(ro, b, 6) == 6 && !strcmp(b, "region"));
  CHECK(lseek(fd, 0, SEEK_CUR) == 0);
  const char* r = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, ro, 0);
  CHECK(r != MAP_FAILED);
  w[0] = 'R';
  CHECK(r[0] == 'R');
  // Read-only is read-only.
  errno = 0;
  CHECK(write(ro, "x", 1) == -1 && errno == EBADF);
  errno = 0;
  CHECK(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0) == MAP_FAILED &&
        errno == EACCES);
  errno = 0;
  CHECK(fcntl(ro, F_ADD_SEALS, F_SEAL_GROW) == -1 && errno == EPERM);
  // Seals added through one description show through the other.
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW) == 0);
  CHECK(fcntl(ro, F_GET_SEALS) == (F_SEAL_SHRINK | F_SEAL_GROW));
  int rw = reopen(fd, O_RDWR);
  CHECK(rw >= 0 && access_mode(rw) == O_RDWR);
  CHECK(fcntl(rw, F_ADD_SEALS, F_SEAL_FUTURE_WRITE) == 0);
  CHECK(fcntl(fd, F_GET_SEALS) == (F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_FUTURE_WRITE));
  errno = 0;
  CHECK(write(fd, "x", 1) == -1 && errno == EPERM);
  errno = 0;
  CHECK(ftruncate(rw, 0) == -1 && errno == EPERM);
  // O_TRUNC would shrink a shrink-sealed memfd.
  errno = 0;
  CHECK(reopen(fd, O_RDWR | O_TRUNC) == -1 && errno == EPERM);
  // /proc/<pid>/fd names the same thing.
  char path[64];
  snprintf(path, sizeof path, "/proc/%d/fd/%d", getpid(), fd);
  int again = open(path, O_RDONLY | O_CLOEXEC);
  CHECK(again >= 0 && fcntl(again, F_GET_SEALS) == fcntl(fd, F_GET_SEALS));
  close(again);
  close(rw);
  munmap((void*)r, PAGE);
  munmap(w, PAGE);
  close(ro);
  // A reopen outlives the fd it was opened through.
  int keep = reopen(fd, O_RDONLY);
  close(fd);
  CHECK(keep >= 0 && pread(keep, b, 6, 0) == 6 && !strcmp(b, "Region"));
  CHECK(fcntl(keep, F_GET_SEALS) == (F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_FUTURE_WRITE));
  close(keep);
}

static void seal_rules(void) {
  int fd = memfd_create("rules", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, 0x100) == -1 && errno == EINVAL);
  // F_SEAL_WRITE needs every writable shared mapping gone.
  char* m = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(m != MAP_FAILED);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EBUSY);
  munmap(m, PAGE);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
  errno = 0;
  CHECK(pwrite(fd, "x", 1, 0) == -1 && errno == EPERM);
  errno = 0;
  CHECK(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, 0, PAGE) == -1 &&
        errno == EPERM);
  CHECK(ftruncate(fd, 2 * PAGE) == 0);  // WRITE alone does not fix the size
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SEAL) == 0);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK) == -1 && errno == EPERM);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, 0) == -1 && errno == EPERM);
  CHECK(fcntl(fd, F_GET_SEALS) == (F_SEAL_WRITE | F_SEAL_SEAL));
  close(fd);
  // Without MFD_ALLOW_SEALING a memfd is born sealed.
  int plain = memfd_create("plain", MFD_CLOEXEC);
  CHECK(plain >= 0 && fcntl(plain, F_GET_SEALS) == F_SEAL_SEAL);
  close(plain);
  // A file that is not a memfd has no seals.
  int other = open("/proc/self/stat", O_RDONLY | O_CLOEXEC);
  errno = 0;
  CHECK(fcntl(other, F_GET_SEALS) == -1 && errno == EINVAL);
  close(other);
}

static void seals_across_dup(void) {
  int fd = memfd_create("dup", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0);
  int d = dup(fd);
  int d3 = fcntl(fd, F_DUPFD_CLOEXEC, 100);
  CHECK(d >= 0 && d3 >= 100);
  CHECK(fcntl(d, F_ADD_SEALS, F_SEAL_GROW) == 0);
  CHECK(fcntl(fd, F_GET_SEALS) == F_SEAL_GROW && fcntl(d3, F_GET_SEALS) == F_SEAL_GROW);
  close(fd);
  CHECK(fcntl(d3, F_ADD_SEALS, F_SEAL_SEAL) == 0);
  CHECK(fcntl(d, F_GET_SEALS) == (F_SEAL_GROW | F_SEAL_SEAL));
  CHECK(link_is(d3, "/memfd:dup (deleted)"));
  close(d);
  close(d3);
}

// A fork child sees the seals the parent set, and seals it adds are the
// parent's too.
static void seals_across_fork(void) {
  int fd = memfd_create("fork", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK) == 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    if (fcntl(fd, F_GET_SEALS) != F_SEAL_SHRINK) _exit(2);
    if (ftruncate(fd, 0) == 0 || errno != EPERM) _exit(3);
    if (!link_is(fd, "/memfd:fork (deleted)")) _exit(4);
    int ro = reopen(fd, O_RDONLY);
    if (ro < 0 || access_mode(ro) != O_RDONLY || fcntl(ro, F_GET_SEALS) != F_SEAL_SHRINK) _exit(5);
    if (fcntl(fd, F_ADD_SEALS, F_SEAL_FUTURE_WRITE) != 0) _exit(6);
    _exit(0);
  }
  int status = 0;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(fcntl(fd, F_GET_SEALS) == (F_SEAL_SHRINK | F_SEAL_FUTURE_WRITE));
  errno = 0;
  CHECK(write(fd, "x", 1) == -1 && errno == EPERM);
  close(fd);
}

static int send_fd(int sock, int fd) {
  char c = 'f';
  struct iovec iov = {&c, 1};
  char ctl[CMSG_SPACE(sizeof(int))];
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = ctl,
                     .msg_controllen = sizeof ctl};
  struct cmsghdr* h = CMSG_FIRSTHDR(&m);
  h->cmsg_level = SOL_SOCKET;
  h->cmsg_type = SCM_RIGHTS;
  h->cmsg_len = CMSG_LEN(sizeof(int));
  memcpy(CMSG_DATA(h), &fd, sizeof fd);
  return sendmsg(sock, &m, 0) == 1 ? 0 : -1;
}

static int recv_fd(int sock) {
  char c;
  struct iovec iov = {&c, 1};
  char ctl[CMSG_SPACE(sizeof(int))];
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = ctl,
                     .msg_controllen = sizeof ctl};
  if (recvmsg(sock, &m, MSG_CMSG_CLOEXEC) != 1) return -1;
  struct cmsghdr* h = CMSG_FIRSTHDR(&m);
  int fd = -1;
  if (h && h->cmsg_type == SCM_RIGHTS) memcpy(&fd, CMSG_DATA(h), sizeof fd);
  return fd;
}

// The child learns of the memfds only through the socket, after the fork:
// a read-only one sealed against writes, and one it seals itself.
static void seals_across_scm_rights(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) == 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    close(sv[0]);
    int ro = recv_fd(sv[1]);
    int rw = recv_fd(sv[1]);
    if (ro < 0 || rw < 0) _exit(2);
    if (fcntl(ro, F_GET_SEALS) != (F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_FUTURE_WRITE)) _exit(3);
    if (access_mode(ro) != O_RDONLY) _exit(4);
    if (!link_is(ro, "/memfd:chromium-ashmem (deleted)")) _exit(5);
    const char* p = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, ro, 0);
    if (p == MAP_FAILED || strcmp(p, "from parent")) _exit(6);
    if (mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0) != MAP_FAILED) _exit(7);
    int again = reopen(ro, O_RDWR);
    if (again < 0 || fcntl(again, F_GET_SEALS) != fcntl(ro, F_GET_SEALS)) _exit(8);
    if (write(again, "x", 1) != -1 || errno != EPERM) _exit(9);
    if (fcntl(rw, F_GET_SEALS) != 0) _exit(10);
    if (write(rw, "child", 5) != 5) _exit(11);
    if (fcntl(rw, F_ADD_SEALS, F_SEAL_WRITE | F_SEAL_SEAL) != 0) _exit(12);
    if (write(rw, "x", 1) != -1 || errno != EPERM) _exit(13);
    _exit(0);
  }
  close(sv[1]);
  int fd = memfd_create("chromium-ashmem", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  char* w = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(w != MAP_FAILED);
  strcpy(w, "from parent");
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_FUTURE_WRITE) == 0);
  int ro = reopen(fd, O_RDONLY);
  CHECK(ro >= 0);
  int other = memfd_create("other", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(other >= 0);
  CHECK(send_fd(sv[0], ro) == 0 && send_fd(sv[0], other) == 0);
  int status = 0;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status));
  CHECK(WEXITSTATUS(status) == 0);
  CHECK(fcntl(other, F_GET_SEALS) == (F_SEAL_WRITE | F_SEAL_SEAL));
  char b[6] = {0};
  CHECK(pread(other, b, 5, 0) == 5 && !strcmp(b, "child"));
  errno = 0;
  CHECK(pwrite(other, "x", 1, 0) == -1 && errno == EPERM);
  munmap(w, PAGE);
  close(fd);
  close(ro);
  close(other);
  close(sv[0]);
}

int main(void) {
  RUN(reopen_proc_self_fd);
  RUN(seal_rules);
  RUN(seals_across_dup);
  RUN(seals_across_fork);
  RUN(seals_across_scm_rights);
  DONE();
}
