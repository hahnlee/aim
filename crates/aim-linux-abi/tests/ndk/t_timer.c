// POSIX timers as the Bluetooth stack's alarms use them: signal and
// thread-directed notification, overruns, SIGEV_NONE and bionic's
// SIGEV_THREAD.
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

static int64_t now_ms(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static volatile int ticks;
static volatile int bad_info;

static void on_tick(int sig, siginfo_t* si, void* uc) {
  (void)uc;
  if (sig != SIGRTMIN + 2 || si->si_code != SI_TIMER || si->si_value.sival_int != 42) bad_info++;
  ticks++;
}

static void periodic_signal(void) {
  struct sigaction sa = {.sa_sigaction = on_tick, .sa_flags = SA_SIGINFO | SA_RESTART};
  CHECK(sigaction(SIGRTMIN + 2, &sa, NULL) == 0);
  struct sigevent ev = {.sigev_notify = SIGEV_SIGNAL, .sigev_signo = SIGRTMIN + 2};
  ev.sigev_value.sival_int = 42;
  timer_t t;
  CHECK(timer_create(CLOCK_MONOTONIC, &ev, &t) == 0);
  struct itimerspec its = {.it_value = {0, 10000000}, .it_interval = {0, 10000000}};
  int64_t t0 = now_ms();
  CHECK(timer_settime(t, 0, &its, NULL) == 0);
  while (ticks < 5 && now_ms() - t0 < 2000) usleep(1000);
  int64_t took = now_ms() - t0;
  CHECK(ticks >= 5 && took >= 45 && !bad_info);
  struct itimerspec cur;
  CHECK(timer_gettime(t, &cur) == 0 && cur.it_interval.tv_nsec == 10000000);
  CHECK(cur.it_value.tv_sec == 0 && cur.it_value.tv_nsec > 0 && cur.it_value.tv_nsec <= 10000000);
  CHECK(timer_delete(t) == 0);
  int seen = ticks;
  usleep(30000);
  CHECK(ticks <= seen + 1);
  errno = 0;
  CHECK(timer_gettime(t, &cur) == -1 && errno == EINVAL);
}

static pid_t target_tid;
static int target_go[2];
static volatile int overrun_seen = -1;
static volatile int target_ok;
static timer_t target_timer;

// Blocks the timer's signal, lets it expire several times, then takes it:
// one signal, with the other expiries as overruns.
static void* target(void* arg) {
  (void)arg;
  sigset_t set;
  sigemptyset(&set);
  sigaddset(&set, SIGUSR2);
  pthread_sigmask(SIG_BLOCK, &set, NULL);
  target_tid = gettid();
  char c;
  read(target_go[0], &c, 1);
  siginfo_t si;
  int sig = sigwaitinfo(&set, &si);
  if (sig == SIGUSR2 && si.si_code == SI_TIMER) {
    overrun_seen = si.si_overrun;
    target_ok = timer_getoverrun(target_timer) == si.si_overrun;
  }
  return NULL;
}

static void thread_id_and_overrun(void) {
  sigset_t set;
  sigemptyset(&set);
  sigaddset(&set, SIGUSR2);
  pthread_sigmask(SIG_BLOCK, &set, NULL);
  CHECK(pipe(target_go) == 0);
  pthread_t th;
  CHECK(pthread_create(&th, NULL, target, NULL) == 0);
  while (!__atomic_load_n(&target_tid, __ATOMIC_SEQ_CST)) usleep(1000);
  struct sigevent ev = {.sigev_notify = SIGEV_THREAD_ID, .sigev_signo = SIGUSR2};
  ev._sigev_un._tid = target_tid;
  CHECK(timer_create(CLOCK_MONOTONIC, &ev, &target_timer) == 0);
  struct itimerspec its = {.it_value = {0, 5000000}, .it_interval = {0, 5000000}};
  CHECK(timer_settime(target_timer, 0, &its, NULL) == 0);
  usleep(60000);
  CHECK(write(target_go[1], "x", 1) == 1);
  CHECK(pthread_join(th, NULL) == 0);
  CHECK(overrun_seen >= 3 && target_ok);
  CHECK(timer_delete(target_timer) == 0);
  ev._sigev_un._tid = 12345;
  timer_t bad;
  errno = 0;
  CHECK(timer_create(CLOCK_MONOTONIC, &ev, &bad) == -1 && errno == EINVAL);
  pthread_sigmask(SIG_UNBLOCK, &set, NULL);
}

static void none_and_abstime(void) {
  struct sigevent ev = {.sigev_notify = SIGEV_NONE};
  timer_t t;
  CHECK(timer_create(CLOCK_REALTIME, &ev, &t) == 0);
  struct timespec now;
  clock_gettime(CLOCK_REALTIME, &now);
  struct itimerspec its = {.it_value = {now.tv_sec + 2, now.tv_nsec}};
  CHECK(timer_settime(t, TIMER_ABSTIME, &its, NULL) == 0);
  struct itimerspec cur, old;
  CHECK(timer_gettime(t, &cur) == 0);
  CHECK(cur.it_value.tv_sec == 1 || (cur.it_value.tv_sec == 2 && cur.it_value.tv_nsec == 0));
  struct itimerspec off = {0};
  CHECK(timer_settime(t, 0, &off, &old) == 0 && old.it_value.tv_sec >= 1);
  CHECK(timer_gettime(t, &cur) == 0 && cur.it_value.tv_sec == 0 && cur.it_value.tv_nsec == 0);
  CHECK(timer_delete(t) == 0);
  // No sigevent: SIGALRM, whose default would end the process.
  signal(SIGALRM, SIG_IGN);
  CHECK(timer_create(CLOCK_BOOTTIME, NULL, &t) == 0);
  struct itimerspec soon = {.it_value = {0, 1000000}};
  CHECK(timer_settime(t, 0, &soon, NULL) == 0);
  usleep(10000);
  CHECK(timer_delete(t) == 0);
  errno = 0;
  CHECK(timer_create(CLOCK_PROCESS_CPUTIME_ID + 100, &ev, &t) == -1 && errno == EINVAL);
}

static volatile int callbacks;

static void on_expiry(union sigval v) {
  if (v.sival_int == 7) callbacks++;
}

// bionic runs SIGEV_THREAD callbacks on its own thread, driven by a
// thread-directed real-time signal.
static void sigev_thread(void) {
  struct sigevent ev = {.sigev_notify = SIGEV_THREAD, .sigev_notify_function = on_expiry};
  ev.sigev_value.sival_int = 7;
  timer_t t;
  CHECK(timer_create(CLOCK_MONOTONIC, &ev, &t) == 0);
  struct itimerspec its = {.it_value = {0, 5000000}, .it_interval = {0, 5000000}};
  CHECK(timer_settime(t, 0, &its, NULL) == 0);
  int64_t t0 = now_ms();
  while (callbacks < 3 && now_ms() - t0 < 2000) usleep(1000);
  CHECK(callbacks >= 3);
  CHECK(timer_delete(t) == 0);
}

int main(void) {
  RUN(periodic_signal);
  RUN(thread_id_and_overrun);
  RUN(none_and_abstime);
  RUN(sigev_thread);
  DONE();
}
