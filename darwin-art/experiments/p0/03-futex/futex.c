// P0 experiment 3: Linux futex on Darwin.
//
// Two backends are prototyped behind one lx_futex() entry point:
//
//   direct  - every op maps straight onto os_sync_wait_on_address /
//             os_sync_wake_by_address_{any,all} (the public wrapper of
//             __ulock_wait2 / __ulock_wake). Waiters are keyed by address
//             (private) or by the backing VM object (SHARED).
//   table   - private futexes only: an in-process hash table of waiters,
//             each blocking on its own per-thread word through os_sync.
//             This gives exact Linux semantics for FUTEX_WAKE_BITSET with a
//             real bitset, FUTEX_REQUEUE / FUTEX_CMP_REQUEUE and accurate
//             wake counts, none of which __ulock can express.
//
// Guest clocks: the guest's CLOCK_MONOTONIC is defined as mach_absolute_time
// scaled to ns (== Darwin CLOCK_UPTIME_RAW; like Linux CLOCK_MONOTONIC it
// stops during sleep), so absolute MONOTONIC deadlines convert exactly to
// os_sync_wait_on_address_with_deadline(OS_CLOCK_MACH_ABSOLUTE_TIME).
// CLOCK_REALTIME deadlines are converted to a relative timeout on entry.
#include <errno.h>
#include <inttypes.h>
#include <mach/mach_time.h>
#include <os/clock.h>
#include <os/os_sync_wait_on_address.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/event.h>
#include <os/lock.h>

// Linux uapi values.
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_REQUEUE 3
#define FUTEX_CMP_REQUEUE 4
#define FUTEX_WAIT_BITSET 9
#define FUTEX_WAKE_BITSET 10
#define FUTEX_PRIVATE_FLAG 128
#define FUTEX_CLOCK_REALTIME 256
#define FUTEX_CMD_MASK ~(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME)
#define FUTEX_BITSET_MATCH_ANY 0xffffffffu
#define FUTEX_WAIT_PRIVATE_ (FUTEX_WAIT | FUTEX_PRIVATE_FLAG)
struct lx_timespec { int64_t tv_sec, tv_nsec; };

// Private libsystem_kernel API that os_sync wraps (for comparison).
extern int __ulock_wait2(uint32_t op, void *addr, uint64_t value, uint64_t timeout_ns, uint64_t value2);
extern int __ulock_wake(uint32_t op, void *addr, uint64_t wake_value);
#define UL_COMPARE_AND_WAIT 1
#define UL_COMPARE_AND_WAIT_SHARED 3
#define ULF_WAKE_ALL 0x100
#define ULF_NO_ERRNO 0x01000000

static mach_timebase_info_data_t tb;
static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }
static inline uint64_t ns_to_abs(uint64_t ns) { return ns * tb.denom / tb.numer; }
// Guest clock_gettime(CLOCK_MONOTONIC) in this design.
static inline uint64_t guest_monotonic_ns(void) { return now_ns(); }
static inline uint64_t guest_realtime_ns(void) { return clock_gettime_nsec_np(CLOCK_REALTIME); }

// ------------------------------------------------------------ direct backend

enum { K_PRIVATE, K_SHARED };

static long os_wait(uint32_t *a, uint32_t val, int kind, int cmd, int op, const struct lx_timespec *ts) {
  uint32_t f = kind == K_SHARED ? OS_SYNC_WAIT_ON_ADDRESS_SHARED : OS_SYNC_WAIT_ON_ADDRESS_NONE;
  // Linux returns EAGAIN on mismatch; os_sync just returns. Check first; a
  // change after this check makes os_sync return at once, which callers see
  // as a spurious wakeup (allowed).
  if (__atomic_load_n(a, __ATOMIC_SEQ_CST) != val) return -EAGAIN;
  int r;
  if (!ts) {
    r = os_sync_wait_on_address(a, val, 4, f);
  } else if (cmd == FUTEX_WAIT) {  // relative, measured on CLOCK_MONOTONIC
    uint64_t rel = (uint64_t)ts->tv_sec * 1000000000ull + (uint64_t)ts->tv_nsec;
    if (rel == 0) return -ETIMEDOUT;
    r = os_sync_wait_on_address_with_timeout(a, val, 4, f, OS_CLOCK_MACH_ABSOLUTE_TIME, rel);
  } else {  // WAIT_BITSET: absolute
    uint64_t abs = (uint64_t)ts->tv_sec * 1000000000ull + (uint64_t)ts->tv_nsec;
    if (op & FUTEX_CLOCK_REALTIME) {
      uint64_t now = guest_realtime_ns();
      if (abs <= now) return -ETIMEDOUT;
      r = os_sync_wait_on_address_with_timeout(a, val, 4, f, OS_CLOCK_MACH_ABSOLUTE_TIME, abs - now);
    } else {
      if (abs <= guest_monotonic_ns()) return -ETIMEDOUT;
      r = os_sync_wait_on_address_with_deadline(a, val, 4, f, OS_CLOCK_MACH_ABSOLUTE_TIME, ns_to_abs(abs));
    }
  }
  if (r < 0) return errno == ETIMEDOUT ? -ETIMEDOUT : errno == EINTR ? -EINTR : -errno;
  return 0;
}

