// P0 experiment 1: does a user-written TPIDR_EL0 survive on Darwin?
//
// Each worker thread writes a unique 64-bit value to TPIDR_EL0 (as bionic
// does with `msr tpidr_el0, x0`) and then, for the whole run, re-reads it
// after syscalls, yields, sleeps, libSystem calls, and while being preempted
// and interrupted by signals. Any mismatch is recorded with the observed
// value so we can tell what overwrote it.
//
// Build: clang -O2 -o tpidr tpidr.c
// Run:   ./tpidr [seconds] [threads]
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/sysctl.h>
#include <time.h>
#include <unistd.h>
#include <dispatch/dispatch.h>
#include <os/lock.h>

static inline uint64_t rd_tpidr(void) {
  uint64_t v;
  __asm__ volatile("mrs %0, tpidr_el0" : "=r"(v));
  return v;
}
static inline void wr_tpidr(uint64_t v) { __asm__ volatile("msr tpidr_el0, %0" ::"r"(v)); }
static inline uint64_t rd_tpidrro(void) {
  uint64_t v;
  __asm__ volatile("mrs %0, tpidrro_el0" : "=r"(v));
  return v;
}

#define MAXT 256
struct worker {
  pthread_t th;
  int idx;
  uint64_t value;          // value we wrote
  uint64_t initial;        // TPIDR_EL0 seen before our first write
  _Atomic uint64_t checks;
  _Atomic uint64_t mismatches;
  _Atomic uint64_t signals;
  _Atomic uint64_t sig_mismatch;
  uint64_t first_bad;      // first wrong value observed
  const char *first_where; // after which operation
  uint64_t cpu_before, cpu_after;
} W[MAXT];

static _Atomic int stop;
static __thread struct worker *self_w;
static int devnull;
static os_unfair_lock ulock = OS_UNFAIR_LOCK_INIT;
static pthread_mutex_t pmutex = PTHREAD_MUTEX_INITIALIZER;

// Per-operation tally of clobbers, keyed by the (literal) operation name.
static struct { const char *where; _Atomic uint64_t n; _Atomic uint64_t clean; } OPS[32];
static void tally(const char *where, int bad) {
  for (int i = 0; i < 32; i++) {
    const char *cur = atomic_load((_Atomic(const char *) *)&OPS[i].where);
    if (cur == NULL) {
      const char *expect = NULL;
      if (!atomic_compare_exchange_strong((_Atomic(const char *) *)&OPS[i].where, &expect, where) && expect != where)
        continue;
      cur = where;
    }
    if (cur == where) {
      if (bad) atomic_fetch_add(&OPS[i].n, 1); else atomic_fetch_add(&OPS[i].clean, 1);
      return;
    }
  }
}

static void note_bad(struct worker *w, uint64_t got, const char *where) {
  if (atomic_fetch_add(&w->mismatches, 1) == 0) {
    w->first_bad = got;
    w->first_where = where;
  }
  wr_tpidr(w->value);  // restore and keep going so we count every clobber
}

#define CHECK(w, where)                         \
  do {                                          \
    uint64_t g_ = rd_tpidr();                   \
    atomic_fetch_add(&(w)->checks, 1);          \
    tally((where), g_ != (w)->value);             \
    if (g_ != (w)->value) note_bad((w), g_, (where)); \
  } while (0)

static void on_usr1(int sig, siginfo_t *si, void *uc) {
  (void)sig; (void)si; (void)uc;
  struct worker *w = self_w;
  if (!w) return;
  atomic_fetch_add(&w->signals, 1);
  if (rd_tpidr() != w->value) atomic_fetch_add(&w->sig_mismatch, 1);
}

