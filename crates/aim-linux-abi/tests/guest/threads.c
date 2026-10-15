// Guest-side checks of threads, futexes and signals on the Linux syscall
// layer. Built with the NDK for aarch64-linux-android and run under
// linux-run with the image's own linker64 and bionic (tests/threads.rs).
//
// Usage: threads [TEST...]; no argument runs every test. Each test prints
// "ok <name>" or "FAIL <name>: why"; latencies print as "lat <name> ...".
// The process exits 0 when every selected test passed.
#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <linux/futex.h>
#include <pthread.h>
#include <sched.h>
#include <semaphore.h>
#include <setjmp.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static int failures;

#define CHECK(name, cond, ...)                   \
  do {                                           \
    if (!(cond)) {                               \
      printf("FAIL %s: ", name);                 \
      printf(__VA_ARGS__);                       \
      printf("\n");                              \
      failures++;                                \
      return;                                    \
    }                                            \
  } while (0)

// The generic timer, readable at EL0: finer than the guest's
// CLOCK_MONOTONIC on this layer (which has microsecond resolution).
static uint64_t now_ns(void) {
  uint64_t cnt, freq;
  __asm__ volatile("isb\n mrs %0, cntvct_el0" : "=r"(cnt));
  __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(freq));
  return (uint64_t)((__uint128_t)cnt * 1000000000u / freq);
}

static int cmp_u64(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return x < y ? -1 : x > y;
}

static void report(const char *name, uint64_t *v, int n) {
  qsort(v, (size_t)n, sizeof *v, cmp_u64);
  printf("lat %s p50=%.2fus p99=%.2fus (n=%d)\n", name, v[n / 2] / 1e3, v[n * 99 / 100] / 1e3, n);
}

static long futex(void *uaddr, int op, uint32_t val, const void *ts_or_val2, void *uaddr2, uint32_t val3) {
  return syscall(SYS_futex, uaddr, op, val, ts_or_val2, uaddr2, val3);
}

// ---- threads ------------------------------------------------------------------

static _Atomic uint64_t started_at;
static void *mark_start(void *arg) {
  atomic_store(&started_at, now_ns());
  return arg;
}

static void *return_tid(void *arg) {
  (void)arg;
  return (void *)(intptr_t)gettid();
}

static sem_t detached_done;
static void *detached_fn(void *arg) {
  (void)arg;
  char name[16] = {0};
  prctl(PR_SET_NAME, "detached-t");
  prctl(PR_GET_NAME, name);
  if (strcmp(name, "detached-t") == 0) sem_post(&detached_done);
  return NULL;
}

static void test_create_join_detach(void) {
  const char *t = "create_join_detach";
  enum { N = 64 };
  pthread_t th[N];
  pid_t tids[N];
  for (int i = 0; i < N; i++) CHECK(t, pthread_create(&th[i], NULL, return_tid, NULL) == 0, "create %d", i);
  for (int i = 0; i < N; i++) {
    void *r;
    CHECK(t, pthread_join(th[i], &r) == 0, "join %d", i);
    tids[i] = (pid_t)(intptr_t)r;
    CHECK(t, tids[i] > 0 && tids[i] != getpid(), "tid %d = %d", i, tids[i]);
  }
  for (int i = 0; i < N; i++)
    for (int j = i + 1; j < N; j++) CHECK(t, tids[i] != tids[j], "live threads share tid %d", tids[i]);
  sem_init(&detached_done, 0, 0);
  pthread_attr_t a;
  pthread_attr_init(&a);
  pthread_attr_setdetachstate(&a, PTHREAD_CREATE_DETACHED);
  for (int i = 0; i < 16; i++) CHECK(t, pthread_create(&th[i], &a, detached_fn, NULL) == 0, "detached create");
  for (int i = 0; i < 16; i++) {
    struct timespec dl;
    clock_gettime(CLOCK_REALTIME, &dl);
    dl.tv_sec += 5;
    CHECK(t, sem_timedwait(&detached_done, &dl) == 0, "detached thread %d did not finish (errno %d)", i, errno);
  }
  printf("ok %s\n", t);

  enum { M = 2000 };
  static uint64_t start[M], total[M];
  for (int i = 0; i < M; i++) {
    pthread_t p;
    uint64_t t0 = now_ns();
    pthread_create(&p, NULL, mark_start, NULL);
    pthread_join(p, NULL);
    total[i] = now_ns() - t0;
    start[i] = atomic_load(&started_at) - t0;
  }
  report("pthread_create->running", start, M);
  report("pthread_create+join", total, M);
}

// ---- mutex and condition variable -----------------------------------------------

static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static long counter;
static int queue_len, produced, consumed;
enum { ITER = 100000, WORKERS = 4, ITEMS = 20000 };

static void *incrementer(void *arg) {
  for (int i = 0; i < ITER; i++) {
    pthread_mutex_lock(arg);
    counter++;
    pthread_mutex_unlock(arg);
  }
  return NULL;
}

static void *consumer(void *arg) {
  (void)arg;
  for (;;) {
    pthread_mutex_lock(&mu);
    while (queue_len == 0 && consumed < ITEMS) pthread_cond_wait(&cv, &mu);
    if (consumed >= ITEMS) {
      pthread_mutex_unlock(&mu);
      return NULL;
    }
    queue_len--;
    if (++consumed == ITEMS) pthread_cond_broadcast(&cv);
    pthread_mutex_unlock(&mu);
  }
}

static void run_incrementers(pthread_mutex_t *m) {
  pthread_t th[WORKERS];
  counter = 0;
  for (int i = 0; i < WORKERS; i++) pthread_create(&th[i], NULL, incrementer, m);
  for (int i = 0; i < WORKERS; i++) pthread_join(th[i], NULL);
}