static long os_wake(uint32_t *a, uint32_t n, int kind) {
  uint32_t f = kind == K_SHARED ? OS_SYNC_WAKE_BY_ADDRESS_SHARED : OS_SYNC_WAKE_BY_ADDRESS_NONE;
  if (n >= INT32_MAX) return os_sync_wake_by_address_all(a, 4, f) == 0 ? 1 : 0;  // count unknown
  long woken = 0;
  while (woken < (long)n && os_sync_wake_by_address_any(a, 4, f) == 0) woken++;
  return woken;
}

// ------------------------------------------------------------- table backend

struct waiter {
  struct waiter *next;
  uint32_t *uaddr;
  uint32_t bitset;
  _Atomic uint32_t state;  // 0 waiting, 1 claimed by a waker
  int kq;                  // >= 0: timed waiter blocked in kevent (see below)
};

// Timed waits: os_sync/__ulock timeouts carry ~10% timer leeway on Darwin
// (2 ms late on a 20 ms wait, timer_slack.c). A kqueue EVFILT_TIMER with
// NOTE_LEEWAY 0 fires within tens of us, so the table backend parks timed
// waiters in a per-thread kqueue: EVFILT_USER is the wake, EVFILT_TIMER the
// deadline.
static int g_timed_kqueue = 1;
static __thread int tls_kq = -1;
static int thread_kq(void) {
  if (tls_kq < 0) {
    tls_kq = kqueue();
    struct kevent64_s ev;
    EV_SET64(&ev, 1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0, 0, 0);
    kevent64(tls_kq, &ev, 1, NULL, 0, 0, NULL);
  }
  return tls_kq;
}
// A claimed waiter may return (and its stack frame die) as soon as it sees
// state != 0, so wakers copy what they need under the bucket lock and
// never touch the waiter afterwards. A late wake on a reused address is a
// spurious wakeup for whoever waits there now, which every wait loop allows.
struct wake_ref { _Atomic uint32_t *state; int kq; };
static void claim(struct waiter *w, struct wake_ref *out) {
  out->state = &w->state;
  out->kq = w->kq;
  atomic_store(&w->state, 1);
}
static void wake_ref(const struct wake_ref *r) {
  if (r->kq >= 0) {
    struct kevent64_s ev;
    EV_SET64(&ev, 1, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0, 0, 0);
    kevent64(r->kq, &ev, 1, NULL, 0, 0, NULL);
  } else {
    os_sync_wake_by_address_any(r->state, 4, 0);
  }
}
#define NBUCKET 256
static struct bucket { os_unfair_lock m; struct waiter *head; } buckets[NBUCKET];
static struct bucket *bucket_of(uint32_t *a) { return &buckets[((uintptr_t)a >> 2) * 0x9E3779B97F4A7C15ull >> 56]; }

static void unlink_waiter(struct bucket *b, struct waiter *w) {
  for (struct waiter **p = &b->head; *p; p = &(*p)->next)
    if (*p == w) { *p = w->next; return; }
}

