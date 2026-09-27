// P0 experiment 3b: how late do Darwin timed waits fire, and what reduces it?
// futex.c saw ~2 ms overshoot on 20 ms os_sync timeouts. Linux's default
// timer slack is 50 us, and ART / bionic timed waits assume roughly that.
#include <errno.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/thread_policy.h>
#include <os/clock.h>
#include <os/os_sync_wait_on_address.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/event.h>
#include <time.h>
#include <unistd.h>

extern int __ulock_wait2(uint32_t op, void *addr, uint64_t value, uint64_t timeout_ns, uint64_t value2);
#define UL_COMPARE_AND_WAIT 1
#define ULF_NO_ERRNO 0x01000000

static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }
static int cmp(const void *a, const void *b) { int64_t x = *(const int64_t *)a, y = *(const int64_t *)b; return x < y ? -1 : x > y; }
static uint32_t word;
static mach_timebase_info_data_t tb;

enum { N = 60 };
typedef void (*waitfn)(uint64_t ns);
static void w_os_sync_timeout(uint64_t ns) { os_sync_wait_on_address_with_timeout(&word, 0, 4, 0, OS_CLOCK_MACH_ABSOLUTE_TIME, ns); }
static void w_os_sync_deadline(uint64_t ns) {
  os_sync_wait_on_address_with_deadline(&word, 0, 4, 0, OS_CLOCK_MACH_ABSOLUTE_TIME, mach_absolute_time() + ns * tb.denom / tb.numer);
}
static void w_ulock(uint64_t ns) { __ulock_wait2(UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, &word, 0, ns, 0); }
static void w_nanosleep(uint64_t ns) { struct timespec ts = {0, (long)ns}; nanosleep(&ts, NULL); }
static void w_kevent_leeway0(uint64_t ns) {
  static int kq = -1;
  if (kq < 0) kq = kqueue();
  struct kevent64_s ev;
  EV_SET64(&ev, 1, EVFILT_TIMER, EV_ADD | EV_ONESHOT, NOTE_NSECONDS | NOTE_LEEWAY | NOTE_CRITICAL, ns, 0, 0, 0);
  ev.ext[1] = 0;  // leeway
  struct kevent64_s out;
  kevent64(kq, &ev, 1, &out, 1, 0, NULL);
}
static void w_mach_wait_until(uint64_t ns) { mach_wait_until(mach_absolute_time() + ns * tb.denom / tb.numer); }

static void run(const char *name, waitfn f, uint64_t ns) {
  int64_t late[N];
  for (int i = 0; i < N; i++) {
    uint64_t t0 = now_ns();
    f(ns);
    late[i] = (int64_t)(now_ns() - t0) - (int64_t)ns;
  }
  qsort(late, N, sizeof late[0], cmp);
  printf("    %-26s %6.0f us wait: late p50=%7.1f us p90=%7.1f us max=%7.1f us\n", name, ns / 1e3, late[N / 2] / 1e3,
         late[N * 9 / 10] / 1e3, late[N - 1] / 1e3);
}

static void suite(const char *label) {
  printf("  %s\n", label);
  uint64_t d[] = {1000000, 20000000};
  for (int i = 0; i < 2; i++) {
    run("os_sync ..._with_timeout", w_os_sync_timeout, d[i]);
    run("os_sync ..._with_deadline", w_os_sync_deadline, d[i]);
    run("__ulock_wait2", w_ulock, d[i]);
    run("nanosleep", w_nanosleep, d[i]);
    run("mach_wait_until", w_mach_wait_until, d[i]);
    run("kevent timer leeway=0", w_kevent_leeway0, d[i]);
  }
}

static void *qos_thread(void *arg) {
  qos_class_t q = (qos_class_t)(uintptr_t)arg;
  pthread_set_qos_class_self_np(q, 0);
  suite(q == QOS_CLASS_USER_INTERACTIVE ? "QOS_CLASS_USER_INTERACTIVE thread" : "QOS_CLASS_UTILITY thread");
  return NULL;
}

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  mach_timebase_info(&tb);
  suite("main thread (default QoS)");
  pthread_t t;
  pthread_create(&t, NULL, qos_thread, (void *)(uintptr_t)QOS_CLASS_USER_INTERACTIVE);
  pthread_join(t, NULL);
  pthread_create(&t, NULL, qos_thread, (void *)(uintptr_t)QOS_CLASS_UTILITY);
  pthread_join(t, NULL);
  return 0;
}
