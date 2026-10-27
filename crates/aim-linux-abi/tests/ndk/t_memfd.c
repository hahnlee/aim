// memfds as Chromium's shared memory uses them: reopened through
// /proc/self/fd with another access mode, sealed, and passed to other
// processes by fork and SCM_RIGHTS. Seals belong to the memfd, not to the
// fd or the process.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/file.h>
#include <sys/stat.h>
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

// What a write seal does to mappings (mm/memfd.c, mm/mmap.c): a read-only
// shared view of a sealed memfd can never be made writable, and any shared
// view of a writable fd, even read-only, may be written, so it keeps
// F_SEAL_WRITE out (EBUSY).
static void seals_and_views(void) {
  int fd = memfd_create("views", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  char* ro = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
  CHECK(ro != MAP_FAILED);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EBUSY);
  // A private view never writes the file.
  munmap(ro, PAGE);
  char* priv = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
  CHECK(priv != MAP_FAILED);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
  munmap(priv, PAGE);
  ro = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
  CHECK(ro != MAP_FAILED);
  errno = 0;
  CHECK(mprotect(ro, PAGE, PROT_READ | PROT_WRITE) == -1 && errno == EACCES);
  CHECK(mprotect(ro, PAGE, PROT_NONE) == 0);
  CHECK(mprotect(ro, PAGE, PROT_READ) == 0);
  errno = 0;
  CHECK(mprotect(ro, PAGE, PROT_READ | PROT_WRITE | PROT_EXEC) == -1 && errno == EACCES);
  // Conversion to executable-capable memory keeps the write denial.
  CHECK(mprotect(ro, PAGE, PROT_READ | PROT_EXEC) == 0);
  errno = 0;
  CHECK(mprotect(ro, PAGE, PROT_READ | PROT_WRITE) == -1 && errno == EACCES);
  munmap(ro, PAGE);
  close(fd);
  // F_SEAL_FUTURE_WRITE alike, and a view mapped before the seal stays
  // writable.
  fd = memfd_create("future", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  char* before = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
  CHECK(before != MAP_FAILED);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_FUTURE_WRITE) == 0);
  ro = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
  CHECK(ro != MAP_FAILED);
  errno = 0;
  CHECK(mprotect(ro, PAGE, PROT_READ | PROT_WRITE) == -1 && errno == EACCES);
  CHECK(mprotect(before, PAGE, PROT_READ | PROT_WRITE) == 0);
  before[0] = 'b';
  CHECK(ro[0] == 'b');
  munmap(ro, PAGE);
  munmap(before, PAGE);
  close(fd);
  // A read-only description's executable alias cannot write the file.
  fd = memfd_create("readonly-exec", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  int readonly = reopen(fd, O_RDONLY);
  CHECK(readonly >= 0);
  void* rx = mmap(NULL, PAGE, PROT_READ | PROT_EXEC, MAP_SHARED, readonly, 0);
  CHECK(rx != MAP_FAILED);
  errno = 0;
  CHECK(mprotect(rx, PAGE, PROT_READ | PROT_WRITE) == -1 && errno == EACCES);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
  munmap(rx, PAGE);
  close(readonly);
  close(fd);
}