static long tbl_wait(uint32_t *a, uint32_t val, uint32_t bitset, int cmd, int op, const struct lx_timespec *ts) {
  if (bitset == 0) return -EINVAL;
  struct bucket *b = bucket_of(a);
  struct waiter w = {.uaddr = a, .bitset = bitset, .kq = (ts && g_timed_kqueue) ? thread_kq() : -1};
  os_unfair_lock_lock(&b->m);
  if (__atomic_load_n(a, __ATOMIC_SEQ_CST) != val) {  // atomic with respect to wakers
    os_unfair_lock_unlock(&b->m);
    return -EAGAIN;
  }
  w.next = b->head;
  b->head = &w;
  os_unfair_lock_unlock(&b->m);

  uint64_t deadline_abs = 0;  // mach ticks, 0 = none
  if (ts) {
    uint64_t t = (uint64_t)ts->tv_sec * 1000000000ull + (uint64_t)ts->tv_nsec;
    if (cmd == FUTEX_WAIT) deadline_abs = ns_to_abs(guest_monotonic_ns() + t);
    else if (op & FUTEX_CLOCK_REALTIME) {
      uint64_t now = guest_realtime_ns();
      deadline_abs = ns_to_abs(guest_monotonic_ns() + (t > now ? t - now : 0));
    } else deadline_abs = ns_to_abs(t);
  }
  long ret = 0;
  if (w.kq >= 0) {
    struct kevent64_s ev;
    int64_t rel = (int64_t)(deadline_abs - mach_absolute_time()) * tb.numer / tb.denom;
    if (rel < 1) rel = 1;
    EV_SET64(&ev, 2, EVFILT_TIMER, EV_ADD | EV_ONESHOT, NOTE_NSECONDS | NOTE_LEEWAY | NOTE_CRITICAL, (uint64_t)rel, 0, 0, 0);
    ev.ext[1] = 0;  // leeway
    kevent64(w.kq, &ev, 1, NULL, 0, 0, NULL);
    while (atomic_load(&w.state) == 0) {
      struct kevent64_s out;
      int n = kevent64(w.kq, NULL, 0, &out, 1, 0, NULL);
      if (n == 1 && out.filter == EVFILT_TIMER && atomic_load(&w.state) == 0) { ret = -ETIMEDOUT; break; }
      if (n < 0 && errno == EINTR && atomic_load(&w.state) == 0) { ret = -EINTR; break; }
    }
    EV_SET64(&ev, 2, EVFILT_TIMER, EV_DELETE, 0, 0, 0, 0, 0);
    kevent64(w.kq, &ev, 1, NULL, 0, 0, NULL);
  }
  while (!ret && atomic_load(&w.state) == 0) {
    int r;
    if (deadline_abs) {
      if (mach_absolute_time() >= deadline_abs) { ret = -ETIMEDOUT; break; }
      r = os_sync_wait_on_address_with_deadline(&w.state, 0, 4, 0, OS_CLOCK_MACH_ABSOLUTE_TIME, deadline_abs);
    } else {
      r = os_sync_wait_on_address(&w.state, 0, 4, 0);
    }
    if (r < 0 && errno == ETIMEDOUT) { ret = -ETIMEDOUT; break; }
    if (r < 0 && errno == EINTR && atomic_load(&w.state) == 0) { ret = -EINTR; break; }
  }
  if (ret) {
    // Requeue may have moved us; lock whichever bucket we are in now.
    for (;;) {
      struct bucket *cb = bucket_of(w.uaddr);
      os_unfair_lock_lock(&cb->m);
      if (cb != bucket_of(w.uaddr)) { os_unfair_lock_unlock(&cb->m); continue; }
      // Claimed while timing out: the wake counts, return 0 as Linux does.
      if (atomic_load(&w.state) == 0) unlink_waiter(cb, &w); else ret = 0;
      os_unfair_lock_unlock(&cb->m);
      break;
    }
  }
  return ret;
}

static long tbl_wake(uint32_t *a, uint32_t n, uint32_t bitset) {
  if (bitset == 0) return -EINVAL;
  struct bucket *b = bucket_of(a);
  long woken = 0;
  for (;;) {
    struct wake_ref refs[64];
    int k = 0;
    os_unfair_lock_lock(&b->m);
    for (struct waiter **p = &b->head; *p && woken < (long)n && k < 64;) {
      struct waiter *w = *p;
      if (w->uaddr == a && (w->bitset & bitset)) {
        *p = w->next;
        claim(w, &refs[k++]);
        woken++;
      } else p = &w->next;
    }
    os_unfair_lock_unlock(&b->m);
    for (int i = 0; i < k; i++) wake_ref(&refs[i]);
    if (k < 64 || woken >= (long)n) return woken;
  }
}

static long tbl_requeue(uint32_t *a, uint32_t nwake, uint32_t nreq, uint32_t *a2, bool cmp, uint32_t val3) {
  struct bucket *b1 = bucket_of(a), *b2 = bucket_of(a2);
  struct bucket *lo = b1 < b2 ? b1 : b2, *hi = b1 < b2 ? b2 : b1;
  if (nwake > 64) nwake = 64;  // prototype limit; callers wake 0 or 1
  os_unfair_lock_lock(&lo->m);
  if (hi != lo) os_unfair_lock_lock(&hi->m);
  if (cmp && __atomic_load_n(a, __ATOMIC_SEQ_CST) != val3) {
    if (hi != lo) os_unfair_lock_unlock(&hi->m);
    os_unfair_lock_unlock(&lo->m);
    return -EAGAIN;
  }
  struct wake_ref refs[64];
  long woken = 0, moved = 0;
  for (struct waiter **p = &b1->head; *p;) {
    struct waiter *w = *p;
    if (w->uaddr != a) { p = &w->next; continue; }
    if (woken < (long)nwake) {
      *p = w->next;
      claim(w, &refs[woken++]);
    } else if (moved < (long)nreq) {
      *p = w->next;
      w->uaddr = a2;
      w->next = b2->head;
      b2->head = w;
      moved++;
    } else break;
  }
  if (hi != lo) os_unfair_lock_unlock(&hi->m);
  os_unfair_lock_unlock(&lo->m);
  for (long i = 0; i < woken; i++) wake_ref(&refs[i]);
  return woken + moved;
}