static void *worker_main(void *arg) {
  struct worker *w = arg;
  self_w = w;
  w->initial = rd_tpidr();
  size_t cpu = 0;
  pthread_cpu_number_np(&cpu);
  w->cpu_before = cpu;
  wr_tpidr(w->value);
  CHECK(w, "write");
  uint64_t i = 0;
  while (!atomic_load_explicit(&stop, memory_order_relaxed)) {
    switch (i++ % 12) {
      case 0: (void)getpid(); CHECK(w, "getpid"); break;
      case 1: sched_yield(); CHECK(w, "sched_yield"); break;
      case 2: (void)write(devnull, "x", 1); CHECK(w, "write(2)"); break;
      case 3: { void *p = malloc(64 + (i & 1023)); free(p); CHECK(w, "malloc/free"); break; }
      case 4: usleep(50); CHECK(w, "usleep"); break;
      case 5: os_unfair_lock_lock(&ulock); os_unfair_lock_unlock(&ulock); CHECK(w, "os_unfair_lock"); break;
      case 6: pthread_mutex_lock(&pmutex); pthread_mutex_unlock(&pmutex); CHECK(w, "pthread_mutex"); break;
      case 7: { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); CHECK(w, "clock_gettime"); break; }
      case 8: { char b[64]; snprintf(b, sizeof b, "%llu", (unsigned long long)i); CHECK(w, "snprintf"); break; }
      case 9: { size_t c; pthread_cpu_number_np(&c); CHECK(w, "pthread_cpu_number_np"); break; }
      default:
        // Pure spinning so the scheduler has to preempt us (threads > cores).
        for (volatile int k = 0; k < 20000; k++) {}
        CHECK(w, "spin/preempt");
    }
  }
  cpu = 0;
  pthread_cpu_number_np(&cpu);
  w->cpu_after = cpu;
  CHECK(w, "end");
  return NULL;
}

// A thread that never writes TPIDR_EL0: record what the kernel keeps there.
static void observe_kernel_value(void) {
  printf("-- kernel-owned TPIDR_EL0 in a thread that never writes it --\n");
  for (int i = 0; i < 8; i++) {
    uint64_t v = rd_tpidr();
    size_t cpu = 0;
    pthread_cpu_number_np(&cpu);
    printf("  sample %d: TPIDR_EL0=0x%016" PRIx64 " (low12=%" PRIu64 ") pthread_cpu_number_np=%zu TPIDRRO_EL0=0x%016" PRIx64 "\n",
           i, v, v & 0xfff, cpu, rd_tpidrro());
    usleep(20000);
  }
}

static void *probe_libsystem(void *arg) {
  (void)arg;
  // Write a value, exercise a broad slice of libSystem, check nothing wrote it.
  const uint64_t v = 0x0000123456789ab0ull;
  wr_tpidr(v);
  const char *bad = NULL;
#define P(name, ...) do { __VA_ARGS__; if (!bad && rd_tpidr() != v) bad = name; } while (0)
  P("dispatch_sync", { dispatch_queue_t q = dispatch_queue_create("p0", NULL); dispatch_sync(q, ^{ }); dispatch_release(q); });
  P("dispatch_async+semaphore", { dispatch_semaphore_t s = dispatch_semaphore_create(0); dispatch_async(dispatch_get_global_queue(0, 0), ^{ dispatch_semaphore_signal(s); }); dispatch_semaphore_wait(s, DISPATCH_TIME_FOREVER); dispatch_release(s); });
  P("pthread_create/join", { pthread_t t; pthread_create(&t, NULL, (void *(*)(void *))rd_tpidr, NULL); pthread_join(t, NULL); });
  P("stdio", { FILE *f = fopen("/dev/null", "w"); fprintf(f, "%f\n", 3.14); fclose(f); });
  P("malloc sizes", { for (int i = 0; i < 10000; i++) free(malloc((size_t)i * 16)); });
  P("getrusage", { struct rusage ru; getrusage(RUSAGE_SELF, &ru); });
  P("sysctl", { int mib[2] = {CTL_HW, HW_NCPU}; int n; size_t l = sizeof n; sysctl(mib, 2, &n, &l, NULL, 0); });
  P("strerror", { char *e = strerror(EINVAL); (void)e; });
  uint64_t got = rd_tpidr();
  printf("-- libSystem probe on a secondary thread: %s (value now 0x%016" PRIx64 ")\n",
         bad ? bad : "no libSystem call changed TPIDR_EL0", got);
  size_t cpu = 0;
  pthread_cpu_number_np(&cpu);
  printf("   pthread_cpu_number_np with guest value installed = %zu (== value & 0xfff = %" PRIu64 ")\n",
         cpu, v & 0xfff);
  return NULL;
}