// A view that may be written keeps F_SEAL_WRITE out in whichever process
// it is (i_mmap_writable belongs to the file): one a fork child maps
// itself, one it inherited, until it goes or the child exits.
static void views_in_other_processes(void) {
  int fd = memfd_create("elsewhere", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  int up[2], down[2];
  CHECK(pipe(up) == 0 && pipe(down) == 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    char c;
    char* v = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
    if (v == MAP_FAILED || write(up[1], "m", 1) != 1) _exit(2);
    if (read(down[0], &c, 1) != 1 || munmap(v, PAGE) != 0) _exit(3);
    if (write(up[1], "u", 1) != 1) _exit(4);
    _exit(0);
  }
  char c;
  CHECK(read(up[0], &c, 1) == 1);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EBUSY);
  CHECK(write(down[1], "g", 1) == 1 && read(up[0], &c, 1) == 1);
  int status = 0;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  // The child's view went before it exited.
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
  close(fd);
  // A view the child inherits keeps the seal out after the parent's goes.
  fd = memfd_create("inherited", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  char* v = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(v != MAP_FAILED);
  FORK_OR_SKIP(child);
  if (child == 0) {
    if (write(up[1], "f", 1) != 1 || read(down[0], &c, 1) != 1) _exit(2);
    v[0] = 'c';
    _exit(0);
  }
  CHECK(read(up[0], &c, 1) == 1 && munmap(v, PAGE) == 0);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EBUSY);
  CHECK(write(down[1], "g", 1) == 1);
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(pread(fd, &c, 1, 0) == 1 && c == 'c');
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
  close(fd);
  // An executable view is backed by shared anonymous memory on Darwin.
  // It still belongs to this memfd for i_mmap_writable.
  fd = memfd_create("executable-view", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
  FORK_OR_SKIP(executable);
  if (executable == 0) {
    char* rx = mmap(NULL, PAGE, PROT_READ | PROT_EXEC, MAP_SHARED, fd, 0);
    if (rx == MAP_FAILED || write(up[1], "x", 1) != 1) _exit(2);
    close(fd);
    if (read(down[0], &c, 1) != 1) _exit(3);
    _exit(0);
  }
  CHECK(read(up[0], &c, 1) == 1);
  errno = 0;
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == -1 && errno == EBUSY);
  CHECK(write(down[1], "g", 1) == 1);
  CHECK(waitpid(executable, &status, 0) == executable && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE) == 0);
  close(fd);
  close(up[0]);
  close(up[1]);
  close(down[0]);
  close(down[1]);
}

static void seal_races_mmap(void) {
  for (int i = 0; i < 24; ++i) {
    int fd = memfd_create("race", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    int sv[2];
    CHECK(fd >= 0 && ftruncate(fd, PAGE) == 0);
    CHECK(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) == 0);
    FORK_OR_SKIP(pid);
    if (pid == 0) {
      char c;
      close(sv[0]);
      if (read(sv[1], &c, 1) != 1) _exit(2);
      char* v = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
      c = v != MAP_FAILED ? 'm' : errno == EPERM ? 's' : 'e';
      if (write(sv[1], &c, 1) != 1 || read(sv[1], &c, 1) != 1) _exit(3);
      _exit(0);
    }
    close(sv[1]);
    CHECK(write(sv[0], "g", 1) == 1);
    int sealed = fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE);
    int error = errno;
    char c;
    CHECK(read(sv[0], &c, 1) == 1);
    CHECK((sealed == 0 && c == 's') || (sealed == -1 && error == EBUSY && c == 'm'));
    CHECK(write(sv[0], "g", 1) == 1);
    int status;
    CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    close(sv[0]);
    close(fd);
  }
}

// F_SEAL_EXEC: the exec bits cannot change (shmem_setattr); the rest can.
static void exec_seal_chmod(void) {
  int fd = memfd_create("exec", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0);
  struct stat st;
  CHECK(fstat(fd, &st) == 0);
  CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_EXEC) == 0);
  errno = 0;
  CHECK(fchmod(fd, st.st_mode & 0666) == -1 && errno == EPERM);
  CHECK(fchmod(fd, (st.st_mode & 07777) & ~0060) == 0);
  // A separate description and a procfs chmod see the inode's seal too.
  int again = reopen(fd, O_RDONLY);
  CHECK(again >= 0);
  errno = 0;
  CHECK(fchmod(again, 0600) == -1 && errno == EPERM);
  char path[64];
  snprintf(path, sizeof path, "/proc/self/fd/%d", again);
  errno = 0;
  CHECK(chmod(path, 0600) == -1 && errno == EPERM);
  CHECK(ftruncate(fd, PAGE) == -1 && errno == EPERM);
  close(again);
  close(fd);
  int noexec = memfd_create("noexec", MFD_CLOEXEC | MFD_NOEXEC_SEAL);
  CHECK(noexec >= 0);
  errno = 0;
  CHECK(fchmod(noexec, 0755) == -1 && errno == EPERM);
  CHECK(fchmod(noexec, 0600) == 0);
  CHECK(fstat(noexec, &st) == 0 && (st.st_mode & 0777) == 0600);
  // F_SEAL_EXEC protects mode bits; it does not deny executable mmap.
  CHECK(ftruncate(noexec, PAGE) == 0);
  void* rx = mmap(NULL, PAGE, PROT_READ | PROT_EXEC, MAP_SHARED, noexec, 0);
  CHECK(rx != MAP_FAILED);
  munmap(rx, PAGE);
  close(noexec);
}