// ---------------------------------------------------------------- lx_futex

static int g_private_backend_table = 1;

// `kind` is decided by the syscall layer from the mapping that backs uaddr
// (MAP_SHARED -> K_SHARED, anything else -> K_PRIVATE), NOT from
// FUTEX_PRIVATE_FLAG: on Linux a non-private op on private memory matches
// a private op on the same address, and os_sync keeps the two flavours in
// separate namespaces (measured below).
static long lx_futex(uint32_t *uaddr, int op, uint32_t val, const struct lx_timespec *ts, uint32_t *uaddr2,
                     uint32_t val3, int kind) {
  int cmd = op & FUTEX_CMD_MASK;
  bool table = kind == K_PRIVATE && g_private_backend_table;
  switch (cmd) {
    case FUTEX_WAIT:
      return table ? tbl_wait(uaddr, val, FUTEX_BITSET_MATCH_ANY, cmd, op, ts) : os_wait(uaddr, val, kind, cmd, op, ts);
    case FUTEX_WAIT_BITSET:
      if (table) return tbl_wait(uaddr, val, val3, cmd, op, ts);
      if (val3 != FUTEX_BITSET_MATCH_ANY) return -ENOSYS;  // not expressible on __ulock
      return os_wait(uaddr, val, kind, cmd, op, ts);
    case FUTEX_WAKE:
      return table ? tbl_wake(uaddr, val, FUTEX_BITSET_MATCH_ANY) : os_wake(uaddr, val, kind);
    case FUTEX_WAKE_BITSET:
      if (table) return tbl_wake(uaddr, val, val3);
      if (val3 != FUTEX_BITSET_MATCH_ANY) return -ENOSYS;
      return os_wake(uaddr, val, kind);
    case FUTEX_REQUEUE:
    case FUTEX_CMP_REQUEUE: {
      uint32_t nreq = (uint32_t)(uintptr_t)ts;  // val2 travels in the timeout slot
      if (table) return tbl_requeue(uaddr, val, nreq, uaddr2, cmd == FUTEX_CMP_REQUEUE, val3);
      // Direct backend: waking instead of moving is a legal (spurious) wakeup.
      if (cmd == FUTEX_CMP_REQUEUE && __atomic_load_n(uaddr, __ATOMIC_SEQ_CST) != val3) return -EAGAIN;
      os_wake(uaddr, INT32_MAX, kind);
      return val;
    }
  }
  return -ENOSYS;
}

// ------------------------------------------------------------------- tests

static int fails;
#define EXPECT(cond, ...) do { int ok_ = (cond); printf("  [%s] ", ok_ ? " ok " : "FAIL"); printf(__VA_ARGS__); printf("\n"); fails += !ok_; } while (0)

static struct lx_timespec ts_from_ns(uint64_t ns) { return (struct lx_timespec){(int64_t)(ns / 1000000000ull), (int64_t)(ns % 1000000000ull)}; }

static void test_timeouts(const char *backend) {
  printf("timeouts (%s backend, private):\n", backend);
  uint32_t word = 5;
  EXPECT(lx_futex(&word, FUTEX_WAIT_PRIVATE_, 6, NULL, NULL, 0, K_PRIVATE) == -EAGAIN, "WAIT with stale value -> EAGAIN");
  struct lx_timespec rel = ts_from_ns(20000000);
  uint64_t t0 = now_ns();
  long r = lx_futex(&word, FUTEX_WAIT | FUTEX_PRIVATE_FLAG, 5, &rel, NULL, 0, K_PRIVATE);
  uint64_t dt = now_ns() - t0;
  EXPECT(r == -ETIMEDOUT && dt >= 20000000, "WAIT relative 20 ms -> ETIMEDOUT after %.3f ms", dt / 1e6);
  struct lx_timespec zero = {0, 0};
  EXPECT(lx_futex(&word, FUTEX_WAIT | FUTEX_PRIVATE_FLAG, 5, &zero, NULL, 0, K_PRIVATE) == -ETIMEDOUT, "WAIT {0,0} -> ETIMEDOUT immediately");

  struct lx_timespec absm = ts_from_ns(guest_monotonic_ns() + 20000000);
  t0 = now_ns();
  r = lx_futex(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 5, &absm, NULL, FUTEX_BITSET_MATCH_ANY, K_PRIVATE);
  dt = now_ns() - t0;
  uint64_t late = now_ns() - ((uint64_t)absm.tv_sec * 1000000000ull + (uint64_t)absm.tv_nsec);
  EXPECT(r == -ETIMEDOUT && dt >= 19900000, "WAIT_BITSET abs CLOCK_MONOTONIC +20 ms -> ETIMEDOUT, %.3f ms, %.1f us past deadline", dt / 1e6, (double)(int64_t)late / 1e3);

  uint64_t rt_deadline = guest_realtime_ns() + 20000000;
  struct lx_timespec absr = ts_from_ns(rt_deadline);
  t0 = now_ns();
  r = lx_futex(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME, 5, &absr, NULL, FUTEX_BITSET_MATCH_ANY, K_PRIVATE);
  dt = now_ns() - t0;
  int64_t rlate = (int64_t)(guest_realtime_ns() - rt_deadline);
  EXPECT(r == -ETIMEDOUT && dt >= 19900000, "WAIT_BITSET abs CLOCK_REALTIME +20 ms -> ETIMEDOUT, %.3f ms, %.1f us past deadline", dt / 1e6, rlate / 1e3);
  struct lx_timespec past = ts_from_ns(guest_monotonic_ns() - 1000);
  EXPECT(lx_futex(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, 5, &past, NULL, FUTEX_BITSET_MATCH_ANY, K_PRIVATE) == -ETIMEDOUT, "WAIT_BITSET deadline in the past -> ETIMEDOUT");
}

