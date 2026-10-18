// A thread moving through the scheduling Android gives background work
// (Process.setThreadPriority, set_sched_policy), foreground, display and
// audio work, and real-time threads, then raised by another thread as the
// AOSP audio HAL raises a FAST worker: the host test checks the host
// thread's QoS at each step. Each step prints its name and waits for a line
// on stdin.
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <unistd.h>

static int to_main[2], to_worker[2];

static void step(const char* name) {
  printf("%s\n", name);
  fflush(stdout);
  char line[16];
  if (!fgets(line, sizeof line, stdin)) _exit(1);
}

static void* worker(void* arg) {
  (void)arg;
  prctl(PR_SET_NAME, "qos-worker");
  step("default");
  // THREAD_PRIORITY_BACKGROUND.
  if (setpriority(PRIO_PROCESS, 0, 10) != 0) _exit(2);
  step("utility");
  struct sched_param p = {0};
  if (sched_setscheduler(0, SCHED_IDLE, &p) != 0) _exit(3);
  step("background");
  if (sched_setscheduler(0, SCHED_OTHER, &p) != 0 || setpriority(PRIO_PROCESS, 0, 0) != 0) _exit(4);
  step("default");
  // THREAD_PRIORITY_LOWEST.
  if (setpriority(PRIO_PROCESS, 0, 19) != 0) _exit(5);
  step("background");
  // THREAD_PRIORITY_FOREGROUND.
  if (setpriority(PRIO_PROCESS, 0, -2) != 0) _exit(6);
  step("foreground");
  // THREAD_PRIORITY_URGENT_AUDIO.
  if (setpriority(PRIO_PROCESS, 0, -19) != 0) _exit(7);
  step("urgent-audio");
  if (setpriority(PRIO_PROCESS, 0, 0) != 0) _exit(8);
  p.sched_priority = 3;
  if (sched_setscheduler(0, SCHED_FIFO, &p) != 0) _exit(9);
  step("fifo");
  p.sched_priority = 0;
  if (sched_setscheduler(0, SCHED_OTHER, &p) != 0) _exit(10);
  step("default");
  // The main thread raises this one to SCHED_FIFO; it takes effect on the
  // host at this thread's next syscall.
  pid_t tid = syscall(SYS_gettid);
  char ack;
  if (write(to_main[1], &tid, sizeof tid) != sizeof tid || read(to_worker[0], &ack, 1) != 1) _exit(11);
  step("fifo");
  return NULL;
}

int main(void) {
  pthread_t t;
  if (pipe(to_main) != 0 || pipe(to_worker) != 0) return 1;
  if (pthread_create(&t, NULL, worker, NULL) != 0) return 1;
  pid_t tid;
  if (read(to_main[0], &tid, sizeof tid) != sizeof tid) return 1;
  struct sched_param p = {.sched_priority = 3};
  if (sched_setscheduler(tid, SCHED_FIFO, &p) != 0 || write(to_worker[1], "", 1) != 1) return 1;
  pthread_join(t, NULL);
  printf("PASS\n");
  return 0;
}
