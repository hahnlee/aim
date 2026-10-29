// LTP-style fcntl_locking(2) stress. SQLite's rollback-journal byte ranges
// exercise reader admission, writer reservation, pending and exclusive locks.
// Also runnable directly on Linux with: cc -pthread t_posix_lock_stress.c.
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

#define REQUIRE(c) do { if (!(c)) { \
  dprintf(2, "FAIL pid %d line %d: %s (errno %d)\n", getpid(), __LINE__, #c, errno); \
  _exit(1); \
} } while (0)

enum { PROCESSES = 6, ROUNDS = 100, CHURN = 1000, SHARED_SIZE = 510 };
static const off_t pending = 0x40000000, reserved = 0x40000001, shared = 0x40000002;
static const char *path;
static volatile sig_atomic_t children[PROCESSES];
static volatile sig_atomic_t child_count;

static void timed_out(int signal) {
  (void)signal;
  for (int i = 0; i < child_count; ++i)
    if (children[i] > 0) kill(children[i], SIGKILL);
  for (int i = 0; i < child_count; ++i)
    if (children[i] > 0)
      while (waitpid(children[i], NULL, 0) < 0 && errno == EINTR) {}
  _exit(1);
}

static int lock_range(int fd, int cmd, short type, off_t start, off_t len) {
  struct flock lock = {.l_type = type, .l_whence = SEEK_SET,
                       .l_start = start, .l_len = len};
  return fcntl(fd, cmd, &lock);
}

static int try_lock(int fd, short type, off_t start, off_t len) {
  if (lock_range(fd, F_SETLK, type, start, len) == 0) return 1;
  REQUIRE(errno == EAGAIN || errno == EACCES);
  return 0;
}

static void *closer(void *arg) {
  int gate = *(int *)arg;
  char byte;
  REQUIRE(read(gate, &byte, 1) == 1 && close(gate) == 0);
  for (int i = 0; i < CHURN; ++i) {
    int fd = open(path, O_RDWR);
    REQUIRE(fd >= 0 && close(fd) == 0);
  }
  return NULL;
}

static void close_churn(int fd, int index) {
  int gate[2];
  REQUIRE(pipe(gate) == 0);
  pthread_t thread;
  REQUIRE(pthread_create(&thread, NULL, closer, &gate[0]) == 0);
  REQUIRE(write(gate[1], "s", 1) == 1 && close(gate[1]) == 0);
  // No other process locks this range. A close may remove our lock, but
  // cannot make a still-open fd's nonblocking fcntl fail with EBADF/EINTR.
  off_t start = shared + SHARED_SIZE + index;
  for (int i = 0; i < CHURN; ++i) {
    REQUIRE(lock_range(fd, F_SETLK, F_RDLCK, start, 1) == 0);
    struct flock query = {.l_type = F_WRLCK, .l_whence = SEEK_SET,
                          .l_start = start, .l_len = 1};
    REQUIRE(fcntl(fd, F_GETLK, &query) == 0 && query.l_type == F_UNLCK);
    REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, start, 1) == 0);
  }
  REQUIRE(pthread_join(thread, NULL) == 0);
}

static void worker(int index, int gate) {
  int fd = open(path, O_RDWR);
  REQUIRE(fd >= 0);
  char byte;
  REQUIRE(read(gate, &byte, 1) == 1 && close(gate) == 0);
  for (int round = 0; round < ROUNDS; ++round) {
    for (;;) {
      // Readers pass pending before taking shared; pending excludes new
      // readers while the reserved writer upgrades the shared range.
      REQUIRE(lock_range(fd, F_SETLKW, F_RDLCK, pending, 1) == 0);
      REQUIRE(lock_range(fd, F_SETLK, F_RDLCK, shared, SHARED_SIZE) == 0);
      REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, pending, 1) == 0);
      if (try_lock(fd, F_WRLCK, reserved, 1)) break;
      struct flock query = {.l_type = F_WRLCK, .l_whence = SEEK_SET,
                            .l_start = reserved, .l_len = 1};
      REQUIRE(fcntl(fd, F_GETLK, &query) == 0);
      REQUIRE(query.l_type == F_UNLCK ||
              (query.l_type == F_WRLCK && query.l_pid > 0 && query.l_pid != getpid() &&
               query.l_start <= reserved &&
               (query.l_len == 0 || query.l_start + query.l_len > reserved)));
      REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, shared, SHARED_SIZE) == 0);
      sched_yield();
    }
    REQUIRE(lock_range(fd, F_SETLKW, F_WRLCK, pending, 1) == 0);
    // Both the nonblocking conflict errno and eventual blocking acquisition
    // are checked while other processes drain their shared ranges.
    if (!try_lock(fd, F_WRLCK, shared, SHARED_SIZE))
      REQUIRE(lock_range(fd, F_SETLKW, F_WRLCK, shared, SHARED_SIZE) == 0);
    int value;
    REQUIRE(pread(fd, &value, sizeof(value), 0) == sizeof(value));
    ++value;
    REQUIRE(pwrite(fd, &value, sizeof(value), 0) == sizeof(value));
    REQUIRE(lock_range(fd, F_SETLK, F_RDLCK, shared, SHARED_SIZE) == 0);
    REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, pending, 2) == 0);
    REQUIRE(lock_range(fd, F_SETLK, F_UNLCK, shared, SHARED_SIZE) == 0);
  }
  close_churn(fd, index);
  REQUIRE(close(fd) == 0);
  _exit(0);
}

int main(int argc, char **argv) {
  REQUIRE(argc <= 2);
  path = argc == 2 ? argv[1] : "/data/local/tmp/posix-lock-stress";
  struct sigaction action = {.sa_handler = timed_out};
  sigemptyset(&action.sa_mask);
  REQUIRE(sigaction(SIGALRM, &action, NULL) == 0);
  alarm(120);
  int fd = open(path, O_CREAT | O_EXCL | O_RDWR, 0600);
  REQUIRE(fd >= 0);
  int value = 0;
  REQUIRE(write(fd, &value, sizeof(value)) == sizeof(value));
  REQUIRE(close(fd) == 0);
  int gate[2];
  REQUIRE(pipe(gate) == 0);
  for (int i = 0; i < PROCESSES; ++i) {
    children[i] = fork();
    if (children[i] < 0) {
      for (int j = 0; j < i; ++j) kill(children[j], SIGKILL);
      for (int j = 0; j < i; ++j) waitpid(children[j], NULL, 0);
      REQUIRE(0);
    }
    if (!children[i]) {
      REQUIRE(close(gate[1]) == 0);
      worker(i, gate[0]);
    }
    child_count = i + 1;
  }
  REQUIRE(close(gate[0]) == 0);
  REQUIRE(write(gate[1], "start!", PROCESSES) == PROCESSES && close(gate[1]) == 0);
  int failed = 0;
  for (int i = 0; i < PROCESSES; ++i) {
    int status;
    if (waitpid(children[i], &status, 0) != children[i] ||
        !WIFEXITED(status) || WEXITSTATUS(status) != 0) failed = 1;
    children[i] = 0;
  }
  REQUIRE(!failed);
  fd = open(path, O_RDONLY);
  REQUIRE(fd >= 0 && read(fd, &value, sizeof(value)) == sizeof(value));
  REQUIRE(value == PROCESSES * ROUNDS && close(fd) == 0 && unlink(path) == 0);
  puts("PASS 6 processes: shared/pending/reserved/exclusive ranges and concurrent close");
  return 0;
}