static void test_mutex_condvar(void) {
  const char *t = "mutex_condvar";
  uint64_t t0 = now_ns();
  run_incrementers(&mu);
  uint64_t dt = now_ns() - t0;
  CHECK(t, counter == (long)WORKERS * ITER, "counter %ld", counter);
  printf("lat mutex_contended_4x%d total=%.1fms per_op=%.0fns\n", ITER, dt / 1e6, (double)dt / (WORKERS * ITER));

  pthread_mutexattr_t ma;
  pthread_mutexattr_init(&ma);
  pthread_mutexattr_setprotocol(&ma, PTHREAD_PRIO_INHERIT);
  pthread_mutex_t pi;
  pthread_mutex_init(&pi, &ma);
  t0 = now_ns();
  run_incrementers(&pi);
  dt = now_ns() - t0;
  CHECK(t, counter == (long)WORKERS * ITER, "PI counter %ld", counter);
  printf("lat pi_mutex_contended_4x%d total=%.1fms per_op=%.0fns\n", ITER, dt / 1e6, (double)dt / (WORKERS * ITER));

  pthread_t th[WORKERS];
  for (int i = 0; i < WORKERS; i++) pthread_create(&th[i], NULL, consumer, NULL);
  for (int i = 0; i < ITEMS; i++) {
    pthread_mutex_lock(&mu);
    queue_len++;
    produced++;
    pthread_cond_signal(&cv);
    pthread_mutex_unlock(&mu);
    if (i % 64 == 0) sched_yield();
  }
  for (int i = 0; i < WORKERS; i++) pthread_join(th[i], NULL);
  CHECK(t, consumed == ITEMS && queue_len == 0, "consumed %d queue %d", consumed, queue_len);

  // Timed wait on a condition nobody signals.
  struct timespec dl;
  clock_gettime(CLOCK_REALTIME, &dl);
  dl.tv_nsec += 20 * 1000000;
  if (dl.tv_nsec >= 1000000000) dl.tv_sec++, dl.tv_nsec -= 1000000000;
  pthread_mutex_lock(&mu);
  t0 = now_ns();
  int r = pthread_cond_timedwait(&cv, &mu, &dl);
  dt = now_ns() - t0;
  pthread_mutex_unlock(&mu);
  CHECK(t, r == ETIMEDOUT, "timedwait returned %d", r);
  CHECK(t, dt >= 19000000 && dt < 30000000, "20 ms timed wait took %.2f ms", dt / 1e6);
  printf("lat cond_timedwait_20ms late=%.0fus\n", (dt - 20000000.0) / 1e3);
  printf("ok %s\n", t);
}

// ---- raw futex ---------------------------------------------------------------------

static _Atomic uint32_t fa, fb;
static _Atomic int woke_from;

static void *futex_waiter(void *arg) {
  (void)arg;
  long r = futex(&fa, FUTEX_WAIT_PRIVATE, 0, NULL, NULL, 0);
  if (r == 0) atomic_fetch_add(&woke_from, 1);
  return (void *)r;
}

static _Atomic uint32_t ping, pong;
static void *ponger(void *arg) {
  int n = (int)(intptr_t)arg;
  for (int i = 1; i <= n; i++) {
    while (atomic_load(&ping) != (uint32_t)i) futex(&ping, FUTEX_WAIT_PRIVATE, (uint32_t)i - 1, NULL, NULL, 0);
    atomic_store(&pong, (uint32_t)i);
    futex(&pong, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0);
  }
  return NULL;
}

