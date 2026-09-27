// P0 experiment 2: Linux clone(CLONE_THREAD...) on Darwin threads.
//
// The guest side is bionic's __bionic_clone (common/lx_guest.S) with its
// `svc #0` redirected into the prototype syscall layer. The host side:
//
//   clone  -> allocate a Linux tid, honour PARENT_SETTID, copy the caller's
//             frame (x0 = 0, sp = child_stack), create a Darwin pthread with
//             a small host stack; that thread binds itself (TSD slots for
//             the lx_thread and the guest thread pointer = tls), then
//             lx_resume()s the copied frame: the child continues right after
//             the svc on the guest stack.
//   exit   -> CHILD_CLEARTID: store 0 to ctid, futex-wake it, pthread_exit
//             from the host stack.
//   futex  -> WAIT/WAKE on os_sync_wait_on_address (experiment 3 covers the
//             full mapping).
#include "../common/lx.h"

#include <errno.h>
#include <inttypes.h>
#include <os/os_sync_wait_on_address.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#define CLONE_VM 0x00000100
#define CLONE_FS 0x00000200
#define CLONE_FILES 0x00000400
#define CLONE_SIGHAND 0x00000800
#define CLONE_THREAD 0x00010000
#define CLONE_SYSVSEM 0x00040000
#define CLONE_SETTLS 0x00080000
#define CLONE_PARENT_SETTID 0x00100000
#define CLONE_CHILD_CLEARTID 0x00200000
#define CLONE_CHILD_SETTID 0x01000000
#define BIONIC_THREAD_FLAGS                                                              \
  (CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM | \
   CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID)

#define NR_clone 220
#define NR_exit 93
#define NR_gettid 178
#define NR_getpid 172
#define NR_futex 98
#define NR_set_tid_address 96
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_PRIVATE_FLAG 128

// Not variadic: Darwin passes variadic arguments on the stack.
long guest_syscall6(long nr, long a0, long a1, long a2, long a3, long a4, long a5);
#define GS_(nr, a, b, c, d, e, f, ...) \
  guest_syscall6((long)(nr), (long)(a), (long)(b), (long)(c), (long)(d), (long)(e), (long)(f))
#define guest_syscall(...) GS_(__VA_ARGS__, 0, 0, 0, 0, 0, 0, 0)
int guest_bionic_clone(unsigned long flags, void *child_stack, void *ptid, void *tls, void *ctid,
                       int (*fn)(void *), void *arg);
int guest_clone_regs_probe(unsigned long flags, void *child_stack, void *ptid, void *tls, void *ctid,
                           uint64_t *out);

static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

// ---------------------------------------------------------------- host side

static _Atomic int next_tid = 1000;
static const size_t kHostStack = 128 * 1024;

static long futex_host(uint32_t *uaddr, int op, uint32_t val) {
  switch (op & ~FUTEX_PRIVATE_FLAG) {
    case FUTEX_WAIT:
      if (__atomic_load_n(uaddr, __ATOMIC_SEQ_CST) != val) return -EAGAIN;
      if (os_sync_wait_on_address(uaddr, val, 4, OS_SYNC_WAIT_ON_ADDRESS_NONE) < 0) return -errno;
      return 0;
    case FUTEX_WAKE: {
      long n = 0;
      if (val == INT32_MAX) return os_sync_wake_by_address_all(uaddr, 4, OS_SYNC_WAKE_BY_ADDRESS_NONE) == 0 ? 1 : 0;
      while (n < (long)val && os_sync_wake_by_address_any(uaddr, 4, OS_SYNC_WAKE_BY_ADDRESS_NONE) == 0) n++;
      return n;
    }
  }
  return -ENOSYS;
}

static void *host_thread_start(void *arg) {
  struct lx_thread *t = arg;
  char anchor;
  lx_bind_thread(t, (uint64_t)&anchor - 512);
  lx_resume(&t->f);  // continue the guest right after its clone svc
}

