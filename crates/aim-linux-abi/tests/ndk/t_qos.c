// A thread moving through the scheduling Android gives background work
// (Process.setThreadPriority, set_sched_policy), then a thread of another
// process changed from here: the host test checks the host thread's QoS
// at each step. Each step prints its name and waits for a
// line on stdin.
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

static void step(const char* name) {
  if (*name) printf("%s\n", name);
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

static int peer_up[2], peer_down[2];

static void* peer(void* arg) {
  (void)arg;
  prctl(PR_SET_NAME, "qos-peer");
  pid_t tid = gettid();
  write(peer_up[1], &tid, sizeof tid);
  char c;
  read(peer_down[0], &c, 1);
  return NULL;
}

// A thread of another process, as ActivityManager boosts a top app's
// RenderThread: SCHED_FIFO, then the default, then TOP_APP_PRIORITY_BOOST.
static void remote(void) {
  if (pipe(peer_up) != 0 || pipe(peer_down) != 0) _exit(6);
  pid_t child = fork();
  if (child == 0) {
    pthread_t t;
    if (pthread_create(&t, NULL, peer, NULL) != 0) _exit(1);
    pthread_join(t, NULL);
    _exit(0);
  }
  pid_t tid;
  if (read(peer_up[0], &tid, sizeof tid) != sizeof tid) _exit(7);
  struct sched_param p = {.sched_priority = 1};
  if (sched_setscheduler(tid, SCHED_FIFO, &p) != 0) _exit(8);
  printf("remote-fifo %d\n", child);
  step("");
  p.sched_priority = 0;
  if (sched_setscheduler(tid, SCHED_OTHER, &p) != 0) _exit(9);
  step("remote-default");
  if (setpriority(PRIO_PROCESS, tid, -10) != 0) _exit(10);
  step("remote-boost");
  write(peer_down[1], "x", 1);
  int st;
  if (waitpid(child, &st, 0) != child || !WIFEXITED(st) || WEXITSTATUS(st) != 0) _exit(11);
}

int main(void) {
  pthread_t t;
  if (pthread_create(&t, NULL, worker, NULL) != 0) return 1;
  pthread_join(t, NULL);
  remote();
  printf("PASS\n");
  return 0;
}
