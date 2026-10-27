// Task identity contracts, after LTP's gettid, clone and tgkill tests:
// namespace uniqueness, pid_max's exclusive bound, CPU clock round trips,
// and stable /proc and signal ownership across fork and thread exit.
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

#define THREADS 8
#define PROCESSES 4
#define CPU_CLOCK(tid, bits) ((clockid_t)(~(unsigned)(tid) << 3 | (bits)))

static _Atomic int finish;
static pthread_t threads[THREADS];

static void* worker(void* unused) {
  (void)unused;
  while (!atomic_load(&finish)) usleep(1000);
  return NULL;
}

static void start(pid_t tids[THREADS + 1]) {
  tids[0] = getpid();
  CHECK(syscall(SYS_gettid) == tids[0]);
  for (int i = 0; i < THREADS; i++) {
    CHECK(pthread_create(&threads[i], NULL, worker, NULL) == 0);
    tids[i + 1] = pthread_gettid_np(threads[i]);
    clockid_t id;
    CHECK(pthread_getcpuclockid(threads[i], &id) == 0);
    CHECK(id == CPU_CLOCK(tids[i + 1], 6));
    CHECK(~(id >> 3) == tids[i + 1]);
    struct timespec ts;
    for (int which = 0; which < 3; which++) {
      CHECK(syscall(SYS_clock_gettime, CPU_CLOCK(tids[i + 1], 4 | which), &ts) == 0);
    }
  }
}

static void stop(pid_t tids[THREADS + 1]) {
  atomic_store(&finish, 1);
  for (int i = 0; i < THREADS; i++) {
    CHECK(pthread_join(threads[i], NULL) == 0);
    CHECK(syscall(SYS_tgkill, getpid(), tids[i + 1], 0) == -1 && errno == ESRCH);
    struct timespec ts;
    CHECK(syscall(SYS_clock_gettime, CPU_CLOCK(tids[i + 1], 6), &ts) == -1 && errno == EINVAL);
  }
  atomic_store(&finish, 0);
}

static void namespace_ids(void) {
  FILE* f = fopen("/proc/sys/kernel/pid_max", "r");
  CHECK(f != NULL);
  int pid_max;
  CHECK(fscanf(f, "%d", &pid_max) == 1);
  fclose(f);
  CHECK(pid_max > 0 && pid_max <= (1 << 22));
  pid_t tids[PROCESSES + 1][THREADS + 1], children[PROCESSES];
  int report[2], release[2];
  CHECK(pipe(report) == 0 && pipe(release) == 0);
  for (int p = 0; p < PROCESSES; p++) {
    children[p] = fork();
    CHECK(children[p] >= 0);
    if (children[p] == 0) {
      close(report[0]);
      close(release[1]);
      pid_t own[THREADS + 1];
      start(own);
      CHECK(write(report[1], own, sizeof own) == sizeof own);
      char byte;
      CHECK(read(release[0], &byte, 1) == 1);
      stop(own);
      _exit(failures ? 1 : 0);
    }
  }
  close(report[1]);
  close(release[0]);
  start(tids[0]);
  for (int p = 1; p <= PROCESSES; p++) {
    CHECK(read(report[0], tids[p], sizeof tids[p]) == sizeof tids[p]);
  }
  for (int p = 0; p <= PROCESSES; p++) {
    for (int t = 0; t <= THREADS; t++) {
      pid_t tid = tids[p][t];
      CHECK(tid > 0 && tid < pid_max);
      for (int q = 0; q <= p; q++) {
        for (int u = 0; u < (q == p ? t : THREADS + 1); u++) CHECK(tid != tids[q][u]);
      }
      CHECK(syscall(SYS_tgkill, tids[p][0], tid, 0) == 0);
      if (p != 0) CHECK(syscall(SYS_tgkill, getpid(), tid, 0) == -1 && errno == ESRCH);
      char path[128];
      snprintf(path, sizeof path, "/proc/%d/task/%d/status", tids[p][0], tid);
      f = fopen(path, "r");
      CHECK(f != NULL);
      char line[256];
      int tgid = 0, pid = 0;
      while (fgets(line, sizeof line, f)) {
        sscanf(line, "Tgid: %d", &tgid);
        sscanf(line, "Pid: %d", &pid);
      }
      fclose(f);
      CHECK(tgid == tids[p][0] && pid == tid);
    }
  }
  stop(tids[0]);
  CHECK(write(release[1], "done", PROCESSES) == PROCESSES);
  close(release[1]);
  close(report[0]);
  for (int p = 0; p < PROCESSES; p++) {
    int status;
    CHECK(waitpid(children[p], &status, 0) == children[p]);
    CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  }
  // Repeated creation reuses local record slots even though task IDs advance.
  for (int wave = 0; wave < 260; wave++) {
    start(tids[0]);
    stop(tids[0]);
  }
}

int main(void) {
  RUN(namespace_ids);
  DONE();
}
