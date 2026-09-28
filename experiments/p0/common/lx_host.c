// Per-thread host binding for the P0 syscall-layer prototype.
#include "lx.h"

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>

uint64_t lx_host_slot_off;
uint64_t lx_guest_tp_off;
static pthread_key_t host_key, guest_tp_key;

void lx_init_process(void) {
  if (pthread_key_create(&host_key, NULL) || pthread_key_create(&guest_tp_key, NULL)) abort();
  // pthread_getspecific(k) is tsd[k] with tsd = TPIDRRO_EL0 on Darwin arm64.
  lx_host_slot_off = (uint64_t)host_key * 8;
  lx_guest_tp_off = (uint64_t)guest_tp_key * 8;
}

void lx_bind_thread(struct lx_thread *t, uint64_t host_stack_top) {
  t->host_sp = host_stack_top & ~15ull;
  pthread_setspecific(host_key, t);
  pthread_setspecific(guest_tp_key, (void *)t->guest_tp);
}

struct lx_thread *lx_self(void) { return pthread_getspecific(host_key); }

void lx_set_guest_tp(uint64_t tp) {
  struct lx_thread *t = lx_self();
  if (t) t->guest_tp = tp;
  pthread_setspecific(guest_tp_key, (void *)tp);
}
