#include <pthread.h>
#include <sched.h>
#include <stdbool.h>
#include <stdio.h>

extern bool aim_x18_yield_probe(unsigned long iterations);

static volatile bool keep_running = true;

static void* competing_thread(void* unused) {
  (void)unused;
  while (keep_running) sched_yield();
  return NULL;
}

int main(void) {
  pthread_t worker;
  if (pthread_create(&worker, NULL, competing_thread, NULL) != 0) return 2;
  const bool preserved = aim_x18_yield_probe(100000);
  keep_running = false;
  if (pthread_join(worker, NULL) != 0) return 3;
  printf("darwin-x18-abi: preserved=%d\n", preserved ? 1 : 0);
  return preserved ? 0 : 1;
}