int main(int argc, char **argv) {
  int secs = argc > 1 ? atoi(argv[1]) : 10;
  int nthreads = argc > 2 ? atoi(argv[2]) : 64;
  if (nthreads > MAXT) nthreads = MAXT;
  devnull = open("/dev/null", O_WRONLY);
  int ncpu = 0;
  size_t l = sizeof ncpu;
  sysctlbyname("hw.ncpu", &ncpu, &l, NULL, 0);
  printf("hw.ncpu=%d threads=%d seconds=%d\n", ncpu, nthreads, secs);
  printf("main thread initial TPIDR_EL0=0x%016" PRIx64 "\n", rd_tpidr());
  observe_kernel_value();
  {
    pthread_t t;
    pthread_create(&t, NULL, probe_libsystem, NULL);
    pthread_join(t, NULL);
  }

  struct sigaction sa = {0};
  sa.sa_sigaction = on_usr1;
  sa.sa_flags = SA_SIGINFO | SA_RESTART;
  sigaction(SIGUSR1, &sa, NULL);

  struct rusage r0;
  getrusage(RUSAGE_SELF, &r0);
  for (int i = 0; i < nthreads; i++) {
    W[i].idx = i;
    // Bionic-like: a heap/stack address with nonzero low 12 bits, plus a tag
    // in the upper bits so a partial overwrite is recognisable.
    W[i].value = 0x00007f0000000000ull | ((uint64_t)(i + 1) << 20) | 0xab8;
    pthread_create(&W[i].th, NULL, worker_main, &W[i]);
  }
  // Signal storm: SIGUSR1 to random workers.
  struct timespec t0, t1;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  uint64_t sent = 0;
  unsigned seed = 1;
  for (;;) {
    clock_gettime(CLOCK_MONOTONIC, &t1);
    if (t1.tv_sec - t0.tv_sec >= secs) break;
    pthread_kill(W[rand_r(&seed) % nthreads].th, SIGUSR1);
    sent++;
    usleep(20);
  }
  atomic_store(&stop, 1);
  for (int i = 0; i < nthreads; i++) pthread_join(W[i].th, NULL);
  struct rusage r1;
  getrusage(RUSAGE_SELF, &r1);

  uint64_t checks = 0, bad = 0, sigs = 0, sigbad = 0;
  int bad_threads = 0;
  for (int i = 0; i < nthreads; i++) {
    checks += W[i].checks;
    bad += W[i].mismatches;
    sigs += W[i].signals;
    sigbad += W[i].sig_mismatch;
    if (W[i].mismatches) {
      if (bad_threads++ < 8)
        printf("  thread %d: wrote 0x%016" PRIx64 " first saw 0x%016" PRIx64 " after %s (%" PRIu64 " mismatches)\n",
               i, W[i].value, W[i].first_bad, W[i].first_where, (uint64_t)W[i].mismatches);
    }
  }
  printf("per-operation: clobbered / checks after that operation\n");
  for (int i = 0; i < 32 && OPS[i].where; i++)
    printf("  %-22s %9" PRIu64 " / %9" PRIu64 " (%.1f%%)\n", OPS[i].where, (uint64_t)OPS[i].n,
           (uint64_t)(OPS[i].n + OPS[i].clean), 100.0 * OPS[i].n / (double)(OPS[i].n + OPS[i].clean));
  printf("worker initial TPIDR_EL0 (thread 0) = 0x%016" PRIx64 "\n", W[0].initial);
  printf("checks=%" PRIu64 " mismatches=%" PRIu64 " threads_with_mismatch=%d\n", checks, bad, bad_threads);
  printf("signals sent=%" PRIu64 " handled=%" PRIu64 " handler_saw_wrong_value=%" PRIu64 "\n", sent, sigs, sigbad);
  printf("involuntary context switches=%ld voluntary=%ld\n", r1.ru_nivcsw - r0.ru_nivcsw, r1.ru_nvcsw - r0.ru_nvcsw);
  return bad || sigbad ? 1 : 0;
}
