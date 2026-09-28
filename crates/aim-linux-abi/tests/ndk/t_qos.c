// A thread moving through the scheduling Android gives background work
// (Process.setThreadPriority, set_sched_policy): the host test checks the
// host thread's QoS at each step. Each step prints its name and waits for a
// line on stdin.
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <unistd.h>

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
  return NULL;
}

int main(void) {
  pthread_t t;
  if (pthread_create(&t, NULL, worker, NULL) != 0) return 1;
  pthread_join(t, NULL);
  printf("PASS\n");
  return 0;
}
