// fcntl_locking(2): process locks survive kernel references, fork creates a
// new owner, exec keeps the owner, and any guest close/exit releases ranges.
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define REQUIRE(c) do { if (!(c)) { \
  dprintf(2, "FAIL line %d: %s (errno %d)\n", __LINE__, #c, errno); \
  _exit(1); \
} } while (0)

#ifndef LOCK_PATH
#define LOCK_PATH "/data/local/tmp/posix-lock-file"
#endif
static const char *path = LOCK_PATH;
static volatile sig_atomic_t signalled;

static void caught(int sig) { signalled = sig; }
static void send_byte(int fd, char c) { REQUIRE(write(fd, &c, 1) == 1); }
static void expect(int fd, char c) {
  char got;
  REQUIRE(read(fd, &got, 1) == 1 && got == c);
}
static int lock_range(int fd, int cmd, short type, off_t start, off_t len) {
  struct flock l = {.l_type = type, .l_whence = SEEK_SET,
                    .l_start = start, .l_len = len};
  return fcntl(fd, cmd, &l);
}
static void conflict(int fd, pid_t owner) {
  struct flock q = {.l_type = F_WRLCK, .l_whence = SEEK_SET,
                    .l_start = 15, .l_len = 1};
  REQUIRE(fcntl(fd, F_GETLK, &q) == 0);
  REQUIRE(q.l_type == F_WRLCK && q.l_whence == SEEK_SET);
  REQUIRE(q.l_start == 10 && q.l_len == 10 && q.l_pid == owner);
  REQUIRE(lock_range(fd, F_SETLK, F_WRLCK, 15, 1) == -1);
  REQUIRE(errno == EAGAIN || errno == EACCES);
  // Adjacent ranges are independent.
  REQUIRE(lock_range(fd, F_SETLK, F_WRLCK, 20, 10) == 0);
  REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, 20, 10) == 0);
}
static void reaped(pid_t child) {
  int status;
  REQUIRE(waitpid(child, &status, 0) == child);
  REQUIRE(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
static void owner_loop(int fd, int channel, int after_exec, const char *self) {
  if (!after_exec) {
    REQUIRE(lock_range(fd, F_SETLK, F_WRLCK, 10, 10) == 0);
    // Each operation privately pins the descriptor in the syscall layer.
    char byte;
    struct stat st;
    struct pollfd p = {.fd = fd, .events = POLLIN};
    REQUIRE(read(fd, &byte, 1) == 1);
    REQUIRE(pread(fd, &byte, 1, 0) == 1);
    REQUIRE(fstat(fd, &st) == 0 && st.st_size == 64);
    REQUIRE(poll(&p, 1, 0) == 1 && (p.revents & POLLIN));
    pid_t owner = getpid();
    pid_t child = fork();
    REQUIRE(child >= 0);
    if (!child) {
      // Inherited file descriptions do not confer process lock ownership.
      conflict(fd, owner);
      REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, 0, 0) == 0);
      REQUIRE(close(fd) == 0);
      _exit(0);
    }
    reaped(child);
    send_byte(channel, 'p');
    expect(channel, 'e');
    char descriptor[24], socket[24];
    snprintf(descriptor, sizeof(descriptor), "%d", fd);
    snprintf(socket, sizeof(socket), "%d", channel);
    execl(self, self, "exec", descriptor, socket, (char *)NULL);
    REQUIRE(0);
  }
  send_byte(channel, 'e');
  expect(channel, 'c');
  // Closing a separately opened fd for the inode releases every owner range.
  int other = open(path, O_RDONLY);
  REQUIRE(other >= 0 && close(other) == 0);
  send_byte(channel, 'c');
  expect(channel, 'l');
  REQUIRE(lock_range(fd, F_SETLK, F_WRLCK, 10, 10) == 0);
  send_byte(channel, 'l');
  expect(channel, 'x');
  // Leave the locked descriptor open: process exit must release the lock.
  _exit(0);
}
int main(int argc, char **argv) {
  alarm(30);
  if (argc == 4) {
    owner_loop(atoi(argv[2]), atoi(argv[3]), 1, argv[0]);
    return 1;
  }
  REQUIRE(argc == 1);
  int fd = open(path, O_CREAT | O_TRUNC | O_RDWR, 0600);
  REQUIRE(fd >= 0 && ftruncate(fd, 64) == 0);
  int channel[2];
  REQUIRE(socketpair(AF_UNIX, SOCK_STREAM, 0, channel) == 0);
  pid_t owner = fork();
  REQUIRE(owner >= 0);
  if (!owner) {
    REQUIRE(close(channel[0]) == 0);
    owner_loop(fd, channel[1], 0, argv[0]);
  }
  REQUIRE(close(channel[1]) == 0);
  expect(channel[0], 'p');
  conflict(fd, owner);
  puts("ok pins and fork ownership");
  send_byte(channel[0], 'e');
  expect(channel[0], 'e');
  conflict(fd, owner);
  puts("ok exec retains owner and range");

  int waiting[2];
  REQUIRE(socketpair(AF_UNIX, SOCK_STREAM, 0, waiting) == 0);
  pid_t waiter = fork();
  REQUIRE(waiter >= 0);
  if (!waiter) {
    REQUIRE(close(waiting[0]) == 0);
    struct sigaction action = {.sa_handler = caught};
    sigemptyset(&action.sa_mask);
    REQUIRE(sigaction(SIGALRM, &action, NULL) == 0);
    alarm(1);
    REQUIRE(lock_range(fd, F_SETLKW, F_WRLCK, 10, 10) == -1);
    REQUIRE(errno == EINTR && signalled == SIGALRM);
    alarm(20);
    send_byte(waiting[1], 'i');
    REQUIRE(lock_range(fd, F_SETLKW, F_WRLCK, 10, 10) == 0);
    send_byte(waiting[1], 'a');
    expect(waiting[1], 'u');
    REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, 0, 0) == 0);
    _exit(0);
  }
  REQUIRE(close(waiting[1]) == 0);
  expect(waiting[0], 'i');
  struct pollfd pending = {.fd = waiting[0], .events = POLLIN};
  REQUIRE(poll(&pending, 1, 100) == 0);
  conflict(fd, owner);
  send_byte(channel[0], 'c');
  expect(channel[0], 'c');
  expect(waiting[0], 'a');
  send_byte(waiting[0], 'u');
  reaped(waiter);
  REQUIRE(close(waiting[0]) == 0);
  REQUIRE(lock_range(fd, F_SETLK, F_WRLCK, 10, 10) == 0);
  REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, 0, 0) == 0);
  puts("ok any close releases, blocking wait and EINTR");

  send_byte(channel[0], 'l');
  expect(channel[0], 'l');
  conflict(fd, owner);
  send_byte(channel[0], 'x');
  reaped(owner);
  REQUIRE(lock_range(fd, F_SETLKW, F_WRLCK, 10, 10) == 0);
  REQUIRE(close(fd) == 0 && close(channel[0]) == 0);
  REQUIRE(unlink(path) == 0);
  puts("ok exit releases");
  puts("PASS");
  return 0;
}