static long do_clone(struct lx_thread *parent) {
  struct lx_frame *f = &parent->f;
  unsigned long flags = f->x[0];
  uint64_t child_sp = f->x[1];
  uint32_t *ptid = (uint32_t *)f->x[2];
  uint64_t tls = f->x[3];
  uint32_t *ctid = (uint32_t *)f->x[4];
  if ((flags & (CLONE_VM | CLONE_THREAD)) != (CLONE_VM | CLONE_THREAD)) return -ENOSYS;  // fork is elsewhere

  struct lx_thread *c = calloc(1, sizeof *c);
  c->f = *f;
  c->f.x[0] = 0;
  if (child_sp) c->f.sp = child_sp;
  c->tid = atomic_fetch_add(&next_tid, 1);
  c->guest_tp = (flags & CLONE_SETTLS) ? tls : parent->guest_tp;
  if (flags & CLONE_CHILD_CLEARTID) c->clear_child_tid = ctid;
  // Linux writes both before the child can run.
  if (flags & CLONE_PARENT_SETTID) __atomic_store_n(ptid, (uint32_t)c->tid, __ATOMIC_RELEASE);
  if (flags & CLONE_CHILD_SETTID) __atomic_store_n(ctid, (uint32_t)c->tid, __ATOMIC_RELEASE);

  // The child can run, exit and free c before pthread_create returns: never
  // touch c afterwards (reading c->tid there once returned 0 to the parent,
  // which then took the child path).
  const int tid = c->tid;
  pthread_attr_t a;
  pthread_attr_init(&a);
  pthread_attr_setstacksize(&a, kHostStack);
  pthread_attr_setdetachstate(&a, PTHREAD_CREATE_DETACHED);
  pthread_t th;
  int e = pthread_create(&th, &a, host_thread_start, c);
  pthread_attr_destroy(&a);
  if (e) {
    free(c);
    return -EAGAIN;
  }
  return tid;
}

__attribute__((noreturn)) static void do_exit(struct lx_thread *t) {
  if (t->clear_child_tid) {
    __atomic_store_n(t->clear_child_tid, 0, __ATOMIC_SEQ_CST);
    futex_host(t->clear_child_tid, FUTEX_WAKE, 1);
  }
  free(t);
  pthread_exit(NULL);  // we are on the host stack; the guest stack is the guest's business
}

void lx_dispatch(struct lx_thread *t) {
  struct lx_frame *f = &t->f;
  long r;
  switch (f->x[8]) {
    case NR_clone: r = do_clone(t); break;
    case NR_exit: do_exit(t);
    case NR_gettid: r = t->tid; break;
    case NR_getpid: r = 1000; break;
    case NR_set_tid_address: t->clear_child_tid = (uint32_t *)f->x[0]; r = t->tid; break;
    case NR_futex: r = futex_host((uint32_t *)f->x[0], (int)f->x[1], (uint32_t)f->x[2]); break;
    default: r = -ENOSYS;
  }
  f->x[0] = (uint64_t)r;
}

// --------------------------------------------------------------- guest side

// A mock of bionic's pthread_internal_t: the TLS slots live inside it and
// tid doubles as ptid and ctid, exactly like bionic.
struct gthread {
  uint64_t tls[8];
  _Alignas(4) uint32_t tid;
  uint64_t seen_tp;
  long seen_gettid;
  uint32_t seen_tid_field;
  uint64_t t_clone, t_running;
  void *stack;
};

void guest_start_thread(int (*fn)(void *), void *arg) {
  int r = fn(arg);
  guest_syscall(NR_exit, r);
}

static int child_fn(void *arg) {
  struct gthread *g = arg;
  g->t_running = now_ns();
  g->seen_tp = lx_read_guest_tp();  // rewritten `mrs x0, tpidr_el0`
  g->seen_gettid = guest_syscall(NR_gettid);
  g->seen_tid_field = __atomic_load_n(&g->tid, __ATOMIC_ACQUIRE);
  return 0;
}

static const size_t kGuestStack = 256 * 1024;

