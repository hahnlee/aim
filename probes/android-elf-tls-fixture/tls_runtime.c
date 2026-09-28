#include <pthread.h>
#include <stdint.h>

static __thread int tls_initialized = 7;
static __thread int tls_zero;
static __thread unsigned char tls_aligned[64] __attribute__((aligned(64)));

static void* Worker(void* argument) {
  const intptr_t index = (intptr_t)argument;
  if (tls_initialized != 7 || tls_zero != 0 ||
      ((uintptr_t)&tls_aligned[0] & 63u) != 0) {
    return (void*)(intptr_t)-1;
  }
  tls_initialized = 100 + (int)index;
  tls_zero = 200 + (int)index;
  tls_aligned[index] = (unsigned char)(30 + index);
  if (tls_initialized != 100 + (int)index ||
      tls_zero != 200 + (int)index ||
      tls_aligned[index] != (unsigned char)(30 + index)) {
    return (void*)(intptr_t)-2;
  }
  return (void*)(index + 1);
}

__attribute__((visibility("default"))) int JNI_OnLoad(void* vm,
                                                        void* reserved) {
  (void)vm;
  (void)reserved;
  if (tls_initialized != 7 || tls_zero != 0 ||
      ((uintptr_t)&tls_aligned[0] & 63u) != 0) {
    return -1;
  }
  tls_initialized = 71;
  tls_zero = 17;
  tls_aligned[0] = 9;

  pthread_t threads[4];
  int started = 0;
  for (; started < 4; ++started) {
    if (pthread_create(&threads[started], 0, Worker,
                       (void*)(intptr_t)started) != 0) {
      break;
    }
  }
  int accepted = started == 4;
  for (int index = 0; index < started; ++index) {
    void* result = 0;
    if (pthread_join(threads[index], &result) != 0 ||
        result != (void*)(intptr_t)(index + 1)) {
      accepted = 0;
    }
  }
  if (!accepted || tls_initialized != 71 || tls_zero != 17 ||
      tls_aligned[0] != 9) {
    return -1;
  }
  return 0x00010006;
}