static void flock_and_lifetime(void) {
  int fd = memfd_create("flock", MFD_CLOEXEC | MFD_ALLOW_SEALING);
  CHECK(fd >= 0);
  CHECK(flock(fd, LOCK_UN) == 0);
  // Make the file eligible for the periodic sweep, then force a sweep.
  sleep(6);
  for (int i = 0; i < 64; ++i) {
    int sweep = memfd_create("sweep", MFD_CLOEXEC);
    CHECK(sweep >= 0);
    close(sweep);
  }
  int other = reopen(fd, O_RDWR);
  CHECK(other >= 0);
  // Merely reopening a file takes no guest lock.
  CHECK(flock(fd, LOCK_EX | LOCK_NB) == 0);
  errno = 0;
  CHECK(flock(other, LOCK_EX | LOCK_NB) == -1 && errno == EWOULDBLOCK);
  int copy = dup(fd);
  CHECK(copy >= 0);
  close(fd);
  errno = 0;
  CHECK(flock(other, LOCK_EX | LOCK_NB) == -1 && errno == EWOULDBLOCK);
  close(copy);
  // The keeper must release the last closed description's lock first.
  CHECK(flock(other, LOCK_EX | LOCK_NB) == 0);
  CHECK(flock(other, LOCK_UN) == 0);
  int record = reopen(other, O_RDWR);
  CHECK(record >= 0);
  struct flock lock = {.l_type = F_WRLCK, .l_whence = SEEK_SET};
  CHECK(fcntl(record, F_OFD_SETLK, &lock) == 0);
  lock.l_type = F_WRLCK;
  errno = 0;
  CHECK(fcntl(other, F_OFD_SETLK, &lock) == -1 && errno == EAGAIN);
  close(record);
  CHECK(fcntl(other, F_OFD_SETLK, &lock) == 0);
  close(other);

  // A description queued in a socket still references the memfd.
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) == 0);
  fd = memfd_create("queued", MFD_CLOEXEC);
  CHECK(fd >= 0 && flock(fd, LOCK_EX | LOCK_NB) == 0);
  CHECK(send_fd(sv[0], fd) == 0);
  close(fd);
  for (int i = 0; i < 64; ++i) {
    int sweep = memfd_create("sweep", MFD_CLOEXEC);
    CHECK(sweep >= 0);
    close(sweep);
  }
  fd = recv_fd(sv[1]);
  CHECK(fd >= 0);
  other = reopen(fd, O_RDWR);
  CHECK(other >= 0);
  errno = 0;
  CHECK(flock(other, LOCK_EX | LOCK_NB) == -1 && errno == EWOULDBLOCK);
  CHECK(flock(fd, LOCK_UN) == 0);
  CHECK(flock(other, LOCK_EX | LOCK_NB) == 0);
  close(other);
  close(fd);
  close(sv[0]);
  close(sv[1]);
}

static int scope;

// The exec bits of a memfd made with `flags`, and its seals; -1 if
// memfd_create failed (errno kept).
static int make(unsigned flags, int* seals) {
  int fd = memfd_create("noexec", flags);
  if (fd < 0) return -1;
  struct stat st;
  int exec = fstat(fd, &st) == 0 ? (int)(st.st_mode & 0111) : -2;
  *seals = fcntl(fd, F_GET_SEALS);
  close(fd);
  return exec;
}