static void test_futex(void) {
  const char *t = "futex";
  // Stale value, zero timeout, bitset zero.
  uint32_t w = 1;
  CHECK(t, futex(&w, FUTEX_WAIT_PRIVATE, 0, NULL, NULL, 0) == -1 && errno == EAGAIN, "stale value: errno %d", errno);
  struct timespec zero = {0, 0};
  CHECK(t, futex(&w, FUTEX_WAIT_PRIVATE, 1, &zero, NULL, 0) == -1 && errno == ETIMEDOUT, "zero timeout: errno %d", errno);
  CHECK(t, futex(&w, FUTEX_WAIT_BITSET_PRIVATE, 1, NULL, NULL, 0) == -1 && errno == EINVAL, "bitset 0: errno %d", errno);
  CHECK(t, futex(&w, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0) == 0, "wake with no waiters");

  // CMP_REQUEUE: wake 1, move 3, then wake exactly those 3 on the target.
  enum { NW = 4 };
  pthread_t th[NW];
  atomic_store(&fa, 0);
  atomic_store(&woke_from, 0);
  for (int i = 0; i < NW; i++) pthread_create(&th[i], NULL, futex_waiter, NULL);
  usleep(50000);
  CHECK(t, futex(&fa, FUTEX_CMP_REQUEUE_PRIVATE, 1, (void *)(intptr_t)3, &fb, 1) == -1 && errno == EAGAIN,
        "CMP_REQUEUE with a stale value: errno %d", errno);
  long r = futex(&fa, FUTEX_CMP_REQUEUE_PRIVATE, 1, (void *)(intptr_t)3, &fb, 0);
  CHECK(t, r == 4, "CMP_REQUEUE returned %ld, want 4 (1 woken + 3 moved)", r);
  usleep(20000);
  CHECK(t, atomic_load(&woke_from) == 1, "%d woke after requeue", atomic_load(&woke_from));
  CHECK(t, futex(&fa, FUTEX_WAKE_PRIVATE, INT_MAX, NULL, NULL, 0) == 0, "nobody left on the source word");
  r = futex(&fb, FUTEX_WAKE_PRIVATE, INT_MAX, NULL, NULL, 0);
  CHECK(t, r == 3, "wake on the requeue target returned %ld", r);
  for (int i = 0; i < NW; i++) pthread_join(th[i], NULL);
  CHECK(t, atomic_load(&woke_from) == NW, "%d of %d woke", atomic_load(&woke_from), NW);

  // Timeouts: relative (FUTEX_WAIT) and absolute (WAIT_BITSET, both clocks).
  struct timespec rel = {0, 20 * 1000000};
  uint64_t t0 = now_ns();
  r = futex(&w, FUTEX_WAIT_PRIVATE, 1, &rel, NULL, 0);
  uint64_t dt = now_ns() - t0;
  CHECK(t, r == -1 && errno == ETIMEDOUT && dt >= 20000000 && dt < 25000000, "20 ms relative wait: r=%ld errno=%d %.2f ms", r,
        errno, dt / 1e6);
  printf("lat futex_wait_20ms_relative late=%.0fus\n", (dt - 20000000.0) / 1e3);
  struct timespec abs;
  clock_gettime(CLOCK_REALTIME, &abs);
  abs.tv_nsec += 10000000;
  if (abs.tv_nsec >= 1000000000) abs.tv_sec++, abs.tv_nsec -= 1000000000;
  t0 = now_ns();
  r = futex(&w, FUTEX_WAIT_BITSET_PRIVATE | FUTEX_CLOCK_REALTIME, 1, &abs, NULL, FUTEX_BITSET_MATCH_ANY);
  dt = now_ns() - t0;
  CHECK(t, r == -1 && errno == ETIMEDOUT && dt >= 9000000 && dt < 15000000, "10 ms absolute realtime: r=%ld %.2f ms", r,
        dt / 1e6);

  // Latency: uncontended wake syscall, and a two-thread ping-pong.
  enum { N = 20000 };
  static uint64_t lat[N];
  for (int i = 0; i < N; i++) {
    t0 = now_ns();
    futex(&w, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0);
    lat[i] = now_ns() - t0;
  }
  report("futex_wake_no_waiters", lat, N);
  pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
  for (int i = 0; i < N; i++) {
    t0 = now_ns();
    pthread_mutex_lock(&m);
    pthread_mutex_unlock(&m);
    lat[i] = now_ns() - t0;
  }
  report("mutex_lock_unlock_uncontended", lat, N);
  enum { P = 5000 };
  atomic_store(&ping, 0);
  atomic_store(&pong, 0);
  pthread_t p;
  pthread_create(&p, NULL, ponger, (void *)(intptr_t)P);
  for (int i = 1; i <= P; i++) {
    t0 = now_ns();
    atomic_store(&ping, (uint32_t)i);
    futex(&ping, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0);
    while (atomic_load(&pong) != (uint32_t)i) futex(&pong, FUTEX_WAIT_PRIVATE, (uint32_t)i - 1, NULL, NULL, 0);
    lat[i - 1] = now_ns() - t0;
  }
  pthread_join(p, NULL);
  report("futex_pingpong_roundtrip", lat, P);
  printf("ok %s\n", t);
}

// Shared futex on a MAP_SHARED mapping between two threads (the shared
// flavour, as system properties use).
static void *shared_waker(void *arg) {
  usleep(20000);
  atomic_store((_Atomic uint32_t *)arg, 1);
  futex(arg, FUTEX_WAKE, 1, NULL, NULL, 0);
  return NULL;
}

static void test_shared_futex(void) {
  const char *t = "shared_futex";
  _Atomic uint32_t *w = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  CHECK(t, w != MAP_FAILED, "mmap");
  pthread_t p;
  pthread_create(&p, NULL, shared_waker, (void *)w);
  long r = 0;
  while (atomic_load(w) == 0 && r == 0) r = futex((void *)w, FUTEX_WAIT, 0, NULL, NULL, 0);
  pthread_join(p, NULL);
  CHECK(t, atomic_load(w) == 1, "woken with value %u (r=%ld errno=%d)", atomic_load(w), r, errno);
  munmap((void *)w, 16384);
  printf("ok %s\n", t);
}

// ---- signals ---------------------------------------------------------------------

static _Atomic int handled_sig, handled_code, on_altstack;
static _Atomic pid_t handled_tid;
static char *alt_base;
static size_t alt_size = 64 * 1024;

static void usr1_handler(int sig, siginfo_t *si, void *uc) {
  (void)uc;
  char probe;
  atomic_store(&on_altstack, &probe >= alt_base && &probe < alt_base + alt_size);
  atomic_store(&handled_code, si->si_code);
  atomic_store(&handled_tid, gettid());
  atomic_store(&handled_sig, sig);
}

static _Atomic int spinner_ready;
static void *spinner(void *arg) {
  (void)arg;
  stack_t ss = {.ss_sp = alt_base, .ss_size = alt_size, .ss_flags = 0};
  sigaltstack(&ss, NULL);
  atomic_store(&spinner_ready, 1);
  volatile uint64_t n = 0;
  while (!atomic_load(&handled_sig)) n++;  // pure guest code: no syscalls
  return NULL;
}