struct wargs { uint32_t *a; int op; uint32_t val; uint32_t bitset; _Atomic int done; long ret; };
static void *waiter_thread(void *p) {
  struct wargs *w = p;
  w->ret = lx_futex(w->a, w->op, w->val, NULL, NULL, w->bitset, K_PRIVATE);
  atomic_store(&w->done, 1);
  return NULL;
}
static int count_done(struct wargs *w, int n) { int c = 0; for (int i = 0; i < n; i++) c += atomic_load(&w[i].done); return c; }
static int tbl_count(uint32_t *a) {
  struct bucket *b = bucket_of(a);
  int n = 0;
  os_unfair_lock_lock(&b->m);
  for (struct waiter *w = b->head; w; w = w->next) n += w->uaddr == a;
  os_unfair_lock_unlock(&b->m);
  return n;
}
static void settle(void) { usleep(30000); }
// Wait until n waiters are parked on a (table backend) or just settle (direct).
static void settle_on(uint32_t *a, int n) {
  if (!g_private_backend_table) { settle(); return; }
  for (int i = 0; i < 2000 && tbl_count(a) < n; i++) usleep(1000);
}

static void test_wake_counts(const char *backend) {
  printf("wake counts, bitsets, requeue (%s backend):\n", backend);
  uint32_t word = 1;
  enum { N = 8 };
  struct wargs w[N];
  pthread_t th[N];
  for (int i = 0; i < N; i++) {
    w[i] = (struct wargs){.a = &word, .op = FUTEX_WAIT | FUTEX_PRIVATE_FLAG, .val = 1};
    pthread_create(&th[i], NULL, waiter_thread, &w[i]);
  }
  settle_on(&word, N);
  long r = lx_futex(&word, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 3, NULL, NULL, 0, K_PRIVATE);
  settle();
  EXPECT(r == 3 && count_done(w, N) == 3, "8 waiters, WAKE 3 -> returned %ld, %d woke", r, count_done(w, N));
  r = lx_futex(&word, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, INT32_MAX, NULL, NULL, 0, K_PRIVATE);
  for (int i = 0; i < N; i++) pthread_join(th[i], NULL);
  EXPECT(count_done(w, N) == N, "WAKE INT_MAX wakes the rest (returned %ld; the direct backend cannot count)", r);

  // Bitsets.
  for (int i = 0; i < 4; i++) {
    w[i] = (struct wargs){.a = &word, .op = FUTEX_WAIT_BITSET | FUTEX_PRIVATE_FLAG, .val = 1, .bitset = (i & 1) ? 2 : 1};
    pthread_create(&th[i], NULL, waiter_thread, &w[i]);
  }
  settle_on(&word, 4);
  if (strcmp(backend, "direct") == 0) {
    int early = count_done(w, 4);
    r = lx_futex(&word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, INT32_MAX, NULL, NULL, 2, K_PRIVATE);
    EXPECT(r == -ENOSYS, "WAKE_BITSET(bitset=2) is not expressible on __ulock -> %ld (waiters started? %d still blocked)", r, 4 - early);
    lx_futex(&word, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, INT32_MAX, NULL, NULL, 0, K_PRIVATE);
    for (int i = 0; i < 4; i++) pthread_join(th[i], NULL);
  } else {
    r = lx_futex(&word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, INT32_MAX, NULL, NULL, 2, K_PRIVATE);
    settle();
    int odd = atomic_load(&w[1].done) + atomic_load(&w[3].done), even = atomic_load(&w[0].done) + atomic_load(&w[2].done);
    EXPECT(r == 2 && odd == 2 && even == 0, "WAKE_BITSET(bitset=2) woke %ld: bitset-2 waiters %d/2, bitset-1 waiters %d/2", r, odd, even);
    r = lx_futex(&word, FUTEX_WAKE_BITSET | FUTEX_PRIVATE_FLAG, INT32_MAX, NULL, NULL, 1, K_PRIVATE);
    for (int i = 0; i < 4; i++) pthread_join(th[i], NULL);
    EXPECT(r == 2, "WAKE_BITSET(bitset=1) woke the other %ld", r);
  }

  // CMP_REQUEUE as ART's ConditionVariable::Broadcast / bionic would use it.
  uint32_t cond = 7, mutex = 0;
  for (int i = 0; i < 4; i++) {
    w[i] = (struct wargs){.a = &cond, .op = FUTEX_WAIT | FUTEX_PRIVATE_FLAG, .val = 7};
    pthread_create(&th[i], NULL, waiter_thread, &w[i]);
  }
  settle_on(&cond, 4);
  r = lx_futex(&cond, FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG, 1, (const struct lx_timespec *)(uintptr_t)INT32_MAX, &mutex, 7, K_PRIVATE);
  settle();
  int d1 = count_done(w, 4);
  if (strcmp(backend, "direct") == 0) {
    EXPECT(d1 == 4, "CMP_REQUEUE(wake 1, move rest) degrades to wake-all: %d/4 woke", d1);
  } else {
    EXPECT(r == 4 && d1 == 1, "CMP_REQUEUE(wake 1, move rest) -> %ld, %d/4 woke", r, d1);
    r = lx_futex(&mutex, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, INT32_MAX, NULL, NULL, 0, K_PRIVATE);
    for (int i = 0; i < 4; i++) pthread_join(th[i], NULL);
    EXPECT(r == 3, "WAKE on the requeue target woke the %ld moved waiters", r);
  }
  if (strcmp(backend, "direct") == 0) for (int i = 0; i < 4; i++) pthread_join(th[i], NULL);
}

