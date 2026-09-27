// /dev/ashmem as libcutils, sensorservice and audio clients use it: the
// region ioctls, and one region mapped by two processes, passed by fork and
// by SCM_RIGHTS.
#include <dlfcn.h>
#include <fcntl.h>
#include <linux/ashmem.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

#define PAGE 16384

static void region_ioctls(void) {
  int fd = open("/dev/ashmem", O_RDWR | O_CLOEXEC);
  CHECK(fd >= 0);
  struct stat st, dev;
  CHECK(fstat(fd, &st) == 0 && S_ISCHR(st.st_mode));
  int again = open("/dev/ashmem", O_RDWR | O_CLOEXEC);
  CHECK(fstat(again, &dev) == 0 && dev.st_rdev == st.st_rdev);
  close(again);
  char name[ASHMEM_NAME_LEN] = "audio client";
  CHECK(ioctl(fd, ASHMEM_SET_NAME, name) == 0);
  CHECK(ioctl(fd, ASHMEM_SET_SIZE, 2 * PAGE) == 0);
  CHECK(ioctl(fd, ASHMEM_GET_SIZE, 0) == 2 * PAGE);
  CHECK(ioctl(fd, ASHMEM_GET_PROT_MASK, 0) == (PROT_READ | PROT_WRITE | PROT_EXEC));
  struct ashmem_pin pin = {PAGE, PAGE};
  errno = 0;
  CHECK(ioctl(fd, ASHMEM_UNPIN, &pin) == -1 && errno == EINVAL);  // not mapped yet
  CHECK(mmap(NULL, 4 * PAGE, PROT_READ, MAP_SHARED, fd, 0) == MAP_FAILED && errno == EINVAL);
  char* p = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(p != MAP_FAILED);
  p[0] = 'a';
  p[2 * PAGE - 1] = 'z';
  errno = 0;
  CHECK(ioctl(fd, ASHMEM_SET_SIZE, PAGE) == -1 && errno == EINVAL);
  CHECK(ioctl(fd, ASHMEM_UNPIN, &pin) == ASHMEM_IS_UNPINNED);
  CHECK(ioctl(fd, ASHMEM_GET_PIN_STATUS, &pin) == ASHMEM_IS_UNPINNED);
  CHECK(ioctl(fd, ASHMEM_PIN, &pin) == ASHMEM_NOT_PURGED);
  CHECK(ioctl(fd, ASHMEM_GET_PIN_STATUS, &pin) == ASHMEM_IS_PINNED);
  memset(name, 0, sizeof name);
  CHECK(ioctl(fd, ASHMEM_GET_NAME, name) == 0 && !strcmp(name, "audio client"));
  unsigned long id = 0;
  CHECK(ioctl(fd, ASHMEM_GET_FILE_ID, &id) == 0 && id != 0);
  CHECK(ioctl(fd, ASHMEM_SET_PROT_MASK, PROT_READ) == 0);
  errno = 0;
  CHECK(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0) == MAP_FAILED && errno == EPERM);
  char* ro = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
  CHECK(ro != MAP_FAILED && ro[0] == 'a');
  errno = 0;
  CHECK(ioctl(fd, ASHMEM_SET_PROT_MASK, PROT_READ | PROT_WRITE) == -1 && errno == EINVAL);
  char link[64], path[32];
  snprintf(path, sizeof path, "/proc/self/fd/%d", fd);
  ssize_t n = readlink(path, link, sizeof link - 1);
  CHECK(n > 0);
  link[n] = 0;
  CHECK(!strcmp(link, "/dev/ashmem"));
  munmap(ro, PAGE);
  munmap(p, 2 * PAGE);
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
  if (recvmsg(sock, &m, 0) != 1) return -1;
  struct cmsghdr* h = CMSG_FIRSTHDR(&m);
  int fd = -1;
  if (h && h->cmsg_type == SCM_RIGHTS) memcpy(&fd, CMSG_DATA(h), sizeof fd);
  return fd;
}

// The child learns of the region only through the socket, checks it is
// ashmem of the parent's size, maps it and answers through the memory.
static void shared_across_processes(void) {
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0);
  FORK_OR_SKIP(pid);
  if (pid == 0) {
    close(sv[0]);
    int fd = recv_fd(sv[1]);
    struct stat st;
    int size = fd < 0 ? -1 : ioctl(fd, ASHMEM_GET_SIZE, 0);
    if (fd < 0 || fstat(fd, &st) != 0 || !S_ISCHR(st.st_mode) || size != PAGE) _exit(2);
    char name[ASHMEM_NAME_LEN];
    if (ioctl(fd, ASHMEM_GET_NAME, name) != 0 || strcmp(name, "sensor queue")) _exit(3);
    volatile char* q = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (q == MAP_FAILED || q[1] != 'p') _exit(4);
    q[2] = 'c';
    _exit(0);
  }
  close(sv[1]);
  int fd = open("/dev/ashmem", O_RDWR | O_CLOEXEC);
  CHECK(fd >= 0);
  char name[ASHMEM_NAME_LEN] = "sensor queue";
  CHECK(ioctl(fd, ASHMEM_SET_NAME, name) == 0 && ioctl(fd, ASHMEM_SET_SIZE, PAGE) == 0);
  volatile char* p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(p != MAP_FAILED);
  p[1] = 'p';
  CHECK(send_fd(sv[0], fd) == 0);
  int status = 0;
  CHECK(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(p[2] == 'c');
  // A fork child shares the mapping itself.
  FORK_OR_SKIP(pid2);
  if (pid2 == 0) {
    p[3] = 'f';
    _exit(0);
  }
  CHECK(waitpid(pid2, &status, 0) == pid2 && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(p[3] == 'f');
  close(fd);
  close(sv[0]);
}

// libcutils' ashmem_create_region, with the image's libcutils.
static void libcutils_region(void) {
  void* h = dlopen("libcutils.so", RTLD_NOW);
  if (!h) {
    printf("skip %s: %s\n", __func__, dlerror());
    skipped = 1;
    return;
  }
  int (*create)(const char*, size_t) = dlsym(h, "ashmem_create_region");
  int (*valid)(int) = dlsym(h, "ashmem_valid");
  int (*get_size)(int) = dlsym(h, "ashmem_get_size_region");
  int (*set_prot)(int, int) = dlsym(h, "ashmem_set_prot_region");
  CHECK(create && valid && get_size && set_prot);
  int fd = create("power hint session", 3 * PAGE);
  CHECK(fd >= 0 && valid(fd) == 1 && get_size(fd) == 3 * PAGE);
  char* p = mmap(NULL, 3 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(p != MAP_FAILED);
  p[PAGE] = 1;
  CHECK(set_prot(fd, PROT_READ) == 0);
  munmap(p, 3 * PAGE);
  close(fd);
}

int main(void) {
  RUN(region_ioctls);
  RUN(shared_across_processes);
  RUN(libcutils_region);
  DONE();
}