static void noexec_sysctl_read(void) {
  char value[8] = {0};
  int fd = open("/proc/sys/vm/memfd_noexec", O_RDONLY | O_CLOEXEC);
  CHECK(fd >= 0);
  CHECK(read(fd, value, sizeof value - 1) > 0 && atoi(value) == scope);
  errno = 0;
  CHECK(write(fd, "1", 1) == -1 && errno == EBADF);
  close(fd);
}

static void noexec_defaults(void) {
  int seals = 0;
  switch (scope) {
    case 0:
      // MFD_EXEC: executable, unsealable without MFD_ALLOW_SEALING.
      CHECK(make(MFD_CLOEXEC, &seals) == 0111 && seals == F_SEAL_SEAL);
      CHECK(make(MFD_ALLOW_SEALING, &seals) == 0111 && seals == 0);
      break;
    case 1:
      // MFD_NOEXEC_SEAL: no exec bits, F_SEAL_EXEC, sealable.
      CHECK(make(MFD_CLOEXEC, &seals) == 0 && seals == F_SEAL_EXEC);
      CHECK(make(MFD_EXEC, &seals) == 0111 && seals == F_SEAL_SEAL);
      break;
    default:
      // Enforced: MFD_EXEC is refused, MFD_NOEXEC_SEAL is the default.
      CHECK(make(MFD_CLOEXEC, &seals) == 0 && seals == F_SEAL_EXEC);
      errno = 0;
      CHECK(make(MFD_EXEC, &seals) == -1 && errno == EACCES);
      CHECK(make(MFD_NOEXEC_SEAL, &seals) == 0 && seals == F_SEAL_EXEC);
      break;
  }
  // Both at once is EINVAL at any scope.
  errno = 0;
  CHECK(make(MFD_EXEC | MFD_NOEXEC_SEAL, &seals) == -1 && errno == EINVAL);
}

static void noexec_sysctl_write(void) {
  int fd = open("/proc/sys/vm/memfd_noexec", O_WRONLY | O_CLOEXEC);
  CHECK(fd >= 0);
  for (int n = 0; n <= 2; ++n) {
    char value = '0' + n;
    CHECK(pwrite(fd, &value, 1, 0) == 1);
    scope = n;
    noexec_sysctl_read();
    noexec_defaults();
  }
  for (const char** bad = (const char*[]){"-1", "3", "junk", "4294967296", NULL}; *bad; ++bad) {
    errno = 0;
    CHECK(pwrite(fd, *bad, strlen(*bad), 0) == -1 && errno == EINVAL);
  }
  scope = 2;
  noexec_sysctl_read();
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    int seals;
    if (make(MFD_EXEC, &seals) != -1 || errno != EACCES) _exit(2);
    if (make(0, &seals) != 0 || seals != F_SEAL_EXEC) _exit(3);
    if (setuid(12345) != 0) _exit(4);
    if (open("/proc/sys/vm/memfd_noexec", O_WRONLY) != -1 || errno != EACCES) _exit(5);
    _exit(0);
  }
  int status;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(pwrite(fd, "0", 1, 0) == 1);
  close(fd);
  scope = 0;
  noexec_sysctl_read();
}

int main(int argc, char** argv) {
  if (argc > 1 && !strcmp(argv[1], "--noexec")) {
    scope = argc > 2 ? atoi(argv[2]) : 0;
    RUN(noexec_sysctl_read);
    RUN(noexec_defaults);
    RUN(noexec_sysctl_write);
    DONE();
  }
  RUN(reopen_proc_self_fd);
  RUN(noexec_sysctl_read);
  RUN(seal_rules);
  RUN(seals_across_dup);
  RUN(seals_across_fork);
  RUN(seals_across_scm_rights);
  RUN(seals_and_views);
  RUN(views_in_other_processes);
  RUN(seal_races_mmap);
  RUN(exec_seal_chmod);
  RUN(flock_and_lifetime);
  DONE();
}
