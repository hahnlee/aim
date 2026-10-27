// setpriority(2): another process's leader and worker keep independent
// nice values. The host test reads each target's actual Darwin priority.
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "line %d: %s (errno %d)\n", __LINE__, #x, errno); _exit(1); } } while (0)
static int command[2], reply[2], work[2], done[2];

static void step(const char *name, int want) {
  CHECK(getpriority(PRIO_PROCESS, 0) == want);
  printf("%s %d\n", name, want);
  fflush(stdout);
  char line[16];
  CHECK(fgets(line, sizeof line, stdin));
}

static void *inherited(void *unused) {
  (void)unused;
  CHECK(getpriority(PRIO_PROCESS, 0) == -4);
  return NULL;
}

static void *worker(void *unused) {
  (void)unused;
  CHECK(prctl(PR_SET_NAME, "remote-worker") == 0);
  pid_t tid = syscall(SYS_gettid);
  CHECK(write(reply[1], &tid, sizeof tid) == sizeof tid);
  int want;
  while (read(work[0], &want, sizeof want) == sizeof want && want != 1000) {
    step("worker", want);
    CHECK(write(done[1], "r", 1) == 1);
  }
  CHECK(setpriority(PRIO_PROCESS, 0, -4) == 0);
  pthread_t next;
  CHECK(pthread_create(&next, NULL, inherited, NULL) == 0);
  CHECK(pthread_join(next, NULL) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (!child) {
    CHECK(getpriority(PRIO_PROCESS, 0) == -4);
    execl("/data/local/tmp/t_remote_nice", "t_remote_nice", "-4", NULL);
    perror("exec t_remote_nice");
    _exit(1);
  }
  int status;
  pid_t reaped = waitpid(child, &status, 0);
  if (reaped != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
    fprintf(stderr, "reaped %d child %d status %#x\n", reaped, child, status);
  CHECK(reaped == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  return NULL;
}

static void target(void) {
  close(command[1]);
  close(reply[0]);
  CHECK(prctl(PR_SET_NAME, "remote-main") == 0);
  pthread_t thread;
  CHECK(pthread_create(&thread, NULL, worker, NULL) == 0);
  int values[2];
  while (read(command[0], values, sizeof values) == sizeof values) {
    if (values[0] == 2) break;
    if (values[0]) {
      CHECK(write(work[1], &values[1], sizeof values[1]) == sizeof values[1]);
      char c;
      CHECK(read(done[0], &c, 1) == 1);
    } else {
      step("main", values[1]);
    }
    CHECK(write(reply[1], "r", 1) == 1);
  }
  int stop = 1000;
  CHECK(write(work[1], &stop, sizeof stop) == sizeof stop);
  CHECK(pthread_join(thread, NULL) == 0);
  CHECK(write(reply[1], "r", 1) == 1);
  char c;
  CHECK(read(command[0], &c, 1) == 1);
  _exit(0);
}

int main(int argc, char **argv) {
  if (argc == 2) {
    CHECK(getpriority(PRIO_PROCESS, 0) == atoi(argv[1]));
    return 0;
  }
  CHECK(pipe(command) == 0 && pipe(reply) == 0 && pipe(work) == 0 && pipe(done) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (!child) target();
  close(command[0]);
  close(reply[1]);
  close(work[0]);
  close(work[1]);
  close(done[0]);
  close(done[1]);
  pid_t tid;
  CHECK(read(reply[0], &tid, sizeof tid) == sizeof tid);
  printf("target %d\n", child);
  fflush(stdout);
  for (int which = 0; which < 2; ++which) {
    pid_t target = which ? tid : child;
    pid_t other = which ? child : tid;
    const int nice[] = {19, 10, -2, -19, 0};
    for (unsigned i = 0; i < sizeof nice / sizeof nice[0]; ++i) {
      CHECK(setpriority(PRIO_PROCESS, target, nice[i]) == 0);
      CHECK(getpriority(PRIO_PROCESS, target) == nice[i]);
      CHECK(getpriority(PRIO_PROCESS, other) == 0);
      int values[] = {which, nice[i]};
      CHECK(write(command[1], values, sizeof values) == sizeof values);
      char c;
      CHECK(read(reply[0], &c, 1) == 1);
    }
  }
  CHECK(setpriority(PRIO_PGRP, 0, -4) == 0);
  CHECK(getpriority(PRIO_PROCESS, child) == -4 && getpriority(PRIO_PROCESS, tid) == -4);
  CHECK(setpriority(PRIO_PROCESS, 0, 0) == 0 && setpriority(PRIO_PROCESS, child, 0) == 0);
  CHECK(getpriority(PRIO_PGRP, 0) == -4 && getpriority(PRIO_USER, 0) == -4);
  CHECK(setpriority(PRIO_USER, 0, 0) == 0 && getpriority(PRIO_PROCESS, tid) == 0);
  CHECK(setpriority(PRIO_PROCESS, tid, 100) == 0 && getpriority(PRIO_PROCESS, tid) == 19);
  CHECK(setpriority(PRIO_PROCESS, tid, -100) == 0 && getpriority(PRIO_PROCESS, tid) == -20);
  // The exited tid must fail instead of being mapped to its live leader.
  int values[] = {2, 0};
  CHECK(write(command[1], values, sizeof values) == sizeof values);
  char c;
  CHECK(read(reply[0], &c, 1) == 1);
  CHECK(setpriority(PRIO_PROCESS, tid, 0) == -1 && errno == ESRCH);
  CHECK(getpriority(PRIO_PROCESS, tid) == -1 && errno == ESRCH);
  CHECK(getpriority(PRIO_PROCESS, child) == 0);
  CHECK(write(command[1], "r", 1) == 1);
  int status;
  pid_t reaped = waitpid(child, &status, 0);
  if (reaped != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0)
    fprintf(stderr, "reaped %d child %d status %#x\n", reaped, child, status);
  CHECK(reaped == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  puts("PASS");
  return 0;
}