static int guest_pthread_create(struct gthread *g) {
  g->stack = mmap(NULL, kGuestStack, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  g->t_clone = now_ns();
  return guest_bionic_clone(BIONIC_THREAD_FLAGS, (char *)g->stack + kGuestStack, &g->tid, &g->tls[0], &g->tid,
                            child_fn, g);
}

// bionic's pthread_join: wait on the tid word until the kernel clears it.
static void guest_pthread_join(struct gthread *g) {
  uint32_t t;
  while ((t = __atomic_load_n(&g->tid, __ATOMIC_ACQUIRE)) != 0) guest_syscall(NR_futex, &g->tid, FUTEX_WAIT, t, 0);
  munmap(g->stack, kGuestStack);
}

static int cmp_u64(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return x < y ? -1 : x > y;
}

static void *native_noop(void *a) { return a; }

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  lx_init_process();
  // The main thread's guest code runs on the process stack; lx_dispatch gets
  // its own host stack.
  static struct lx_thread main_t;
  void *hs = malloc(kHostStack);
  main_t.tid = 999;
  lx_bind_thread(&main_t, (uint64_t)hs + kHostStack);
  int fails = 0;

  // 1. Register state handed to the child.
  {
    uint64_t out[8] = {0};
    uint32_t tidw = 0;
    void *stk = mmap(NULL, kGuestStack, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    uint64_t top = (uint64_t)stk + kGuestStack;
    int tid = guest_clone_regs_probe(BIONIC_THREAD_FLAGS, (void *)top, &tidw, (void *)0x7777000, &tidw, out);
    uint32_t t;
    while ((t = __atomic_load_n(&tidw, __ATOMIC_ACQUIRE)) != 0) guest_syscall(NR_futex, &tidw, FUTEX_WAIT, t, 0);
    int ok = out[0] == 0 && out[1] == 0x1919 && out[2] == 0x2020 && out[3] == 0x2828 && out[4] == 0xbeef &&
             out[5] == 0x5a5a && out[6] == top && tid >= 1000;
    printf("child register state: x0=%" PRIu64 " x19=%#" PRIx64 " x20=%#" PRIx64 " x28=%#" PRIx64 " d8=%#" PRIx64
           " v9.d[1]=%#" PRIx64 " sp==child_stack:%s -> %s\n",
           out[0], out[1], out[2], out[3], out[4], out[5], out[6] == top ? "yes" : "no", ok ? "OK" : "FAIL");
    fails += !ok;
    munmap(stk, kGuestStack);
  }

  // 2. bionic-style create/join, sequential.
  enum { SEQ = 2000 };
  static uint64_t start_lat[SEQ], total[SEQ];
  int bad = 0;
  for (int i = 0; i < SEQ; i++) {
    struct gthread g = {0};
    uint64_t t0 = now_ns();
    int tid = guest_pthread_create(&g);
    guest_pthread_join(&g);
    total[i] = now_ns() - t0;
    start_lat[i] = g.t_running - g.t_clone;
    if (g.seen_tp != (uint64_t)&g.tls[0] || g.seen_gettid != tid || g.seen_tid_field != (uint32_t)tid) bad++;
  }
  qsort(start_lat, SEQ, sizeof(uint64_t), cmp_u64);
  qsort(total, SEQ, sizeof(uint64_t), cmp_u64);
  printf("sequential guest clone+join x%d: TLS/gettid/PARENT_SETTID mismatches=%d\n", SEQ, bad);
  printf("  clone -> child running: p50=%.1f us p99=%.1f us\n", start_lat[SEQ / 2] / 1e3, start_lat[SEQ * 99 / 100] / 1e3);
  printf("  clone -> join returns:  p50=%.1f us p99=%.1f us\n", total[SEQ / 2] / 1e3, total[SEQ * 99 / 100] / 1e3);
  fails += bad != 0;

  {
    static uint64_t nat[SEQ];
    for (int i = 0; i < SEQ; i++) {
      pthread_t th;
      uint64_t t0 = now_ns();
      pthread_create(&th, NULL, native_noop, NULL);
      pthread_join(th, NULL);
      nat[i] = now_ns() - t0;
    }
    qsort(nat, SEQ, sizeof(uint64_t), cmp_u64);
    printf("  native pthread_create+join baseline: p50=%.1f us p99=%.1f us\n", nat[SEQ / 2] / 1e3,
           nat[SEQ * 99 / 100] / 1e3);
  }

  // 3. Many concurrent guest threads.
  enum { CONC = 512 };
  static struct gthread gs[CONC];
  uint64_t t0 = now_ns();
  for (int i = 0; i < CONC; i++) {
    if (guest_pthread_create(&gs[i]) < 0) {
      printf("clone failed at %d\n", i);
      return 1;
    }
  }
  for (int i = 0; i < CONC; i++) guest_pthread_join(&gs[i]);
  uint64_t dt = now_ns() - t0;
  bad = 0;
  for (int i = 0; i < CONC; i++)
    if (gs[i].seen_tp != (uint64_t)&gs[i].tls[0] || gs[i].tid != 0 || gs[i].seen_gettid < 1000) bad++;
  printf("concurrent: %d guest threads created then joined in %.2f ms, mismatches=%d\n", CONC, dt / 1e6, bad);
  fails += bad != 0;
  printf("%s\n", fails ? "FAIL" : "PASS");
  return fails != 0;
}
