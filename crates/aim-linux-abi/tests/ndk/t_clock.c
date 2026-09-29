// Linux clock semantics, after LTP's clock_gettime, clock_getres,
// clock_nanosleep and timerfd tests: ids, resolutions, the order between
// clocks, CPU clocks, and sleeps and timers on each clock. The calls go
// through syscall(2), not bionic's vDSO path.
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/capability.h>
#include <sys/syscall.h>
#include <sys/sysinfo.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "check.h"

#define TICK_NS 4000000  // CONFIG_HZ=250
#define CLOCK_TAI_ 11
// An encoded CPU clock: ~pid << 3 | per-thread 4 | which.
#define CPU_CLOCK(id, bits) ((clockid_t)(~(unsigned)(id) << 3 | (bits)))

static int gettime(clockid_t id, struct timespec* ts) {
  return syscall(SYS_clock_gettime, id, ts);
}

static int64_t ns_of(clockid_t id) {
  struct timespec ts;
  if (gettime(id, &ts) != 0) return -1;
  return ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void spin_ms(int ms) {
  int64_t end = ns_of(CLOCK_MONOTONIC) + ms * 1000000LL;
  while (ns_of(CLOCK_MONOTONIC) < end) {
  }
}

static void getres(void) {
  const clockid_t fine[] = {CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_MONOTONIC_RAW,
                            CLOCK_BOOTTIME, CLOCK_REALTIME_ALARM, CLOCK_BOOTTIME_ALARM,
                            CLOCK_TAI_, CLOCK_PROCESS_CPUTIME_ID, CLOCK_THREAD_CPUTIME_ID};
  struct timespec r;
  for (unsigned i = 0; i < sizeof(fine) / sizeof(fine[0]); i++) {
    CHECK(syscall(SYS_clock_getres, fine[i], &r) == 0 && r.tv_sec == 0 && r.tv_nsec == 1);
  }
  CHECK(syscall(SYS_clock_getres, CLOCK_REALTIME_COARSE, &r) == 0 && r.tv_nsec == TICK_NS);
  CHECK(syscall(SYS_clock_getres, CLOCK_MONOTONIC_COARSE, &r) == 0 && r.tv_nsec == TICK_NS);
  CHECK(syscall(SYS_clock_getres, CLOCK_MONOTONIC, NULL) == 0);
  CHECK(syscall(SYS_clock_getres, 10, &r) == -1 && errno == EINVAL);
  CHECK(syscall(SYS_clock_getres, 12, &r) == -1 && errno == EINVAL);
  CHECK(syscall(SYS_clock_getres, 100, NULL) == -1 && errno == EINVAL);
  struct timespec ts;
  CHECK(gettime(10, &ts) == -1 && errno == EINVAL);
}

// Nanosecond resolution: not every reading is a whole microsecond.
static void resolution(void) {
  const clockid_t ids[] = {CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_MONOTONIC_RAW, CLOCK_BOOTTIME};
  for (unsigned i = 0; i < sizeof(ids) / sizeof(ids[0]); i++) {
    int sub_us = 0;
    for (int k = 0; k < 1000; k++) sub_us += ns_of(ids[i]) % 1000 != 0;
    CHECK(sub_us > 500);
  }
}

static void order(void) {
  int64_t last = ns_of(CLOCK_MONOTONIC);
  for (int k = 0; k < 100000; k++) {
    int64_t t = ns_of(CLOCK_MONOTONIC);
    CHECK(t >= last);
    last = t;
  }
  // BOOTTIME also counts the host's sleep, so it is never behind.
  int64_t mono = ns_of(CLOCK_MONOTONIC), boot = ns_of(CLOCK_BOOTTIME);
  CHECK(boot >= mono);
  CHECK(llabs(ns_of(CLOCK_BOOTTIME_ALARM) - ns_of(CLOCK_BOOTTIME)) < 1000000);
  CHECK(llabs(ns_of(CLOCK_MONOTONIC_RAW) - ns_of(CLOCK_MONOTONIC)) < 1000000);
  int64_t coarse = ns_of(CLOCK_MONOTONIC_COARSE);
  CHECK(coarse <= ns_of(CLOCK_MONOTONIC));
  int64_t real = ns_of(CLOCK_REALTIME);
  CHECK(real > 1600000000LL * 1000000000LL);
  CHECK(llabs(ns_of(CLOCK_REALTIME_COARSE) - real) < 10000000);
  CHECK(llabs(ns_of(CLOCK_REALTIME_ALARM) - real) < 10000000);
  CHECK(llabs(ns_of(CLOCK_TAI_) - real) < 10000000);
  struct timeval tv;
  struct timezone tz = {.tz_minuteswest = 99, .tz_dsttime = 99};
  CHECK(syscall(SYS_gettimeofday, &tv, &tz) == 0);
  CHECK(llabs(tv.tv_sec * 1000000000LL + tv.tv_usec * 1000LL - ns_of(CLOCK_REALTIME)) < 10000000);
  CHECK(tz.tz_minuteswest == 0 && tz.tz_dsttime == 0);
}

// /proc/uptime and sysinfo count CLOCK_BOOTTIME.
static void uptime(void) {
  FILE* f = fopen("/proc/uptime", "r");
  CHECK(f != NULL);
  double up = 0;
  int n = fscanf(f, "%lf", &up);
  fclose(f);
  CHECK(n == 1);
  CHECK(llabs((int64_t)(up * 1e9) - ns_of(CLOCK_BOOTTIME)) < 100000000);
  struct sysinfo si;
  CHECK(sysinfo(&si) == 0);
  CHECK(llabs((int64_t)si.uptime - ns_of(CLOCK_BOOTTIME) / 1000000000) <= 1);
}

static volatile int stop;

static void* spinner(void* arg) {
  (void)arg;
  while (!stop) {
  }
  return NULL;
}

static void cpu_clocks(void) {
  int64_t t0 = ns_of(CLOCK_THREAD_CPUTIME_ID), p0 = ns_of(CLOCK_PROCESS_CPUTIME_ID);
  spin_ms(30);
  int64_t t1 = ns_of(CLOCK_THREAD_CPUTIME_ID), p1 = ns_of(CLOCK_PROCESS_CPUTIME_ID);
  CHECK(t1 - t0 >= 15000000 && p1 - p0 >= 15000000);
  CHECK(p1 >= t1);

  clockid_t self;
  CHECK(pthread_getcpuclockid(pthread_self(), &self) == 0);
  CHECK(llabs(ns_of(self) - ns_of(CLOCK_THREAD_CPUTIME_ID)) < 5000000);
  clockid_t proc;
  CHECK(clock_getcpuclockid(getpid(), &proc) == 0);
  CHECK(llabs(ns_of(proc) - ns_of(CLOCK_PROCESS_CPUTIME_ID)) < 5000000);

  // Another thread's clock counts that thread's time, not ours.
  pthread_t th;
  stop = 0;
  CHECK(pthread_create(&th, NULL, spinner, NULL) == 0);
  clockid_t other;
  CHECK(pthread_getcpuclockid(th, &other) == 0);
  usleep(50000);
  int64_t o = ns_of(other);
  struct timespec r;
  int res = syscall(SYS_clock_getres, other, &r) == 0 ? r.tv_nsec : -errno;
  stop = 1;
  pthread_join(th, NULL);
  CHECK(o >= 10000000 && res == 1);
  // A thread that exited has no clock.
  CHECK(syscall(SYS_clock_getres, other, &r) == -1 && errno == EINVAL);

  // VIRT is tick-based; which 3 and ids of no process or thread are
  // invalid.
  clockid_t virt = CPU_CLOCK(0, 4 | 1);
  CHECK(ns_of(virt) >= 0);
  CHECK(syscall(SYS_clock_getres, virt, &r) == 0 && r.tv_nsec == TICK_NS);
  struct timespec ts;
  CHECK(gettime(CPU_CLOCK(0, 3), &ts) == -1 && errno == EINVAL);
  CHECK(gettime(CPU_CLOCK(0x3ffffff, 2), &ts) == -1 && errno == EINVAL);
  CHECK(gettime(CPU_CLOCK(0x3ffffff, 4 | 2), &ts) == -1 && errno == EINVAL);
}

static int64_t nsleep(clockid_t id, int flags, int64_t ns) {
  struct timespec req = {ns / 1000000000, ns % 1000000000};
  return syscall(SYS_clock_nanosleep, id, flags, &req, NULL) == 0 ? 0 : -errno;
}

static void nanosleep_clocks(void) {
  int64_t t0 = ns_of(CLOCK_MONOTONIC);
  CHECK(nsleep(CLOCK_MONOTONIC, 0, 10000000) == 0);
  CHECK(ns_of(CLOCK_MONOTONIC) - t0 >= 10000000);
  const clockid_t abs[] = {CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_BOOTTIME, CLOCK_TAI_};
  for (unsigned i = 0; i < sizeof(abs) / sizeof(abs[0]); i++) {
    int64_t deadline = ns_of(abs[i]) + 10000000;
    CHECK(nsleep(abs[i], TIMER_ABSTIME, deadline) == 0);
    CHECK(ns_of(abs[i]) >= deadline);
  }
  CHECK(nsleep(CLOCK_MONOTONIC_RAW, 0, 1000) == -EOPNOTSUPP);
  CHECK(nsleep(CLOCK_MONOTONIC_COARSE, 0, 1000) == -EOPNOTSUPP);
  CHECK(nsleep(CLOCK_THREAD_CPUTIME_ID, 0, 1000) == -EOPNOTSUPP);
  CHECK(nsleep(100, 0, 1000) == -EINVAL);
}

static void timerfd_clocks(void) {
  int fd = timerfd_create(CLOCK_BOOTTIME, 0);
  CHECK(fd >= 0);
  int64_t deadline = ns_of(CLOCK_BOOTTIME) + 20000000;
  struct itimerspec its = {.it_value = {deadline / 1000000000, deadline % 1000000000}};
  CHECK(timerfd_settime(fd, TFD_TIMER_ABSTIME, &its, NULL) == 0);
  uint64_t n = 0;
  CHECK(read(fd, &n, 8) == 8 && n == 1);
  CHECK(ns_of(CLOCK_BOOTTIME) >= deadline);
  close(fd);
  CHECK(timerfd_create(CLOCK_TAI_, 0) == -1 && errno == EINVAL);
  CHECK(timerfd_create(CLOCK_MONOTONIC_RAW, 0) == -1 && errno == EINVAL);
}

// Alarm clocks need CAP_WAKE_ALARM.
static void alarm_capability(void) {
  int fd = timerfd_create(CLOCK_BOOTTIME_ALARM, 0);
  CHECK(fd >= 0);
  close(fd);
  FORK_OR_SKIP(p);
  if (p == 0) {
    struct __user_cap_header_struct h = {.version = _LINUX_CAPABILITY_VERSION_3};
    struct __user_cap_data_struct d[2] = {{0}};
    int ok = syscall(SYS_capset, &h, d) == 0 &&
             timerfd_create(CLOCK_BOOTTIME_ALARM, 0) == -1 && errno == EPERM &&
             nsleep(CLOCK_REALTIME_ALARM, 0, 1000) == -EPERM &&
             timerfd_create(CLOCK_BOOTTIME, 0) >= 0;
    _exit(ok ? 0 : 1);
  }
  int st;
  CHECK(waitpid(p, &st, 0) == p && WIFEXITED(st) && WEXITSTATUS(st) == 0);
}

// Not a check: what a clock read costs without a vDSO.
static void cost(void) {
  const int n = 200000;
  struct timespec ts;
  int64_t t0 = ns_of(CLOCK_MONOTONIC);
  for (int k = 0; k < n; k++) gettime(CLOCK_MONOTONIC, &ts);
  int64_t t1 = ns_of(CLOCK_MONOTONIC);
  for (int k = 0; k < n; k++) clock_gettime(CLOCK_MONOTONIC, &ts);
  int64_t t2 = ns_of(CLOCK_MONOTONIC);
  printf("clock_gettime: syscall %lld ns, bionic %lld ns\n", (long long)((t1 - t0) / n),
         (long long)((t2 - t1) / n));
}

int main(void) {
  RUN(getres);
  RUN(resolution);
  RUN(order);
  RUN(uptime);
  RUN(cpu_clocks);
  RUN(nanosleep_clocks);
  RUN(timerfd_clocks);
  RUN(alarm_capability);
  cost();
  DONE();
}
