// P0 experiment 1b: what does replacing guest `mrs xN, tpidr_el0` cost?
//
// Since XNU owns TPIDR_EL0 (see tpidr.c), the guest thread pointer has to
// live somewhere else. The cheapest per-thread storage reachable without a
// scratch register is the Darwin TSD array at TPIDRRO_EL0: a pthread key
// gives a slot, and `mrs xN, tpidrro_el0; ldr xN, [xN, #key*8]` reads it
// using only the destination register.
//
// A load-time rewriter cannot grow the instruction in place, so each
// `mrs xN, tpidr_el0` becomes `b thunk_i`, and thunk_i is
//   mrs xN, tpidrro_el0 ; ldr xN, [xN, #slot] ; b back_i
// `msr tpidr_el0, xN` (rare: bionic's __set_tls) becomes a thunk that
// spills one scratch register.
//
// This program checks the replacement is correct across context switches
// and measures its cost against the raw instruction.
#include <inttypes.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

static pthread_key_t guest_tp_key;

static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

#define ITERS 200000000ull

// (a) the original instruction, 8 per iteration.
static uint64_t loop_mrs_tpidr(uint64_t n) {
  uint64_t acc = 0, t;
  for (uint64_t i = 0; i < n; i++) {
    __asm__ volatile(
        "mrs %0, tpidr_el0\n add %1, %1, %0\n mrs %0, tpidr_el0\n add %1, %1, %0\n"
        "mrs %0, tpidr_el0\n add %1, %1, %0\n mrs %0, tpidr_el0\n add %1, %1, %0\n"
        "mrs %0, tpidr_el0\n add %1, %1, %0\n mrs %0, tpidr_el0\n add %1, %1, %0\n"
        "mrs %0, tpidr_el0\n add %1, %1, %0\n mrs %0, tpidr_el0\n add %1, %1, %0\n"
        : "=&r"(t), "+r"(acc));
  }
  return acc;
}

// (b) inline replacement: mrs tpidrro + ldr slot.
static uint64_t loop_tsd_inline(uint64_t n, uint64_t off) {
  uint64_t acc = 0, t;
  for (uint64_t i = 0; i < n; i++) {
#define R "mrs %0, tpidrro_el0\n ldr %0, [%0, %2]\n add %1, %1, %0\n"
    __asm__ volatile(R R R R R R R R : "=&r"(t), "+r"(acc) : "r"(off));
#undef R
  }
  return acc;
}

// (c) what a rewriter actually emits: branch out to a thunk and back.
// The slot offset is patched into the thunk as an immediate; here we use a
// register-offset load with x9 preloaded, which has the same latency.
extern uint64_t loop_tsd_thunk(uint64_t n, uint64_t off);
__asm__(
    ".text\n .p2align 4\n"
    ".globl _loop_tsd_thunk\n_loop_tsd_thunk:\n"
    "  mov x9, x1\n  mov x2, #0\n"
    "1:\n"
    "  b 10f\n11: add x2, x2, x3\n"
    "  b 20f\n21: add x2, x2, x3\n"
    "  b 30f\n31: add x2, x2, x3\n"
    "  b 40f\n41: add x2, x2, x3\n"
    "  b 50f\n51: add x2, x2, x3\n"
    "  b 60f\n61: add x2, x2, x3\n"
    "  b 70f\n71: add x2, x2, x3\n"
    "  b 80f\n81: add x2, x2, x3\n"
    "  subs x0, x0, #1\n  b.ne 1b\n  mov x0, x2\n  ret\n"
    // thunks (placed out of line, as a rewriter would in a thunk page)
    "10: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 11b\n"
    "20: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 21b\n"
    "30: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 31b\n"
    "40: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 41b\n"
    "50: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 51b\n"
    "60: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 61b\n"
    "70: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 71b\n"
    "80: mrs x3, tpidrro_el0\n ldr x3, [x3, x9]\n b 81b\n");

static uint64_t read_guest_tp(uint64_t off) {
  uint64_t v;
  __asm__ volatile("mrs %0, tpidrro_el0\n ldr %0, [%0, %1]" : "=&r"(v) : "r"(off));
  return v;
}

static void *correctness(void *arg) {
  uint64_t off = (uint64_t)guest_tp_key * 8;
  uint64_t want = (uint64_t)(uintptr_t)arg;
  pthread_setspecific(guest_tp_key, (void *)(uintptr_t)want);  // the msr replacement
  uint64_t bad = 0;
  for (int i = 0; i < 20000; i++) {
    if (i % 4 == 0) usleep(10);  // force switches: this is what broke TPIDR_EL0
    if (read_guest_tp(off) != want) bad++;
  }
  return (void *)(uintptr_t)bad;
}

int main(void) {
  pthread_key_create(&guest_tp_key, NULL);
  uint64_t off = (uint64_t)guest_tp_key * 8;
  printf("guest TP slot: pthread key %lu -> [TPIDRRO_EL0 + %" PRIu64 "]\n", (unsigned long)guest_tp_key, off);

  enum { N = 32 };
  pthread_t t[N];
  for (int i = 0; i < N; i++)
    pthread_create(&t[i], NULL, correctness, (void *)(uintptr_t)(0x00007f0000000ab8ull + ((uint64_t)i << 20)));
  uint64_t bad = 0;
  for (int i = 0; i < N; i++) {
    void *r;
    pthread_join(t[i], &r);
    bad += (uint64_t)(uintptr_t)r;
  }
  printf("TSD-slot guest TP across usleep switches: %d threads x 20000 reads, mismatches=%" PRIu64 "\n", N, bad);

  pthread_setspecific(guest_tp_key, (void *)0x1234);
  uint64_t n = ITERS / 8;
  double reads = (double)n * 8;
  uint64_t t0 = now_ns(); volatile uint64_t s1 = loop_mrs_tpidr(n); uint64_t t1 = now_ns();
  volatile uint64_t s2 = loop_tsd_inline(n, off); uint64_t t2 = now_ns();
  volatile uint64_t s3 = loop_tsd_thunk(n, off); uint64_t t3 = now_ns();
  (void)s1; (void)s2; (void)s3;
  printf("per read (dependent add, %.0fM reads):\n", reads / 1e6);
  printf("  mrs tpidr_el0                      %.3f ns\n", (t1 - t0) / reads);
  printf("  mrs tpidrro_el0 + ldr (inline)     %.3f ns\n", (t2 - t1) / reads);
  printf("  b thunk; mrs tpidrro; ldr; b back  %.3f ns\n", (t3 - t2) / reads);
  return bad ? 1 : 0;
}