static void test_pthread_kill_onstack(void) {
  const char *t = "pthread_kill_sa_onstack";
  alt_base = mmap(NULL, alt_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  struct sigaction sa = {0};
  sa.sa_sigaction = usr1_handler;
  sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
  CHECK(t, sigaction(SIGUSR1, &sa, NULL) == 0, "sigaction");
  atomic_store(&handled_sig, 0);
  pthread_t p;
  pthread_create(&p, NULL, spinner, NULL);
  while (!atomic_load(&spinner_ready)) sched_yield();
  usleep(10000);
  pid_t target_tid = pthread_gettid_np(p);
  CHECK(t, pthread_kill(p, SIGUSR1) == 0, "pthread_kill");
  pthread_join(p, NULL);
  CHECK(t, atomic_load(&handled_sig) == SIGUSR1, "signal %d", atomic_load(&handled_sig));
  CHECK(t, atomic_load(&handled_code) == SI_TKILL, "si_code %d", atomic_load(&handled_code));
  CHECK(t, atomic_load(&handled_tid) == target_tid, "ran on tid %d, want %d", atomic_load(&handled_tid), target_tid);
  CHECK(t, atomic_load(&on_altstack), "handler did not run on the alternate stack");
  printf("ok %s\n", t);
}

static sigjmp_buf jb;
static volatile void *fault_addr;
static volatile int fault_code;
static void segv_handler(int sig, siginfo_t *si, void *uc) {
  (void)sig;
  (void)uc;
  fault_addr = si->si_addr;
  fault_code = si->si_code;
  siglongjmp(jb, 1);
}

static void test_null_deref_recovery(void) {
  const char *t = "null_deref_siglongjmp";
  struct sigaction sa = {0}, old;
  sa.sa_sigaction = segv_handler;
  sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
  sigaction(SIGSEGV, &sa, &old);
  volatile int *volatile np = (int *)8;
  if (sigsetjmp(jb, 1) == 0) {
    (void)*np;
    CHECK(t, 0, "no fault");
  }
  CHECK(t, fault_addr == (void *)8 && fault_code == SEGV_MAPERR, "si_addr %p si_code %d", fault_addr, fault_code);
  // A write to a read-only page: SEGV_ACCERR with the exact address.
  char *ro = mmap(NULL, 16384, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (sigsetjmp(jb, 1) == 0) {
    ((volatile char *)ro)[100] = 1;
    CHECK(t, 0, "no fault on a read-only page");
  }
  CHECK(t, fault_addr == ro + 100 && fault_code == SEGV_ACCERR, "si_addr %p (want %p) si_code %d", fault_addr, ro + 100,
        fault_code);
  // The signal mask was restored by siglongjmp: the next fault is caught too.
  enum { N = 5000 };
  static uint64_t lat[N];
  for (int i = 0; i < N; i++) {
    uint64_t t0 = now_ns();
    if (sigsetjmp(jb, 1) == 0) (void)*np;
    lat[i] = now_ns() - t0;
  }
  report("sigsegv_siglongjmp_roundtrip", lat, N);
  sigaction(SIGSEGV, &old, NULL);
  munmap(ro, 16384);
  printf("ok %s\n", t);
}

// Fault -> handler that fixes the pc in the ucontext -> rt_sigreturn resumes
// (ART's implicit null check path). The faulting load is 4 bytes; skip it.
static void skip_handler(int sig, siginfo_t *si, void *ucv) {
  (void)sig;
  (void)si;
  ucontext_t *uc = ucv;
  uc->uc_mcontext.pc += 4;
  uc->uc_mcontext.regs[0] = 0x600d;
}

static void test_sigreturn_resume(void) {
  const char *t = "fault_handler_sigreturn";
  struct sigaction sa = {0}, old;
  sa.sa_sigaction = skip_handler;
  sa.sa_flags = SA_SIGINFO;
  sigaction(SIGSEGV, &sa, &old);
  // x0 carries the pointer in and the result out of `ldr x0, [x0]`.
  uint64_t r;
  __asm__ volatile(
      "mov x0, #16\n"
      "ldr x0, [x0]\n"
      "mov %0, x0\n"
      : "=r"(r)
      :
      : "x0", "memory");
  CHECK(t, r == 0x600d, "resumed with x0=%#llx", (unsigned long long)r);
  enum { N = 5000 };
  static uint64_t lat[N];
  for (int i = 0; i < N; i++) {
    uint64_t t0 = now_ns();
    __asm__ volatile("mov x0, #16\nldr x0, [x0]\n" ::: "x0", "memory");
    lat[i] = now_ns() - t0;
  }
  report("sigsegv_handler_sigreturn_roundtrip", lat, N);
  sigaction(SIGSEGV, &old, NULL);
  printf("ok %s\n", t);
}

static _Atomic int usr2_count;
static void usr2_handler(int sig) {
  (void)sig;
  atomic_fetch_add(&usr2_count, 1);
}

static void test_raise_latency(void) {
  const char *t = "raise";
  signal(SIGUSR2, usr2_handler);
  enum { N = 10000 };
  static uint64_t lat[N];
  for (int i = 0; i < N; i++) {
    uint64_t t0 = now_ns();
    raise(SIGUSR2);
    lat[i] = now_ns() - t0;
  }
  CHECK(t, atomic_load(&usr2_count) == N, "%d handler runs", atomic_load(&usr2_count));
  report("raise_to_handler_return", lat, N);
  printf("ok %s\n", t);
}

// Cross-thread asynchronous delivery latency: tgkill -> handler on a thread
// spinning in guest code.
static _Atomic uint64_t delivered_at;
static void stamp_handler(int sig) {
  (void)sig;
  atomic_store(&delivered_at, now_ns());
}
static _Atomic int stamp_stop;
static void *stamp_spinner(void *arg) {
  (void)arg;
  while (!atomic_load(&stamp_stop)) {
  }
  return NULL;
}

static void test_async_latency(void) {
  const char *t = "async_signal";
  signal(SIGRTMIN + 3, stamp_handler);  // a real-time signal Darwin does not have
  pthread_t p;
  pthread_create(&p, NULL, stamp_spinner, NULL);
  usleep(10000);
  enum { N = 2000 };
  static uint64_t lat[N];
  for (int i = 0; i < N; i++) {
    atomic_store(&delivered_at, 0);
    uint64_t t0 = now_ns();
    pthread_kill(p, SIGRTMIN + 3);
    while (atomic_load(&delivered_at) == 0) {
    }
    lat[i] = atomic_load(&delivered_at) - t0;
  }
  atomic_store(&stamp_stop, 1);
  pthread_join(p, NULL);
  report("tgkill_to_handler_on_spinning_thread", lat, N);
  printf("ok %s\n", t);
}

// Signals landing anywhere in the syscall path (guest code, stub, lean and
// full paths) must leave every register as it was: the handler clobbers
// caller-saved and vector registers, the frame restores them.
static _Atomic uint64_t storm_hits;
static void clobber_handler(int sig) {
  (void)sig;
  atomic_fetch_add(&storm_hits, 1);
  __asm__ volatile(
      "movi v0.16b, #0x55\n movi v8.16b, #0x55\n movi v16.16b, #0x55\n movi v31.16b, #0x55\n"
      "mov x9, #0x5555\n mov x15, #0x5555\n cmp x9, x9\n" ::
          : "x9", "x15", "v0", "v8", "v16", "v31", "cc");
}

// One syscall with known values in the registers the kernel preserves.
// Returns a bitmask of registers that changed.
static uint64_t storm_iter(uint64_t nr) {
  uint64_t bad;
  __asm__ volatile(
      "mov x19, #0x191\n mov x20, #0x202\n mov x28, #0x282\n"
      "mov x9, #0x090\n mov x15, #0x151\n mov x1, #0\n mov x2, #0\n"
      "fmov d8, x19\n fmov d16, x20\n fmov d31, x28\n dup v0.2d, x9\n"
      "mov x0, #0\n mov x8, %1\n cmp x8, x8\n"  // Z and C set
      "svc #0\n"
      "cset x10, eq\n cset x11, hs\n and x10, x10, x11\n eor %0, x10, #1\n"
      "cmp x19, #0x191\n cset x10, ne\n orr %0, %0, x10, lsl #1\n"
      "cmp x20, #0x202\n cset x10, ne\n orr %0, %0, x10, lsl #2\n"
      "cmp x28, #0x282\n cset x10, ne\n orr %0, %0, x10, lsl #3\n"
      "cmp x9, #0x090\n cset x10, ne\n orr %0, %0, x10, lsl #4\n"
      "cmp x15, #0x151\n cset x10, ne\n orr %0, %0, x10, lsl #5\n"
      "fmov x11, d8\n cmp x11, #0x191\n cset x10, ne\n orr %0, %0, x10, lsl #6\n"
      "fmov x11, d16\n cmp x11, #0x202\n cset x10, ne\n orr %0, %0, x10, lsl #7\n"
      "fmov x11, d31\n cmp x11, #0x282\n cset x10, ne\n orr %0, %0, x10, lsl #8\n"
      "mov x11, v0.d[1]\n cmp x11, #0x090\n cset x10, ne\n orr %0, %0, x10, lsl #9\n"
      : "=&r"(bad)
      : "r"(nr)
      : "x0", "x1", "x2", "x8", "x9", "x10", "x11", "x15", "x19", "x20", "x28", "v0", "v8", "v16", "v31", "cc",
        "memory");
  return bad;
}

static _Atomic int storm_done;
static void *stormer(void *arg) {
  uint64_t *bad = arg;
  // getpid (lean path), sched_getscheduler(0) (full path).
  for (int i = 0; i < 200000; i++) *bad |= storm_iter(i & 1 ? SYS_getpid : SYS_sched_getscheduler);
  atomic_store(&storm_done, 1);
  return NULL;
}

static void test_signal_storm(void) {
  const char *t = "signal_storm";
  signal(SIGUSR2, clobber_handler);
  signal(SIGRTMIN + 4, clobber_handler);
  uint64_t bad = 0;
  pthread_t p;
  pthread_create(&p, NULL, stormer, &bad);
  long sent = 0;
  while (!atomic_load(&storm_done)) {
    pthread_kill(p, sent++ & 1 ? SIGUSR2 : SIGRTMIN + 4);
    for (volatile int k = 0; k < 500; k++) {
    }
  }
  pthread_join(p, NULL);
  printf("storm: %ld signals sent, %llu handled\n", sent, (unsigned long long)atomic_load(&storm_hits));
  CHECK(t, bad == 0, "registers changed across syscalls (bitmask %#llx)", (unsigned long long)bad);
  CHECK(t, atomic_load(&storm_hits) > 100, "only %llu handler runs", (unsigned long long)atomic_load(&storm_hits));
  printf("ok %s\n", t);
}

// Cost of a redirected syscall: lean path (getpid) and full path
// (prctl(PR_GET_DUMPABLE)), both carrying the signal exit check.
static void test_syscall_cost(void) {
  const char *t = "syscall_cost";
  enum { N = 1000000 };
  long nrs[2] = {SYS_getpid, SYS_prctl};
  const char *names[2] = {"svc_getpid_lean", "svc_prctl_get_dumpable_full"};
  for (int k = 0; k < 2; k++) {
    uint64_t t0 = now_ns();
    for (int i = 0; i < N; i++) {
      register long x8 __asm__("x8") = nrs[k];
      register long x0 __asm__("x0") = PR_GET_DUMPABLE;
      __asm__ volatile("svc #0" : "+r"(x0) : "r"(x8) : "memory");
    }
    printf("lat %s %.1fns per call\n", names[k], (double)(now_ns() - t0) / N);
  }
  printf("ok %s\n", t);
}

// ART's SignalCatcher shape: SIGQUIT blocked everywhere, one thread waits.
static void *quit_sender(void *arg) {
  (void)arg;
  usleep(20000);
  kill(getpid(), SIGQUIT);
  return NULL;
}

static void test_sigwait(void) {
  const char *t = "sigwait_sigqueue";
  sigset_t set;
  sigemptyset(&set);
  sigaddset(&set, SIGQUIT);
  sigaddset(&set, SIGRTMIN + 5);
  CHECK(t, pthread_sigmask(SIG_BLOCK, &set, NULL) == 0, "block");
  pthread_t p;
  pthread_create(&p, NULL, quit_sender, NULL);
  int sig = 0;
  CHECK(t, sigwait(&set, &sig) == 0 && sig == SIGQUIT, "sigwait got %d", sig);
  pthread_join(p, NULL);
  // Real-time signals queue, with their values, in order.
  for (int i = 0; i < 3; i++) {
    union sigval v = {.sival_int = 100 + i};
    CHECK(t, sigqueue(getpid(), SIGRTMIN + 5, v) == 0, "sigqueue %d: errno %d", i, errno);
  }
  sigset_t pend;
  sigpending(&pend);
  CHECK(t, sigismember(&pend, SIGRTMIN + 5), "sigpending");
  for (int i = 0; i < 3; i++) {
    siginfo_t si;
    struct timespec ts = {1, 0};
    CHECK(t, sigtimedwait(&set, &si, &ts) == SIGRTMIN + 5, "sigtimedwait %d: errno %d", i, errno);
    CHECK(t, si.si_value.sival_int == 100 + i && si.si_code == SI_QUEUE, "value %d code %d", si.si_value.sival_int,
          si.si_code);
  }
  struct timespec short_ts = {0, 5000000};
  CHECK(t, sigtimedwait(&set, NULL, &short_ts) == -1 && errno == EAGAIN, "empty queue: errno %d", errno);
  pthread_sigmask(SIG_UNBLOCK, &set, NULL);
  printf("ok %s\n", t);
}

// Thread-directed blocked signals stay with their target, including before wait.
struct directed_wait {
  _Atomic int ready, release;
  pid_t tid;
  int signal, error;
  siginfo_t info;
};
static void *directed_quit_waiter(void *argument) {
  struct directed_wait *wait = argument;
  sigset_t set;
  sigemptyset(&set);
  sigaddset(&set, SIGQUIT);
  sigaddset(&set, SIGUSR1);
  wait->tid = (pid_t)syscall(SYS_gettid);
  atomic_store(&wait->ready, 1);
  while (!atomic_load(&wait->release)) sched_yield();
  struct timespec timeout = {2, 0};
  wait->signal = sigtimedwait(&set, &wait->info, &timeout);
  wait->error = errno;
  return argument;
}
static void test_directed_sigquit_join(void) {
  const char *t = "directed_sigquit_join";
  sigset_t set, old;
  sigemptyset(&set);
  sigaddset(&set, SIGQUIT);
  sigaddset(&set, SIGUSR1);
  CHECK(t, pthread_sigmask(SIG_BLOCK, &set, &old) == 0, "block");
  for (int queued = 0; queued < 2; queued++) {
    struct directed_wait target = {0}, competitor = {0};
    pthread_t a, b;
    int create_a = pthread_create(&a, NULL, directed_quit_waiter, &target);
    int create_b = pthread_create(&b, NULL, directed_quit_waiter, &competitor);
    if (create_a || create_b) {
      atomic_store(&target.release, 1);
      atomic_store(&competitor.release, 1);
      if (!create_a) pthread_join(a, NULL);
      if (!create_b) pthread_join(b, NULL);
      pthread_sigmask(SIG_SETMASK, &old, NULL);
      CHECK(t, 0, "create %d/%d", create_a, create_b);
    }
    while (!atomic_load(&target.ready) || !atomic_load(&competitor.ready)) sched_yield();
    atomic_store(&competitor.release, 1);
    if (!queued) {
      atomic_store(&target.release, 1);
      usleep(20000);
    }
    uint64_t start = now_ns();
    int send = pthread_kill(a, SIGQUIT);
    // A signal pending on the target must not become pending on the caller.
    struct timespec poll = {0, 0};
    int caller = sigtimedwait(&set, NULL, &poll), caller_error = errno;
    atomic_store(&target.release, 1);
    void *result = NULL;
    int joined = pthread_join(a, &result);
    uint64_t elapsed = now_ns() - start;
    int stop_competitor = pthread_kill(b, SIGUSR1);
    int joined_competitor = pthread_join(b, NULL);
    int valid = send == 0 && joined == 0 && result == &target &&
        target.signal == SIGQUIT && target.info.si_code == SI_TKILL &&
        target.info.si_pid == getpid() && target.info.si_uid == getuid() &&
        target.tid != competitor.tid && caller == -1 && caller_error == EAGAIN &&
        stop_competitor == 0 && joined_competitor == 0 && competitor.signal == SIGUSR1 &&
        competitor.info.si_code == SI_TKILL && competitor.info.si_pid == getpid() &&
        elapsed < 1000000000ULL;
    if (!valid) {
      pthread_sigmask(SIG_SETMASK, &old, NULL);
      CHECK(t, 0, "phase %d send %d join %d target %d/%d code %d pid %d caller %d/%d competitor %d/%d elapsed %llu",
          queued, send, joined, target.signal, target.error, target.info.si_code,
          target.info.si_pid, caller, caller_error, competitor.signal, competitor.error,
          (unsigned long long)elapsed);
    }
    printf("ok %s %s tid=%d sender=%d join_ns=%llu\n", t,
        queued ? "queued-before-wait" : "after-wait-ready", target.tid,
        target.info.si_pid, (unsigned long long)elapsed);
  }
  pthread_sigmask(SIG_SETMASK, &old, NULL);
}

static _Atomic int sus_hits;
static void sus_handler(int sig) {
  (void)sig;
  atomic_fetch_add(&sus_hits, 1);
}
static void *sus_sender(void *arg) {
  usleep(20000);
  pthread_kill(*(pthread_t *)arg, SIGUSR2);
  return NULL;
}

static void test_sigsuspend(void) {
  const char *t = "sigsuspend";
  signal(SIGUSR2, sus_handler);
  sigset_t block, old, wait_mask;
  sigemptyset(&block);
  sigaddset(&block, SIGUSR2);
  pthread_sigmask(SIG_BLOCK, &block, &old);
  pthread_t self = pthread_self(), p;
  pthread_create(&p, NULL, sus_sender, &self);
  sigemptyset(&wait_mask);
  int r = sigsuspend(&wait_mask);
  CHECK(t, r == -1 && errno == EINTR && atomic_load(&sus_hits) == 1, "r=%d errno=%d hits=%d", r, errno,
        atomic_load(&sus_hits));
  sigset_t now;
  pthread_sigmask(SIG_SETMASK, NULL, &now);
  CHECK(t, sigismember(&now, SIGUSR2), "mask not restored after sigsuspend");
  pthread_join(p, NULL);
  pthread_sigmask(SIG_SETMASK, &old, NULL);
  printf("ok %s\n", t);
}

// SA_RESTART: a read blocked on a pipe survives a handled signal; without
// it, the read fails with EINTR.
static int pipe_fds[2];
static _Atomic int restart_hits;
static void restart_handler(int sig) {
  (void)sig;
  atomic_fetch_add(&restart_hits, 1);
}
static void *reader(void *arg) {
  (void)arg;
  char c;
  ssize_t n = read(pipe_fds[0], &c, 1);
  return (void *)(intptr_t)(n == 1 ? c : -errno);
}

static void test_sa_restart(void) {
  const char *t = "sa_restart";
  for (int restart = 1; restart >= 0; restart--) {
    struct sigaction sa = {0};
    sa.sa_handler = restart_handler;
    sa.sa_flags = restart ? SA_RESTART : 0;
    sigaction(SIGUSR1, &sa, NULL);
    if (pipe(pipe_fds) != 0 && errno == ENOSYS) {
      printf("skip %s: no pipe2 in this layer\n", t);
      return;
    }
    pthread_t p;
    pthread_create(&p, NULL, reader, NULL);
    usleep(20000);
    pthread_kill(p, SIGUSR1);
    usleep(20000);
    CHECK(t, write(pipe_fds[1], "x", 1) == 1, "write");
    void *r;
    pthread_join(p, &r);
    if (restart)
      CHECK(t, (intptr_t)r == 'x', "SA_RESTART read returned %ld", (long)(intptr_t)r);
    else
      CHECK(t, (intptr_t)r == -EINTR, "read without SA_RESTART returned %ld", (long)(intptr_t)r);
    close(pipe_fds[0]);
    close(pipe_fds[1]);
  }
  CHECK(t, atomic_load(&restart_hits) == 2, "%d handler runs", atomic_load(&restart_hits));
  printf("ok %s\n", t);
}

static void test_nanosleep(void) {
  const char *t = "nanosleep";
  enum { N = 20 };
  static uint64_t late[N];
  for (int i = 0; i < N; i++) {
    struct timespec ts = {0, 5000000};
    uint64_t t0 = now_ns();
    CHECK(t, nanosleep(&ts, NULL) == 0, "nanosleep");
    uint64_t dt = now_ns() - t0;
    CHECK(t, dt >= 5000000, "woke early after %.3f ms", dt / 1e6);
    late[i] = dt - 5000000;
  }
  report("nanosleep_5ms_lateness", late, N);
  printf("ok %s\n", t);
}

static void test_identity(void) {
  const char *t = "identity";
  CHECK(t, gettid() == getpid(), "main thread tid %d != pid %d", gettid(), getpid());
  char name[16] = {0};
  prctl(PR_SET_NAME, "main-thread-name-too-long");
  prctl(PR_GET_NAME, name);
  CHECK(t, strcmp(name, "main-thread-nam") == 0, "PR_GET_NAME: %s", name);
  cpu_set_t cs;
  CHECK(t, sched_getaffinity(0, sizeof cs, &cs) == 0 && CPU_COUNT(&cs) > 0, "affinity");
  CHECK(t, setpriority(PRIO_PROCESS, 0, 5) == 0 && getpriority(PRIO_PROCESS, 0) == 5, "priority");
  CHECK(t, sched_get_priority_max(SCHED_FIFO) == 99, "priority max");
  printf("ok %s\n", t);
}

// A robust mutex word held by a thread that exits: the kernel (here, the
// layer) marks it FUTEX_OWNER_DIED and wakes a waiter.
struct robust_head {
  void *list;
  long futex_offset;
  void *list_op_pending;
};
struct robust_entry {
  void *next;
  _Atomic uint32_t futex;
};
static struct robust_entry robust_lock;
static _Atomic int robust_ready;
static void *robust_owner(void *arg) {
  (void)arg;
  static __thread struct robust_head head;
  head.list = &robust_lock;
  head.futex_offset = offsetof(struct robust_entry, futex);
  head.list_op_pending = NULL;
  robust_lock.next = &head;
  atomic_store(&robust_lock.futex, (uint32_t)gettid());
  if (syscall(SYS_set_robust_list, &head, sizeof head) != 0) return (void *)1;
  atomic_store(&robust_ready, 1);
  usleep(20000);
  return NULL;  // exits holding the lock
}

static void test_robust_list(void) {
  const char *t = "robust_list";
  pthread_t p;
  pthread_create(&p, NULL, robust_owner, NULL);
  while (!atomic_load(&robust_ready)) sched_yield();
  uint32_t v = atomic_load(&robust_lock.futex);
  // Announce a waiter, then sleep until the owner dies.
  atomic_store(&robust_lock.futex, v | FUTEX_WAITERS);
  struct timespec ts = {2, 0};
  while (!(atomic_load(&robust_lock.futex) & FUTEX_OWNER_DIED)) {
    long r = futex(&robust_lock.futex, FUTEX_WAIT, v | FUTEX_WAITERS, &ts, NULL, 0);
    CHECK(t, r == 0 || errno == EAGAIN || errno == EINTR, "wait: errno %d", errno);
  }
  pthread_join(p, NULL);
  uint32_t now = atomic_load(&robust_lock.futex);
  CHECK(t, now == (FUTEX_OWNER_DIED | FUTEX_WAITERS), "futex word %#x", now);
  printf("ok %s\n", t);
}

// ART's stack-overflow shape: a thread runs into its guard page and the
// SIGSEGV handler, on the thread's alternate stack, recovers.
static sigjmp_buf overflow_jb;
static _Atomic int overflow_on_alt;
static char *overflow_alt;
static void overflow_handler(int sig, siginfo_t *si, void *uc) {
  (void)sig;
  (void)si;
  (void)uc;
  char probe;
  atomic_store(&overflow_on_alt, &probe >= overflow_alt && &probe < overflow_alt + 65536);
  siglongjmp(overflow_jb, 1);
}
static int recurse(volatile char *p, int depth) {
  volatile char buf[1024];
  buf[0] = (char)depth;
  if (depth < 0) return 0;  // never: the guard page ends the recursion
  return p[0] + recurse(buf, depth + 1) + buf[1023];
}
static void *overflower(void *arg) {
  (void)arg;
  overflow_alt = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  stack_t ss = {.ss_sp = overflow_alt, .ss_size = 65536};
  sigaltstack(&ss, NULL);
  if (sigsetjmp(overflow_jb, 1) == 0) {
    char c = 0;
    recurse(&c, 0);
    return (void *)1;
  }
  return NULL;
}

static void test_stack_overflow(void) {
  const char *t = "stack_overflow";
  struct sigaction sa = {0}, old;
  sa.sa_sigaction = overflow_handler;
  sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
  sigaction(SIGSEGV, &sa, &old);
  pthread_attr_t a;
  pthread_attr_init(&a);
  pthread_attr_setstacksize(&a, 256 * 1024);
  pthread_t p;
  pthread_create(&p, &a, overflower, NULL);
  void *r;
  pthread_join(p, &r);
  sigaction(SIGSEGV, &old, NULL);
  CHECK(t, r == NULL, "recursion did not fault");
  CHECK(t, atomic_load(&overflow_on_alt), "handler not on the alternate stack");
  printf("ok %s\n", t);
}

// The main thread leaves with pthread_exit; the process lives on until the
// last thread ends, and exits 0.
static void *late_worker(void *arg) {
  (void)arg;
  usleep(50000);
  printf("worker done\n");
  return NULL;
}
static void main_exit(void) {
  pthread_t p;
  pthread_create(&p, NULL, late_worker, NULL);
  pthread_exit(NULL);
}

// ART's SignalCatcher: SIGQUIT blocked, one thread waits for it, and it
// comes from another process (the harness).
static void wait_external_sigquit(void) {
  sigset_t set;
  sigemptyset(&set);
  sigaddset(&set, SIGQUIT);
  pthread_sigmask(SIG_BLOCK, &set, NULL);
  printf("ready\n");
  int sig = 0;
  sigwait(&set, &sig);
  printf("got %d\n", sig);
  exit(0);
}

// abort() dies of SIGABRT; run as its own process by the harness. The
// debuggerd handler the linker installs is reset first: it needs
// crash_dump, which this root does not have.
static void test_abort(void) {
  signal(SIGABRT, SIG_DFL);
  abort();
}

// An unhandled null dereference kills the process with SIGSEGV (the
// debuggerd handler is reset, as for abort).
static void segv_default(void) {
  signal(SIGSEGV, SIG_DFL);
  volatile int *volatile np = (int *)8;
  (void)*np;
}

struct test {
  const char *name;
  void (*fn)(void);
};
static const struct test tests[] = {
    {"identity", test_identity},
    {"syscall_cost", test_syscall_cost},
    {"create_join_detach", test_create_join_detach},
    {"mutex_condvar", test_mutex_condvar},
    {"futex", test_futex},
    {"shared_futex", test_shared_futex},
    {"pthread_kill_sa_onstack", test_pthread_kill_onstack},
    {"null_deref_siglongjmp", test_null_deref_recovery},
    {"fault_handler_sigreturn", test_sigreturn_resume},
    {"raise", test_raise_latency},
    {"async_signal", test_async_latency},
    {"signal_storm", test_signal_storm},
    {"sigwait_sigqueue", test_sigwait},
    {"directed_sigquit_join", test_directed_sigquit_join},
    {"sigsuspend", test_sigsuspend},
    {"sa_restart", test_sa_restart},
    {"nanosleep", test_nanosleep},
    {"robust_list", test_robust_list},
    {"stack_overflow", test_stack_overflow},
};

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IONBF, 0);
  if (argc == 2 && strcmp(argv[1], "abort") == 0) test_abort();
  if (argc == 2 && strcmp(argv[1], "main_exit") == 0) main_exit();
  if (argc == 2 && strcmp(argv[1], "segv_default") == 0) segv_default();
  if (argc == 2 && strcmp(argv[1], "external_sigquit") == 0) wait_external_sigquit();
  for (size_t i = 0; i < sizeof tests / sizeof tests[0]; i++) {
    int run = argc == 1;
    for (int a = 1; a < argc; a++) run |= strcmp(argv[a], tests[i].name) == 0;
    if (run) tests[i].fn();
  }
  printf("%s (%d failures)\n", failures ? "FAILED" : "ALL PASSED", failures);
  return failures != 0;
}
