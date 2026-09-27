// P0 experiment 5b: which EL0 system instructions that Linux arm64 guests
// execute directly are allowed by XNU? Each runs in a forked child.
// Linux lets EL0 read CTR_EL0/DCZID_EL0/CNTVCT_EL0 and do dc cvau/civac,
// ic ivau, dc zva; it also emulates MIDR_EL1 and friends on trap.
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

static char buf[256] __attribute__((aligned(128)));

#define PROBE(name, body)                                                    \
  do {                                                                       \
    pid_t p = fork();                                                        \
    if (p == 0) {                                                            \
      uint64_t v = 0;                                                        \
      (void)v;                                                               \
      body;                                                                  \
      printf("  %-40s ok    value=%#llx\n", name, (unsigned long long)v);    \
      _exit(0);                                                              \
    }                                                                        \
    int st;                                                                  \
    waitpid(p, &st, 0);                                                      \
    if (WIFSIGNALED(st)) printf("  %-40s TRAP  (signal %d)\n", name, WTERMSIG(st)); \
  } while (0)

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  uintptr_t a = (uintptr_t)buf;
  PROBE("mrs ctr_el0", __asm__ volatile("mrs %0, ctr_el0" : "=r"(v)));
  PROBE("mrs dczid_el0", __asm__ volatile("mrs %0, dczid_el0" : "=r"(v)));
  PROBE("mrs cntvct_el0", __asm__ volatile("mrs %0, cntvct_el0" : "=r"(v)));
  PROBE("mrs cntfrq_el0", __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(v)));
  PROBE("mrs midr_el1 (Linux emulates)", __asm__ volatile("mrs %0, midr_el1" : "=r"(v)));
  PROBE("mrs id_aa64isar0_el1 (Linux emulates)", __asm__ volatile("mrs %0, id_aa64isar0_el1" : "=r"(v)));
  PROBE("mrs tpidrro_el0", __asm__ volatile("mrs %0, tpidrro_el0" : "=r"(v)));
  PROBE("dc cvau", __asm__ volatile("dc cvau, %0" ::"r"(a) : "memory"));
  PROBE("dc civac", __asm__ volatile("dc civac, %0" ::"r"(a) : "memory"));
  PROBE("dc cvac", __asm__ volatile("dc cvac, %0" ::"r"(a) : "memory"));
  PROBE("ic ivau", __asm__ volatile("ic ivau, %0" ::"r"(a) : "memory"));
  PROBE("dc zva (bionic memset)", __asm__ volatile("dc zva, %0" ::"r"(a) : "memory"));
  PROBE("isb; dsb ish", __asm__ volatile("isb\n dsb ish" ::: "memory"));
  return 0;
}
