// Controller values, process/thread moves and fork inheritance, with host
// scheduling observed by ndk.rs while this program waits at each step.
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#include "check.h"

static int command[2][2], answer[2][2];
static pid_t tids[2];
static atomic_uint ready[2], released;
static unsigned epoch;

static void put(const char* path, const char* value) {
  int fd = open(path, O_WRONLY | O_CLOEXEC);
  if (fd < 0 || write(fd, value, strlen(value)) != (ssize_t)strlen(value)) {
    perror(path);
    _exit(2);
  }
  close(fd);
}

static void move(const char* path, pid_t tid) {
  char text[32];
  snprintf(text, sizeof text, "%d", tid);
  put(path, text);
}

static void* worker(void* arg) {
  long i = (long)arg;
  prctl(PR_SET_NAME, i == 0 ? "cg-one" : "cg-two");
  pid_t tid = syscall(SYS_gettid);
  if (write(answer[i][1], &tid, sizeof tid) != sizeof tid) _exit(3);
  unsigned epoch = 0;
  char c;
  while (read(command[i][0], &c, 1) == 1) {
    if (c == 'q') return NULL;
    if (c == 'x') {
      atomic_store_explicit(&ready[i], ++epoch, memory_order_release);
      while (atomic_load_explicit(&released, memory_order_acquire) < epoch) {}
      continue;
    }
    if (c == 'f' || c == 'n') {
      struct sched_param p = {.sched_priority = c == 'f' ? 3 : 0};
      if (sched_setscheduler(0, c == 'f' ? SCHED_FIFO : SCHED_OTHER, &p) != 0) _exit(9);
    }
    if ((c == 'a' || c == 'u' || c == 'b' || c == 'l') &&
        setpriority(PRIO_PROCESS, 0, c == 'a' ? -19 : c == 'b' ? 10 : c == 'l' ? 19 : 0) != 0) _exit(10);
    if (write(answer[i][1], &c, 1) != 1) _exit(4);
  }
  return NULL;
}

static void sync_workers(void) {
  ++epoch;
  for (int i = 0; i < 2; i++) {
    if (write(command[i][1], "x", 1) != 1) _exit(5);
  }
  // A pipe acknowledgement lets the observer race the worker's next syscall
  // and its temporary Darwin lock-owner priority promotion. Sample only after
  // both workers have returned to guest code, with no more syscalls until release.
  for (int i = 0; i < 2; i++) {
    while (atomic_load_explicit(&ready[i], memory_order_acquire) != epoch) {}
  }
}

static void step(const char* name, pid_t child) {
  sync_workers();
  printf("%s %d\n", name, child);
  fflush(stdout);
  char text[16];
  if (!fgets(text, sizeof text, stdin)) _exit(6);
  atomic_store_explicit(&released, epoch, memory_order_release);
}

int main(void) {
  if (mkdir("/dev/cpuctl/value", 0755) != 0 || mkdir("/dev/cpuctl/value/nested", 0755) != 0) return 1;
  put("/dev/cpuctl/value/cpu.idle", "1");
  pthread_t threads[2];
  for (long i = 0; i < 2; i++) {
    if (pipe(command[i]) != 0 || pipe(answer[i]) != 0 ||
        pthread_create(&threads[i], NULL, worker, (void*)i) != 0 ||
        read(answer[i][0], &tids[i], sizeof tids[i]) != sizeof tids[i]) return 1;
  }
  step("default", 0);
  if (mkdir("/dev/blkio/value", 0755) != 0) return 1;
  put("/dev/blkio/value/blkio.prio.class", "restrict-to-be");
  put("/dev/blkio/value/cgroup.procs", "0");
  step("io-utility", 0);
  char ack;
  if (write(command[1][1], "b", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("io-nice-utility", 0);
  if (write(command[1][1], "l", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("io-nice-background", 0);
  if (write(command[1][1], "a", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("io-urgent-audio", 0);
  if (write(command[1][1], "u", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("io-normal", 0);
  put("/dev/blkio/value/blkio.prio.class", "idle");
  step("io-idle", 0);
  if (write(command[1][1], "f", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("io-realtime", 0);
  if (write(command[1][1], "n", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("io-fair", 0);
  put("/dev/blkio/value/blkio.prio.class", "promote-to-rt");
  step("io-important", 0);
  put("/dev/blkio/cgroup.procs", "0");
  step("io-restored", 0);
  put("/dev/cpuctl/value/nested/cgroup.procs", "0");
  step("background", 0);
  if (write(command[1][1], "f", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("realtime", 0);
  if (write(command[1][1], "n", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("fair", 0);
  move("/dev/cpuctl/tasks", tids[0]);
  step("thread", 0);
  put("/dev/cpuctl/value/cpu.idle", "0");
  put("/dev/cpuctl/value/cpu.uclamp.max", "50");
  step("utilization", 0);
  if (write(command[1][1], "f", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  step("capped-realtime", 0);
  if (write(command[1][1], "n", 1) != 1 || read(answer[1][0], &ack, 1) != 1) return 1;
  put("/dev/cpuctl/value/cpu.uclamp.max", "max");
  put("/dev/cpuctl/value/cpu.shares", "512");
  step("shares", 0);
  put("/dev/cpuctl/value/cpu.shares", "1024");
  step("restored", 0);
  put("/dev/cpuctl/value/cpu.uclamp.min", "60");
  put("/dev/cpuctl/value/nested/cpu.uclamp.min", "60");
  step("raised", 0);
  put("/dev/cpuctl/value/cpu.uclamp.max", "50");
  step("clamped", 0);
  put("/dev/cpuctl/value/cpu.uclamp.min", "0");
  put("/dev/cpuctl/value/nested/cpu.uclamp.min", "0");
  put("/dev/cpuctl/value/cpu.uclamp.max", "max");
  step("unclamped", 0);
  put("/dev/cpuctl/value/cpu.idle", "1");
  put("/dev/cpuctl/value/nested/cgroup.procs", "0");
  int to_child[2], from_child[2];
  if (pipe(to_child) != 0 || pipe(from_child) != 0) return 1;
  pid_t child = fork();
  if (child < 0) return 1;
  if (child == 0) {
    prctl(PR_SET_NAME, "cg-child");
    char c;
    if (write(from_child[1], "x", 1) != 1) _exit(7);
    while (read(to_child[0], &c, 1) == 1 && c != 'q') {
      if (write(from_child[1], &c, 1) != 1) _exit(8);
    }
    _exit(0);
  }
  char c;
  if (read(from_child[0], &c, 1) != 1) return 1;
  step("forked", child);
  put("/dev/cpuctl/cgroup.procs", "0");
  step("parent", child);
  move("/dev/cpuctl/cgroup.procs", child);
  if (write(to_child[1], "x", 1) != 1 || read(from_child[0], &c, 1) != 1) return 1;
  step("remote", child);
  if (write(to_child[1], "q", 1) != 1) return 1;
  int status;
  if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status)) return 1;
  for (int i = 0; i < 2; i++) {
    if (write(command[i][1], "q", 1) != 1 || pthread_join(threads[i], NULL) != 0) return 1;
  }
  puts("PASS");
  return 0;
}
