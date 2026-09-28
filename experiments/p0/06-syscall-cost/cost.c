// P0 experiment 6: redirected syscall cost. See cost.S for the paths.
#include "lx.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/ucontext.h>
#include <time.h>
#include <unistd.h>

void loop_call(uint64_t n);
void loop_darwin_getpid(uint64_t n);
void loop_darwin_getppid(uint64_t n);
void loop_thunk_only(uint64_t n);
void loop_full(uint64_t n);
void loop_int(uint64_t n);
void loop_min(uint64_t n);
void loop_sigsys(uint64_t n);

void lx_dispatch(struct lx_thread *t) {
  struct lx_frame *f = &t->f;
  f->x[0] = f->x[8] == 178 ? (uint64_t)t->tid : (uint64_t)-38;
}

static void on_sigsys(int s, siginfo_t *si, void *ucv) {
  (void)s; (void)si;
  ucontext_t *uc = ucv;
  uc->uc_mcontext->__ss.__x[0] = 1234;  // "gettid"; pc already points past the svc
}

static inline uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

static double best_of(void (*fn)(uint64_t), uint64_t n) {
  double best = 1e30;
  for (int r = 0; r < 5; r++) {
    uint64_t t0 = now_ns();
    fn(n);
    double ns = (double)(now_ns() - t0) / (double)n;
    if (ns < best) best = ns;
  }
  return best;
}

int main(void) {
  lx_init_process();
  static struct lx_thread t;
  t.tid = 777;
  void *hs = malloc(256 * 1024);
  lx_bind_thread(&t, (uint64_t)hs + 256 * 1024);
  struct sigaction sa = {0};
  sa.sa_sigaction = on_sigsys;
  sa.sa_flags = SA_SIGINFO;
  sigaction(SIGSYS, &sa, NULL);

  const uint64_t N = 20000000;
  double call = best_of(loop_call, N);
  double getpid_ = best_of(loop_darwin_getpid, N / 4);
  double getppid_ = best_of(loop_darwin_getppid, N / 4);
  double thunk = best_of(loop_thunk_only, N);
  double full = best_of(loop_full, N);
  double integer = best_of(loop_int, N);
  double min = best_of(loop_min, N);
  double sigsys = best_of(loop_sigsys, 200000);
  printf("per call, best of 5 runs (ns):\n");
  printf("  bl/ret to an empty function                          %7.2f\n", call);
  printf("  Darwin getpid, raw svc #0x80                         %7.2f\n", getpid_);
  printf("  Darwin getppid, raw svc #0x80                        %7.2f\n", getppid_);
  printf("  svc thunk only (entry returns at once)               %7.2f\n", thunk);
  printf("  full: x0-x30 + q0-q31 + nzcv/fpsr/fpcr, host stack   %7.2f\n", full);
  printf("  lean: x0-x30 + nzcv, no SIMD, host stack             %7.2f\n", integer);
  printf("  minimal: AAPCS caller-saved x0-x15,x18 + nzcv        %7.2f\n", min);
  printf("  trap: svc #0 -> Darwin SIGSYS -> handler             %7.2f\n", sigsys);
  printf("full redirection + a pass-through Darwin syscall ~= %.0f ns (%.1fx a native getppid)\n", full + getppid_,
         (full + getppid_) / getppid_);
  return 0;
}
