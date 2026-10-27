// proc_pid_mem(5), ptrace(2), Linux fs/proc/base.c mem_rw/mem_lseek;
// LTP-style checks of bytes, partial transfers, errno and open lifetime.
#include <fcntl.h>
#include <linux/capability.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/fsuid.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>
#include "check.h"

static void reaped(pid_t pid) {
  int st;
  CHECK(waitpid(pid, &st, 0) == pid && WIFEXITED(st) && WEXITSTATUS(st) == 0);
}

static void proc_mem(int argc, char** argv) {
  char byte = 0;
  if (argc == 4 && strcmp(argv[1], "--exec-target") == 0) {
    int in = atoi(argv[2]), out = atoi(argv[3]);
    CHECK(write(out, "e", 1) == 1);
    CHECK(read(in, &byte, 1) == 1);
    return;
  }
  if (argc == 4 && strcmp(argv[1], "--exec-reader") == 0) {
    int fd = atoi(argv[2]);
    off_t at = strtoull(argv[3], NULL, 10);
    char buf[8];
    CHECK(pread(fd, buf, sizeof buf, at) == 8 && !memcmp(buf, "written!", 8));
    CHECK(read(fd, buf, 8) == 8);
    return;
  }
  size_t page = sysconf(_SC_PAGESIZE);
  char* memory = mmap(NULL, 2 * page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(memory != MAP_FAILED);
  memcpy(memory, "original", 8);
  memset(memory + page - 8, 'z', 8);
  int commands[2], replies[2];
  CHECK(pipe(commands) == 0 && pipe(replies) == 0);
  pid_t target = fork();
  CHECK(target >= 0);
  if (!target) {
    close(commands[1]); close(replies[0]);
    CHECK(munmap(memory + page, page) == 0);
    CHECK(mprotect(memory, page, PROT_NONE) == 0);
    struct rlimit limit = {20, 20};
    CHECK(setrlimit(RLIMIT_NICE, &limit) == 0);
    CHECK(write(replies[1], "r", 1) == 1);
    while (read(commands[0], &byte, 1) == 1) {
      if (byte == 'd') {
        CHECK(prctl(PR_SET_DUMPABLE, 0) == 0);
        CHECK(write(replies[1], "d", 1) == 1);
      } else if (byte == 'l') {
        CHECK(getrlimit(RLIMIT_NICE, &limit) == 0 && limit.rlim_cur == 10 && limit.rlim_max == 15);
        CHECK(write(replies[1], "l", 1) == 1);
      } else if (byte == 'e') {
        char in[32], out[32];
        snprintf(in, sizeof in, "%d", commands[0]);
        snprintf(out, sizeof out, "%d", replies[1]);
        execl(argv[0], argv[0], "--exec-target", in, out, NULL);
        _exit(30);
      } else break;
    }
    _exit(0);
  }
  close(commands[0]); close(replies[1]);
  CHECK(read(replies[0], &byte, 1) == 1 && byte == 'r');
  char path[64], buf[32] = {0};
  snprintf(path, sizeof path, "/proc/%d/mem", target);
  int fd = open(path, O_RDWR);
  CHECK(fd >= 0);
  struct stat st;
  CHECK(stat(path, &st) == 0 && S_ISREG(st.st_mode) && (st.st_mode & 0777) == 0600 && st.st_size == 0);
  ino_t inode = st.st_ino;
  CHECK(fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size == 0 && st.st_ino == inode);
  off_t at = (uintptr_t)memory;
  CHECK(lseek(fd, at, SEEK_SET) == at);
  CHECK(read(fd, buf, 8) == 8 && !memcmp(buf, "original", 8));
  CHECK(pwrite(fd, "written!", 8, at) == 8);
  CHECK(pread(fd, buf, 8, at) == 8 && !memcmp(buf, "written!", 8));
  CHECK(!memcmp(memory, "original", 8));  // private COW
  CHECK(lseek(fd, 0, SEEK_CUR) == at + 8); // positioned calls leave f_pos
  CHECK(pread(fd, buf, 16, at + page - 8) == 8 && !memcmp(buf, "zzzzzzzz", 8));
  CHECK(pread(fd, buf, 1, at + page) == -1 && errno == EIO);
  CHECK(pwrite(fd, "x", 1, at + page) == -1 && errno == EIO);
  CHECK(pread(fd, (void*)1, 1, at) == -1 && errno == EFAULT);
  CHECK(lseek(fd, 0, SEEK_END) == -1 && errno == EINVAL);
  CHECK(lseek(fd, -8192, SEEK_SET) == -8192);
  CHECK(read(fd, buf, 1) == -1 && errno == EIO);
  CHECK(lseek(fd, at, SEEK_SET) == at);
  int dupfd = dup(fd);
  CHECK(dupfd >= 0 && read(dupfd, buf, 8) == 8);
  CHECK(lseek(fd, 0, SEEK_CUR) == at + 8);
  close(dupfd);
  CHECK(lseek(fd, at, SEEK_SET) == at);
  pid_t reader = fork();
  CHECK(reader >= 0);
  if (!reader) {
    CHECK(read(fd, buf, 8) == 8 && !memcmp(buf, "written!", 8));
    _exit(0);
  }
  reaped(reader);
  CHECK(lseek(fd, 0, SEEK_CUR) == at + 8);
  reader = fork();
  CHECK(reader >= 0);
  if (!reader) {
    char n[32], addr[32];
    snprintf(n, sizeof n, "%d", fd);
    snprintf(addr, sizeof addr, "%llu", (unsigned long long)at);
    execl(argv[0], argv[0], "--exec-reader", n, addr, NULL);
    _exit(31);
  }
  reaped(reader);
  CHECK(lseek(fd, 0, SEEK_CUR) == at + 16);
  CHECK(fcntl(fd, F_SETFL, O_APPEND) == 0);
  CHECK(pwrite(fd, "written!", 8, at) == 8); // mem_write does not append
  CHECK(fcntl(fd, F_SETFL, 0) == 0);
  CHECK(lseek(fd, at + 64, SEEK_SET) == at + 64);
  int gate[2];
  CHECK(pipe(gate) == 0);
  pid_t readers[2];
  for (int i = 0; i < 2; i++) {
    readers[i] = fork();
    CHECK(readers[i] >= 0);
    if (!readers[i]) {
      close(gate[1]);
      CHECK(read(gate[0], &byte, 1) == 1);
      for (int j = 0; j < 64; j++) CHECK(read(fd, &byte, 1) == 1 && byte == 0);
      _exit(0);
    }
  }
  close(gate[0]);
  CHECK(write(gate[1], "rr", 2) == 2);
  close(gate[1]);
  reaped(readers[0]); reaped(readers[1]);
  CHECK(lseek(fd, 0, SEEK_CUR) == at + 192);
  CHECK(lseek(fd, at, SEEK_SET) == at);
  struct iovec vectors[2] = {{buf, 3}, {buf + 3, 5}};
  CHECK(readv(fd, vectors, 2) == 8 && !memcmp(buf, "written!", 8));
  int sockets[2];
  CHECK(socketpair(AF_UNIX, SOCK_DGRAM, 0, sockets) == 0);
  reader = fork();
  CHECK(reader >= 0);
  if (!reader) {
    close(fd); close(sockets[0]);
    union { struct cmsghdr align; char bytes[CMSG_SPACE(sizeof(int))]; } control;
    struct iovec vector = {&byte, 1};
    struct msghdr msg = {.msg_iov = &vector, .msg_iovlen = 1, .msg_control = control.bytes, .msg_controllen = sizeof control.bytes};
    CHECK(recvmsg(sockets[1], &msg, 0) == 1);
    struct cmsghdr* header = CMSG_FIRSTHDR(&msg);
    CHECK(header && header->cmsg_type == SCM_RIGHTS);
    int received;
    memcpy(&received, CMSG_DATA(header), sizeof received);
    CHECK(pread(received, buf, 8, at) == 8 && !memcmp(buf, "written!", 8));
    CHECK(read(received, buf, 8) == 8);
    close(received); close(sockets[1]);
    _exit(0);
  }
  close(sockets[1]);
  union { struct cmsghdr align; char bytes[CMSG_SPACE(sizeof(int))]; } control = {0};
  struct iovec vector = {&byte, 1};
  struct msghdr msg = {.msg_iov = &vector, .msg_iovlen = 1, .msg_control = control.bytes, .msg_controllen = sizeof control.bytes};
  struct cmsghdr* header = CMSG_FIRSTHDR(&msg);
  header->cmsg_level = SOL_SOCKET; header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(sizeof(int));
  memcpy(CMSG_DATA(header), &fd, sizeof fd);
  CHECK(sendmsg(sockets[0], &msg, 0) == 1);
  reaped(reader); close(sockets[0]);
  CHECK(lseek(fd, 0, SEEK_CUR) == at + 16);
  int ro = open(path, O_RDONLY | O_CLOEXEC), wo = open(path, O_WRONLY);
  CHECK(ro >= 0 && wo >= 0);
  CHECK((fcntl(ro, F_GETFL) & O_ACCMODE) == O_RDONLY && (fcntl(ro, F_GETFD) & FD_CLOEXEC));
  CHECK(write(ro, "x", 1) == -1 && errno == EBADF);
  CHECK(read(wo, buf, 1) == -1 && errno == EBADF);
  CHECK(mmap(NULL, page, PROT_READ, MAP_PRIVATE, ro, 0) == MAP_FAILED && errno == ENODEV);
  CHECK(fsync(fd) == -1 && errno == EINVAL);
  CHECK(ftruncate(ro, 0) == -1 && errno == EINVAL);
  CHECK(fallocate(ro, 0, 0, 16) == -1 && errno == EBADF);
  CHECK(fallocate(fd, 0, 0, 16) == -1 && errno == EOPNOTSUPP);
  close(ro); close(wo);
  // Filesystem credentials, not real credentials, govern a new open.
  reader = fork();
  CHECK(reader >= 0);
  if (!reader) {
    struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct caps[2] = {{0}, {0}};
    setfsuid(10001);
    CHECK(syscall(SYS_capset, &h, caps) == 0);
    CHECK(open(path, O_RDONLY) == -1 && errno == EACCES);
    _exit(0);
  }
  reaped(reader);
  reader = fork();
  CHECK(reader >= 0);
  if (!reader) {
    struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct caps[2] = {{0}, {0}};
    CHECK(syscall(SYS_capset, &h, caps) == 0);
    struct rlimit before, lower = {10, 15}, raise = {15, 16}, inverted = {12, 11};
    CHECK(prlimit(target, RLIMIT_NICE, &lower, &before) == 0 && before.rlim_cur == 20 && before.rlim_max == 20);
    CHECK(prlimit(target, RLIMIT_NICE, NULL, &before) == 0 && before.rlim_cur == 10 && before.rlim_max == 15);
    CHECK(prlimit(target, RLIMIT_NICE, &raise, NULL) == -1 && errno == EPERM);
    CHECK(prlimit(target, RLIMIT_NICE, &inverted, NULL) == -1 && errno == EINVAL);
    _exit(0);
  }
  reaped(reader);
  CHECK(write(commands[1], "l", 1) == 1 && read(replies[0], &byte, 1) == 1 && byte == 'l');
  CHECK(write(commands[1], "d", 1) == 1 && read(replies[0], &byte, 1) == 1 && byte == 'd');
  // Existing descriptors retain open-time access after dumpability changes.
  CHECK(pread(fd, buf, 8, at) == 8 && !memcmp(buf, "written!", 8));
  reader = fork();
  CHECK(reader >= 0);
  if (!reader) {
    struct __user_cap_header_struct h = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct caps[2] = {{0}, {0}};
    CHECK(syscall(SYS_capset, &h, caps) == 0);
    CHECK(open(path, O_RDONLY) == -1 && errno == EACCES);
    _exit(0);
  }
  reaped(reader);
  CHECK(write(commands[1], "e", 1) == 1 && read(replies[0], &byte, 1) == 1 && byte == 'e');
  CHECK(pread(fd, buf, 8, at) == 0 && pwrite(fd, "x", 1, at) == 0);
  CHECK(write(commands[1], "x", 1) == 1);
  reaped(target);
  CHECK(pread(fd, buf, 8, at) == 0);
  CHECK(fstat(fd, &st) == 0 && st.st_ino == inode);
  close(fd); close(commands[1]); close(replies[0]);
  CHECK(open(path, O_RDONLY) == -1 && errno == ENOENT);
  CHECK(munmap(memory, 2 * page) == 0);
  puts("proc mem ok");
  return;
}

int main(int argc, char** argv) {
  proc_mem(argc, argv);
  DONE();
}
