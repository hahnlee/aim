#include "darwin_art_bionic_allocator.h"
#include <errno.h>
#include <malloc/malloc.h>

extern void darwin_art_bionic_errno_store(int32_t android_errno);

/* Android16 bionic libc/include/malloc.h: the purge value is ignored.
 * Host blocks are allocated by Darwin malloc, so reclaim through its zones,
 * not through an unrelated guest allocator or by freeing live blocks. */
int darwin_art_bionic_mallopt(int param, int value) {
  (void)value;
  if (param != -101 && param != -104) return 0;
  const int saved_errno = errno;
  (void)malloc_zone_pressure_relief(NULL, 0);
  errno = saved_errno;
  return 1;
}

bool darwin_art_bionic_android_mallopt(int opcode, void* arg, size_t arg_size) {
  /* M_INIT_ZYGOTE_CHILD_PROFILING is optional native heap instrumentation.
   * There is no heapprofd owner for the Darwin allocator. Fail explicitly;
   * the original ActivityThread JNI intentionally ignores this result.
   * Do not advertise a functioning Android profiling hook or mutate an
   * unrelated host profiler. Invalid input retains Bionic's EINVAL contract. */
  if (opcode == 1 && (arg != NULL || arg_size != 0)) {
    darwin_art_bionic_errno_store(DARWIN_ART_BIONIC_EINVAL);
    return false;
  }
  darwin_art_bionic_errno_store(95); /* Android ENOTSUP, not Darwin's value. */
  return false;
}
