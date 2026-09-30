// Interval timers (setitimer(2), getitimer(2), alarm(2)): signals, old
// values, remaining time, CPU-time timers, fork and exec.
#include <signal.h>
#include <stdlib.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

static volatile sig_atomic_t got[3];
static volatile int code;

static void on_signal(int sig, siginfo_t* si, void* uc) {
  (void)uc;
  got[sig == SIGALRM ? 0 : sig == SIGVTALRM ? 1 : 2]++;
  code = si->si_code;
}

static void catch_signals(void) {
  struct sigaction sa = {.sa_sigaction = on_signal, .sa_flags = SA_SIGINFO | SA_RESTART};
  sigaction(SIGALRM, &sa, NULL);
  sigaction(SIGVTALRM, &sa, NULL);
  sigaction(SIGPROF, &sa, NULL);
}

static long us(struct timeval t) { return t.tv_sec * 1000000L + t.tv_usec; }

static void real_timer(void) {
  got[0] = 0;
  struct itimerval v = {.it_value = {0, 20000}}, old;
  CHECK(setitimer(ITIMER_REAL, &v, NULL) == 0);
  struct itimerval cur;
  CHECK(getitimer(ITIMER_REAL, &cur) == 0);
  CHECK(us(cur.it_value) > 0 && us(cur.it_value) <= 20000 && us(cur.it_interval) == 0);
  struct timespec ts = {0, 200000000};
  while (!got[0] && nanosleep(&ts, &ts) < 0 && errno == EINTR) {
  }
  CHECK(got[0] == 1 && code == 0x80);  // SI_KERNEL
  CHECK(getitimer(ITIMER_REAL, &cur) == 0 && us(cur.it_value) == 0);
  // A periodic timer, then its old value when disarmed.
  got[0] = 0;
  v = (struct itimerval){.it_interval = {0, 10000}, .it_value = {0, 10000}};
  CHECK(setitimer(ITIMER_REAL, &v, NULL) == 0);
  while (got[0] < 3) pause();
  struct itimerval off = {0};
  CHECK(setitimer(ITIMER_REAL, &off, &old) == 0);
  CHECK(us(old.it_interval) == 10000 && us(old.it_value) > 0 && us(old.it_value) <= 10000);
}

static void alarm_seconds(void) {
  CHECK(alarm(5) == 0);
  unsigned left = alarm(0);
  CHECK(left >= 4 && left <= 5);
  CHECK(alarm(0) == 0);
}

static void invalid(void) {
  struct itimerval v = {.it_value = {0, 1000000}};
  errno = 0;
  CHECK(setitimer(ITIMER_REAL, &v, NULL) == -1 && errno == EINVAL);
  v = (struct itimerval){.it_value = {-1, 0}};
  CHECK(setitimer(ITIMER_REAL, &v, NULL) == -1 && errno == EINVAL);
  struct itimerval cur;
  CHECK(getitimer(3, &cur) == -1 && errno == EINVAL);
}

static void spin_until(volatile sig_atomic_t* flag) {
  struct timespec start, now;
  clock_gettime(CLOCK_MONOTONIC, &start);
  volatile unsigned long n = 0;
  do {
    for (int i = 0; i < 100000; i++) n++;
    clock_gettime(CLOCK_MONOTONIC, &now);
  } while (!*flag && now.tv_sec - start.tv_sec < 10);
}

static void cpu_timers(void) {
  got[1] = got[2] = 0;
  struct itimerval v = {.it_value = {0, 50000}};
  CHECK(setitimer(ITIMER_VIRTUAL, &v, NULL) == 0);
  CHECK(setitimer(ITIMER_PROF, &v, NULL) == 0);
  struct itimerval cur;
  CHECK(getitimer(ITIMER_VIRTUAL, &cur) == 0 && us(cur.it_value) > 0 && us(cur.it_value) <= 50000);
  spin_until(&got[1]);
  spin_until(&got[2]);
  CHECK(got[1] == 1 && got[2] == 1);
}

static void not_inherited(void) {
  struct itimerval v = {.it_value = {10, 0}}, off = {0};
  CHECK(setitimer(ITIMER_REAL, &v, NULL) == 0);
  FORK_OR_SKIP(p);
  if (p == 0) {
    struct itimerval cur;
    _exit(getitimer(ITIMER_REAL, &cur) == 0 && us(cur.it_value) == 0 ? 0 : 1);
  }
  int st;
  CHECK(waitpid(p, &st, 0) == p && WIFEXITED(st) && WEXITSTATUS(st) == 0);
  CHECK(setitimer(ITIMER_REAL, &off, NULL) == 0);
}

// A timer armed before execve runs on in the new program (argv[1] is
// "after-exec" there), whether the image is replaced in place or, with a
// POSIX timer (which execve deletes), by a new linux-run.
static const char* self;
static void exec_with(int posix_timer) {
  FORK_OR_SKIP(p);
  if (p == 0) {
    timer_t t;
    if (posix_timer && timer_create(CLOCK_MONOTONIC, NULL, &t) != 0) _exit(3);
    struct itimerval v = {.it_value = {10, 0}};
    setitimer(ITIMER_REAL, &v, NULL);
    execl(self, self, "after-exec", (char*)NULL);
    _exit(2);
  }
  int st;
  CHECK(waitpid(p, &st, 0) == p && WIFEXITED(st) && WEXITSTATUS(st) == 0);
}

static void kept_across_exec(void) { exec_with(0); }
static void kept_across_exec_anew(void) { exec_with(1); }

int main(int argc, char** argv) {
  self = argv[0];
  if (argc > 1 && strcmp(argv[1], "after-exec") == 0) {
    struct itimerval cur;
    return getitimer(ITIMER_REAL, &cur) == 0 && us(cur.it_value) > 8000000 &&
                   us(cur.it_value) <= 10000000
               ? 0
               : 1;
  }
  catch_signals();
  RUN(real_timer);
  RUN(alarm_seconds);
  RUN(invalid);
  RUN(cpu_timers);
  RUN(not_inherited);
  RUN(kept_across_exec);
  RUN(kept_across_exec_anew);
  DONE();
}