// Does os_sync match a SHARED wait with a NONE wake on the same private address?
struct xargs { uint32_t *a; uint32_t flags; _Atomic int done; };
static void *xwait(void *p) {
  struct xargs *x = p;
  struct lx_timespec unused; (void)unused;
  os_sync_wait_on_address_with_timeout(x->a, 0, 4, x->flags, OS_CLOCK_MACH_ABSOLUTE_TIME, 300000000);
  atomic_store(&x->done, 1);
  return NULL;
}
static void test_flag_namespaces(void) {
  printf("os_sync flag namespaces on private memory:\n");
  uint32_t word = 0;
  for (int dir = 0; dir < 2; dir++) {
    struct xargs x = {.a = &word, .flags = dir ? OS_SYNC_WAIT_ON_ADDRESS_NONE : OS_SYNC_WAIT_ON_ADDRESS_SHARED};
    pthread_t t;
    pthread_create(&t, NULL, xwait, &x);
    settle();
    int r = os_sync_wake_by_address_any(&word, 4, dir ? OS_SYNC_WAKE_BY_ADDRESS_SHARED : OS_SYNC_WAKE_BY_ADDRESS_NONE);
    int e = errno;
    usleep(20000);
    int woke = atomic_load(&x.done);
    pthread_join(t, NULL);
    printf("  wait %s / wake %s: wake returned %d%s, waiter woken by it: %s\n", dir ? "NONE" : "SHARED",
           dir ? "SHARED" : "NONE", r, r < 0 ? (e == ENOENT ? " (ENOENT: no waiter in that namespace)" : "") : "",
           woke ? "yes" : "no (timed out later)");
  }
}

static void on_usr1(int s) { (void)s; }
static void *eintr_waiter(void *p) {
  uint32_t *a = p;
  uint64_t t0 = now_ns();
  long r = os_wait(a, 0, K_PRIVATE, FUTEX_WAIT, 0, NULL);
  printf("  signal during os_sync wait (SA_RESTART handler): returned %ld (%s) after %.1f ms\n", r,
         r == -EINTR ? "EINTR" : r == 0 ? "0" : "other", (now_ns() - t0) / 1e6);
  return NULL;
}
static void test_eintr(void) {
  printf("signals:\n");
  struct sigaction sa = {0};
  sa.sa_handler = on_usr1;
  sa.sa_flags = SA_RESTART;
  sigaction(SIGUSR1, &sa, NULL);
  static uint32_t word = 0;
  pthread_t t;
  pthread_create(&t, NULL, eintr_waiter, &word);
  usleep(50000);
  pthread_kill(t, SIGUSR1);
  usleep(50000);
  word = 1;
  os_sync_wake_by_address_all(&word, 4, 0);
  pthread_join(t, NULL);
}

// ------------------------------------------------------------ cross-process

