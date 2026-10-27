// CPU-clock deadlines use consumed CPU time, including encoded clocks from
// clock_getcpuclockid/pthread_getcpuclockid. Exercise the kernel timer ABI.
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

#define CPU_CLOCK(id, bits) ((clockid_t)(~(unsigned)(id) << 3 | (bits)))
#define MS 1000000LL

static int64_t now(clockid_t c) {
  struct timespec ts;
  if (clock_gettime(c, &ts)) return -1;
  return ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static struct timespec ts(int64_t n) {
  return (struct timespec){n / 1000000000LL, n % 1000000000LL};
}

static void chunk(void) {
  for (volatile int k = 0; k < 100000; k++) {
  }
}

static void burn(int64_t ns) {
  int64_t end = now(CLOCK_THREAD_CPUTIME_ID) + ns;
  while (now(CLOCK_THREAD_CPUTIME_ID) < end) chunk();
}

static int create(clockid_t c, int notify, int tid) {
  struct sigevent ev = {.sigev_notify = notify, .sigev_signo = SIGUSR1};
  ev.sigev_value.sival_int = 408;
  ev._sigev_un._tid = tid;
  int id = -1;
  if (syscall(SYS_timer_create, c, &ev, &id)) return -1;
  return id;
}

static int set(int id, int flags, int64_t value, int64_t interval, struct itimerspec* old) {
  struct itimerspec spec = {.it_value = ts(value), .it_interval = ts(interval)};
  return syscall(SYS_timer_settime, id, flags, &spec, old);
}

static int64_t left(int id) {
  struct itimerspec spec;
  if (syscall(SYS_timer_gettime, id, &spec)) return -1;
  return spec.it_value.tv_sec * 1000000000LL + spec.it_value.tv_nsec;
}

static volatile sig_atomic_t hits, wrong;
static pid_t receiver;

static void on_timer(int sig, siginfo_t* si, void* uc) {
  (void)uc;
  if (sig != SIGUSR1 || si->si_code != SI_TIMER || si->si_value.sival_int != 408 ||
      (receiver && gettid() != receiver)) wrong = 1;
  hits++;
}

static void signal_timers(void) {
  struct sigaction sa = {.sa_sigaction = on_timer, .sa_flags = SA_SIGINFO};
  CHECK(sigaction(SIGUSR1, &sa, NULL) == 0);
  clockid_t p, t;
  CHECK(clock_getcpuclockid(getpid(), &p) == 0);
  CHECK(pthread_getcpuclockid(pthread_self(), &t) == 0);
  const clockid_t clocks[] = {CLOCK_PROCESS_CPUTIME_ID, CLOCK_THREAD_CPUTIME_ID,
                              p, t, CPU_CLOCK(0, 0), CPU_CLOCK(0, 1), CPU_CLOCK(0, 4),
                              CPU_CLOCK(0, 5)};
  for (unsigned i = 0; i < sizeof(clocks) / sizeof(clocks[0]); i++) {
    hits = wrong = 0;
    receiver = i & 1 ? gettid() : 0;
    int id = create(clocks[i], receiver ? SIGEV_THREAD_ID : SIGEV_SIGNAL, receiver);
    CHECK(id >= 0);
    CHECK(set(id, 0, 80 * MS, 0, NULL) == 0);
    usleep(40000);
    CHECK(hits == 0 && left(id) > 40 * MS);
    int64_t end = now(CLOCK_MONOTONIC) + 5000000000LL;
    while (!hits && now(CLOCK_MONOTONIC) < end) chunk();
    CHECK(hits == 1 && !wrong && left(id) == 0);
    // An absolute expiry already in the past fires even while we sleep.
    hits = 0;
    CHECK(set(id, TIMER_ABSTIME, now(clocks[i]) - 1, 0, NULL) == 0);
    end = now(CLOCK_MONOTONIC) + 2000000000LL;
    while (!hits && now(CLOCK_MONOTONIC) < end) usleep(1000);
    CHECK(hits == 1 && !wrong);
    CHECK(syscall(SYS_timer_delete, id) == 0);
  }
}

static int worker_go[2];
static int worker_stop;

static void* spinner(void* arg) {
  (void)arg;
  char c;
  if (read(worker_go[0], &c, 1) != 1) return NULL;
  while (!__atomic_load_n(&worker_stop, __ATOMIC_SEQ_CST)) chunk();
  return NULL;
}

static void start_worker(pthread_t* th) {
  worker_stop = 0;
  if (pipe(worker_go) || pthread_create(th, NULL, spinner, NULL)) _exit(2);
}

static void stop_worker(pthread_t th) {
  __atomic_store_n(&worker_stop, 1, __ATOMIC_SEQ_CST);
  pthread_join(th, NULL);
  close(worker_go[0]);
  close(worker_go[1]);
}

static void thread_target_and_overrun(void) {
  sigset_t mask;
  sigemptyset(&mask);
  sigaddset(&mask, SIGUSR1);
  CHECK(pthread_sigmask(SIG_BLOCK, &mask, NULL) == 0);
  pthread_t th;
  start_worker(&th);
  clockid_t c;
  CHECK(pthread_getcpuclockid(th, &c) == 0);
  // The worker is parked: burning in the creator cannot expire its timer.
  int id = create(c, SIGEV_THREAD_ID, gettid());
  CHECK(id >= 0 && set(id, 0, 10 * MS, 10 * MS, NULL) == 0);
  burn(30 * MS);
  CHECK(left(id) > 5 * MS);
  CHECK(write(worker_go[1], "x", 1) == 1);
  int64_t end = now(CLOCK_MONOTONIC) + 5000000000LL;
  while (now(c) < 80 * MS && now(CLOCK_MONOTONIC) < end) usleep(1000);
  CHECK(now(c) >= 80 * MS);
  siginfo_t si;
  struct timespec timeout = {2, 0};
  CHECK(sigtimedwait(&mask, &si, &timeout) == SIGUSR1);
  CHECK(si.si_code == SI_TIMER && si.si_value.sival_int == 408 && si.si_overrun >= 4);
  CHECK(syscall(SYS_timer_getoverrun, id) == si.si_overrun);
  CHECK(syscall(SYS_timer_delete, id) == 0);
  stop_worker(th);
  while (sigtimedwait(&mask, &si, &(struct timespec){0}) == SIGUSR1) {
  }
  CHECK(pthread_sigmask(SIG_UNBLOCK, &mask, NULL) == 0);
}

static void none_and_dead_target(void) {
  int id = create(CLOCK_THREAD_CPUTIME_ID, SIGEV_NONE, 0);
  CHECK(id >= 0 && set(id, 0, 40 * MS, 0, NULL) == 0);
  usleep(30000);
  CHECK(left(id) > 20 * MS);
  burn(50 * MS);
  CHECK(left(id) == 0 && syscall(SYS_timer_getoverrun, id) == 0);
  int64_t end = now(CLOCK_THREAD_CPUTIME_ID) + 40 * MS;
  CHECK(set(id, TIMER_ABSTIME, end, 10 * MS, NULL) == 0);
  struct itimerspec old;
  CHECK(set(id, 0, 0, 0, &old) == 0 && old.it_interval.tv_nsec == 10 * MS);
  CHECK(old.it_value.tv_sec == 0 && old.it_value.tv_nsec > 20 * MS && left(id) == 0);
  CHECK(set(id, 2, 1, 0, NULL) == -1 && errno == EINVAL);
  CHECK(syscall(SYS_timer_delete, id) == 0);
  pthread_t th;
  start_worker(&th);
  clockid_t c;
  CHECK(pthread_getcpuclockid(th, &c) == 0);
  id = create(c, SIGEV_NONE, 0);
  CHECK(id >= 0 && set(id, 0, 100 * MS, 0, NULL) == 0);
  CHECK(write(worker_go[1], "x", 1) == 1);
  stop_worker(th);
  CHECK(left(id) == 0);
  CHECK(set(id, 0, 10 * MS, 0, NULL) == -1 && errno == ESRCH);
  CHECK(syscall(SYS_timer_delete, id) == 0);
  CHECK(create(c, SIGEV_NONE, 0) == -1 && errno == EINVAL);
  CHECK(create(CPU_CLOCK(0, 3), SIGEV_NONE, 0) == -1 && errno == EINVAL);
  CHECK(create(CPU_CLOCK(0x3ffffff, 2), SIGEV_NONE, 0) == -1 && errno == EINVAL);
}

static int sleep_cpu(clockid_t c, int flags, int64_t value, struct timespec* rem) {
  struct timespec req = ts(value);
  return syscall(SYS_clock_nanosleep, c, flags, &req, rem);
}

static void sleep_clocks(void) {
  clockid_t self;
  CHECK(pthread_getcpuclockid(pthread_self(), &self) == 0);
  CHECK(sleep_cpu(CLOCK_THREAD_CPUTIME_ID, 0, 1, NULL) == -1 && errno == EINVAL);
  CHECK(sleep_cpu(self, TIMER_ABSTIME, 1, NULL) == -1 && errno == EINVAL);
  CHECK(sleep_cpu(CPU_CLOCK(0, 4), 0, 1, NULL) == -1 && errno == EINVAL);
  CHECK(sleep_cpu(CPU_CLOCK(0x3ffffff, 2), 0, 1, NULL) == -1 && errno == EINVAL);
  pthread_t th;
  start_worker(&th);
  clockid_t proc, other;
  CHECK(clock_getcpuclockid(getpid(), &proc) == 0);
  CHECK(pthread_getcpuclockid(th, &other) == 0);
  CHECK(write(worker_go[1], "x", 1) == 1);
  const clockid_t clocks[] = {CLOCK_PROCESS_CPUTIME_ID, proc, other};
  for (unsigned i = 0; i < sizeof(clocks) / sizeof(clocks[0]); i++) {
    int64_t before = now(clocks[i]);
    CHECK(sleep_cpu(clocks[i], 0, 30 * MS, NULL) == 0);
    CHECK(now(clocks[i]) >= before + 30 * MS);
    int64_t end = now(clocks[i]) + 30 * MS;
    struct timespec rem = {123, 456};
    CHECK(sleep_cpu(clocks[i], TIMER_ABSTIME, end, &rem) == 0);
    CHECK(now(clocks[i]) >= end && rem.tv_sec == 123 && rem.tv_nsec == 456);
    CHECK(sleep_cpu(clocks[i], TIMER_ABSTIME, 1, NULL) == 0);
  }
  stop_worker(th);
}

static void on_interrupt(int sig) { (void)sig; }

static void sleep_interrupted(void) {
  struct sigaction sa = {.sa_handler = on_interrupt, .sa_flags = SA_RESTART};
  CHECK(sigaction(SIGALRM, &sa, NULL) == 0);
  // No CPU burner: a wall timer interrupts a CPU sleep with CPU time left.
  struct sigevent ev = {.sigev_notify = SIGEV_SIGNAL, .sigev_signo = SIGALRM};
  timer_t t;
  CHECK(timer_create(CLOCK_MONOTONIC, &ev, &t) == 0);
  struct itimerspec spec = {.it_value = {0, 30 * MS}};
  struct timespec rem = {123, 456};
  CHECK(timer_settime(t, 0, &spec, NULL) == 0);
  CHECK(sleep_cpu(CLOCK_PROCESS_CPUTIME_ID, 0, 200 * MS, &rem) == -1 && errno == EINTR);
  CHECK(rem.tv_sec == 0 && rem.tv_nsec > 100 * MS && rem.tv_nsec <= 200 * MS);
  rem = (struct timespec){123, 456};
  CHECK(timer_settime(t, 0, &spec, NULL) == 0);
  CHECK(sleep_cpu(CLOCK_PROCESS_CPUTIME_ID, TIMER_ABSTIME,
                  now(CLOCK_PROCESS_CPUTIME_ID) + 200 * MS, &rem) == -1 && errno == EINTR);
  CHECK(rem.tv_sec == 123 && rem.tv_nsec == 456);
  CHECK(timer_delete(t) == 0);
}

static void other_process(void) {
  int go[2];
  CHECK(pipe(go) == 0);
  int parent_timer = create(CLOCK_PROCESS_CPUTIME_ID, SIGEV_NONE, 0);
  CHECK(parent_timer >= 0);
  pid_t p = fork();
  CHECK(p >= 0);
  if (p == 0) {
    close(go[1]);
    char c;
    if (read(go[0], &c, 1) != 1) _exit(2);
    struct itimerspec spec;
    if (syscall(SYS_timer_gettime, parent_timer, &spec) != -1 || errno != EINVAL) _exit(3);
    burn(250 * MS);
    read(go[0], &c, 1);
    _exit(0);
  }
  close(go[0]);
  clockid_t c;
  CHECK(clock_getcpuclockid(p, &c) == 0);
  hits = wrong = 0;
  receiver = gettid();
  int id = create(c, SIGEV_THREAD_ID, receiver);
  CHECK(id >= 0 && set(id, 0, 30 * MS, 0, NULL) == 0);
  int64_t end = now(c) + 100 * MS;
  int64_t before = now(c);
  CHECK(write(go[1], "x", 1) == 1);
  // Preserve the unslept CPU time when the timer interrupts a relative sleep.
  struct timespec rem = ts(40 * MS);
  int r;
  do {
    struct timespec req = rem;
    r = syscall(SYS_clock_nanosleep, c, 0, &req, &rem);
  } while (r == -1 && errno == EINTR);
  CHECK(r == 0 && now(c) >= before + 40 * MS);
  // An absolute retry keeps the original deadline.
  do { r = sleep_cpu(c, TIMER_ABSTIME, end, NULL); } while (r == -1 && errno == EINTR);
  CHECK(r == 0 && now(c) >= end && hits == 1 && !wrong);
  CHECK(syscall(SYS_timer_delete, id) == 0);
  CHECK(syscall(SYS_timer_delete, parent_timer) == 0);
  close(go[1]);
  int st;
  CHECK(waitpid(p, &st, 0) == p && WIFEXITED(st) && WEXITSTATUS(st) == 0);
}

int main(void) {
  RUN(signal_timers);
  RUN(thread_target_and_overrun);
  RUN(none_and_dead_target);
  RUN(sleep_clocks);
  RUN(sleep_interrupted);
  RUN(other_process);
  DONE();
}