static void test_cross_process(void) {
  printf("cross-process (non-private futex on MAP_SHARED):\n");
  // (a) anonymous MAP_SHARED inherited over fork: same address in both.
  // (b) a POSIX shm object mapped separately in each process, at different
  //     addresses: what binder / ashmem / memfd users look like.
  char name[64];
  snprintf(name, sizeof name, "/p0futex.%d", getpid());
  int fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
  shm_unlink(name);
  ftruncate(fd, 16384);
  uint32_t *anon = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0);
  uint32_t *shm_parent = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  anon[0] = 0; anon[1] = 0; shm_parent[0] = 0; shm_parent[16] = 0;

  enum { ROUNDS = 2000 };
  pid_t pid = fork();
  if (pid == 0) {
    // Child: map the shm object again so its address differs from the parent's.
    void *pad = mmap(NULL, 1 << 20, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0); (void)pad;
    uint32_t *shm_child = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    anon[2] = (uint32_t)(shm_child != shm_parent);
    // Ping-pong on shm word 0: wait for odd, write even.
    for (uint32_t i = 0; i < ROUNDS; i++) {
      uint32_t want = 2 * i + 1;
      uint32_t v;
      while ((v = __atomic_load_n(&shm_child[0], __ATOMIC_ACQUIRE)) != want)
        lx_futex(&shm_child[0], FUTEX_WAIT, v, NULL, NULL, 0, K_SHARED);
      __atomic_store_n(&shm_child[0], want + 1, __ATOMIC_RELEASE);
      lx_futex(&shm_child[0], FUTEX_WAKE, 1, NULL, NULL, 0, K_SHARED);
    }
    // Anonymous shared word: wait for the parent to flip it.
    while (__atomic_load_n(&anon[1], __ATOMIC_ACQUIRE) == 0) lx_futex(&anon[1], FUTEX_WAIT, 0, NULL, NULL, 0, K_SHARED);
    // PRIVATE-keyed wait on shared memory: must NOT be woken by the other process.
    struct lx_timespec t = ts_from_ns(100000000);
    long r = os_wait(&shm_child[16], 0, K_PRIVATE, FUTEX_WAIT, 0, &t);
    anon[3] = r == -ETIMEDOUT;
    _exit(0);
  }
  usleep(50000);
  uint64_t t0 = now_ns();
  for (uint32_t i = 0; i < ROUNDS; i++) {
    __atomic_store_n(&shm_parent[0], 2 * i + 1, __ATOMIC_RELEASE);
    lx_futex(&shm_parent[0], FUTEX_WAKE, 1, NULL, NULL, 0, K_SHARED);
    uint32_t v;
    while ((v = __atomic_load_n(&shm_parent[0], __ATOMIC_ACQUIRE)) != 2 * i + 2)
      lx_futex(&shm_parent[0], FUTEX_WAIT, v, NULL, NULL, 0, K_SHARED);
  }
  uint64_t dt = now_ns() - t0;
  usleep(20000);
  __atomic_store_n(&anon[1], 1, __ATOMIC_RELEASE);
  lx_futex(&anon[1], FUTEX_WAKE, 1, NULL, NULL, 0, K_SHARED);
  usleep(20000);
  os_sync_wake_by_address_any(&shm_parent[16], 4, OS_SYNC_WAKE_BY_ADDRESS_NONE);  // wrong namespace on purpose
  int st;
  waitpid(pid, &st, 0);
  EXPECT(WIFEXITED(st) && anon[2], "shm object mapped at different addresses in the two processes: %s", anon[2] ? "yes" : "no");
  EXPECT(WIFEXITED(st), "SHARED ping-pong across processes: %d round trips, %.2f us per round trip (%.2f us per wake+switch)", ROUNDS, dt / 1e3 / ROUNDS, dt / 2e3 / ROUNDS);
  EXPECT(WIFEXITED(st), "SHARED wait on inherited MAP_SHARED|MAP_ANON woken from the parent");
  EXPECT(anon[3], "private-keyed wait on shared memory is not woken from another process (Linux behaves the same)");
  close(fd);
}

// ----------------------------------------------------------------- latency

static int cmp_u64(const void *a, const void *b) { uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b; return x < y ? -1 : x > y; }

enum { PP = 100000 };
static _Alignas(64) uint32_t pp_word;
static int pp_mode;  // 0 lx_futex(table/direct via global), 1 raw __ulock
static void pp_wait(uint32_t v) {
  if (pp_mode == 1) __ulock_wait2(UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, &pp_word, v, 0, 0);
  else lx_futex(&pp_word, FUTEX_WAIT | FUTEX_PRIVATE_FLAG, v, NULL, NULL, 0, K_PRIVATE);
}
static void pp_wake(void) {
  if (pp_mode == 1) __ulock_wake(UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, &pp_word, 0);
  else lx_futex(&pp_word, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 1, NULL, NULL, 0, K_PRIVATE);
}
static void *pp_peer(void *p) {
  (void)p;
  for (uint32_t i = 0; i < PP; i++) {
    uint32_t v;
    while ((v = __atomic_load_n(&pp_word, __ATOMIC_ACQUIRE)) != 2 * i + 1) pp_wait(v);
    __atomic_store_n(&pp_word, 2 * i + 2, __ATOMIC_RELEASE);
    pp_wake();
  }
  return NULL;
}
static double pingpong(int mode) {
  pp_mode = mode;
  pp_word = 0;
  pthread_t t;
  pthread_create(&t, NULL, pp_peer, NULL);
  uint64_t t0 = now_ns();
  for (uint32_t i = 0; i < PP; i++) {
    __atomic_store_n(&pp_word, 2 * i + 1, __ATOMIC_RELEASE);
    pp_wake();
    uint32_t v;
    while ((v = __atomic_load_n(&pp_word, __ATOMIC_ACQUIRE)) != 2 * i + 2) pp_wait(v);
  }
  uint64_t dt = now_ns() - t0;
  pthread_join(t, NULL);
  return dt / 1e3 / PP;
}

// One-shot wake latency: waiter is asleep in the kernel; time from the
// wake call to the waiter running again.
enum { OS = 2000 };
static _Alignas(64) uint32_t os_word;
static _Atomic uint64_t os_t_wake, os_t_run;
static int os_timed;  // waiter passes a (long) timeout: exercises the kqueue path
static void *os_waiter(void *p) {
  (void)p;
  struct lx_timespec ten_s = {10, 0};
  for (int i = 0; i < OS; i++) {
    while (__atomic_load_n(&os_word, __ATOMIC_ACQUIRE) == 0)
      lx_futex(&os_word, FUTEX_WAIT | FUTEX_PRIVATE_FLAG, 0, os_timed ? &ten_s : NULL, NULL, 0, K_PRIVATE);
    atomic_store(&os_t_run, now_ns());
    __atomic_store_n(&os_word, 0, __ATOMIC_RELEASE);
    while (atomic_load(&os_t_run) != 0) { }
  }
  return NULL;
}
static void oneshot(const char *label) {
  static uint64_t lat[OS];
  pthread_t t;
  os_word = 0;
  pthread_create(&t, NULL, os_waiter, NULL);
  for (int i = 0; i < OS; i++) {
    usleep(200);  // let the waiter block in the kernel
    atomic_store(&os_t_wake, now_ns());
    __atomic_store_n(&os_word, 1, __ATOMIC_RELEASE);
    lx_futex(&os_word, FUTEX_WAKE | FUTEX_PRIVATE_FLAG, 1, NULL, NULL, 0, K_PRIVATE);
    uint64_t r;
    while ((r = atomic_load(&os_t_run)) == 0) { }
    lat[i] = r - atomic_load(&os_t_wake);
    atomic_store(&os_t_run, 0);
  }
  pthread_join(t, NULL);
  qsort(lat, OS, sizeof lat[0], cmp_u64);
  printf("  one-shot wake -> waiter running (%s): p50=%.1f us p90=%.1f us p99=%.1f us\n", label, lat[OS / 2] / 1e3,
         lat[OS * 9 / 10] / 1e3, lat[OS * 99 / 100] / 1e3);
}

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  alarm(240);  // a lost wakeup must fail the run, not hang it
  mach_timebase_info(&tb);
  for (int i = 0; i < NBUCKET; i++) buckets[i].m = OS_UNFAIR_LOCK_INIT;
  printf("mach timebase %u/%u\n", tb.numer, tb.denom);

  g_private_backend_table = 0;
  test_timeouts("direct");
  test_wake_counts("direct");
  g_private_backend_table = 1;
  g_timed_kqueue = 0;
  test_timeouts("table, os_sync timeouts");
  g_timed_kqueue = 1;
  test_timeouts("table, kqueue timeouts");
  test_wake_counts("table");
  test_flag_namespaces();
  test_eintr();
  test_cross_process();

  printf("latency (thread ping-pong, %d round trips; per-hop = half a round trip):\n", PP);
  double us;
  us = pingpong(1); printf("  raw __ulock_wait2/__ulock_wake      %.2f us/round trip (%.2f us/hop)\n", us, us / 2);
  g_private_backend_table = 0;
  us = pingpong(0); printf("  lx_futex direct (os_sync)          %.2f us/round trip (%.2f us/hop)\n", us, us / 2);
  g_private_backend_table = 1;
  us = pingpong(0); printf("  lx_futex table                     %.2f us/round trip (%.2f us/hop)\n", us, us / 2);
  g_private_backend_table = 0;
  oneshot("direct");
  g_private_backend_table = 1;
  oneshot("table");
  os_timed = 1;
  oneshot("table, timed wait parked in kqueue");
  os_timed = 0;
  printf("%s (%d failures)\n", fails ? "FAIL" : "PASS", fails);
  return fails != 0;
}
