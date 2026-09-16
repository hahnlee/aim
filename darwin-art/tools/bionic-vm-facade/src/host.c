#include "darwin_art_bionic_vm.h"

#include <errno.h>
#include <libkern/OSCacheControl.h>
#include <sys/mman.h>
#include <unistd.h>

_Static_assert(PROT_NONE == 0 && PROT_READ == 1 && PROT_WRITE == 2 &&
                   PROT_EXEC == 4,
               "Darwin protection constants drift");
_Static_assert(MAP_PRIVATE == 2 && MAP_FIXED == 0x10 && MAP_ANON == 0x1000,
               "Darwin mapping constants drift");
_Static_assert(MAP_NORESERVE == 0x40, "Darwin MAP_NORESERVE drift");
_Static_assert(MADV_NORMAL == 0 && MADV_RANDOM == 1 && MADV_SEQUENTIAL == 2 &&
                   MADV_WILLNEED == 3 && MADV_DONTNEED == 4 &&
                   MADV_FREE == 5 && MADV_ZERO == 11,
               "Darwin advice constants drift");

size_t darwin_art_host_vm_page_size(void) {
  const int value = getpagesize();
  return value > 0 ? (size_t)value : 0;
}

void* darwin_art_host_vm_map(void* address, size_t length, int protection,
                             int android_flags, int fd, int64_t offset,
                             int* host_error) {
  const int saved = errno;
  errno = 0;
  /* Without MAP_FIXED, both Linux and Darwin interpret a non-null address as
   * a placement hint. PartitionAlloc relies on this for its second cage. */
  int flags = (android_flags & 1) != 0 ? MAP_SHARED : MAP_PRIVATE;
  const int anonymous = (android_flags & 0x20) != 0;
  if (anonymous) flags |= MAP_ANON;
  if ((android_flags & 0x10) != 0) flags |= MAP_FIXED;
  /* Linux accepts MAP_NORESERVE for both anonymous and file-backed Binder
   * arenas. Darwin rejects MAP_NORESERVE on a POSIX shared-memory fd with
   * EINVAL, and has no equivalent reservation behavior for that mapping.
   * Preserve the hint only where Darwin implements it. */
  if (anonymous && (android_flags & 0x4000) != 0) flags |= MAP_NORESERVE;
  void* result = mmap(address, length, protection, flags, fd, offset);
  *host_error = result == MAP_FAILED ? errno : 0;
  errno = saved;
  return result;
}

int darwin_art_host_vm_unmap(void* address, size_t length, int* host_error) {
  const int saved = errno;
  errno = 0;
  const int result = munmap(address, length);
  *host_error = result == -1 ? errno : 0;
  errno = saved;
  return result;
}

int darwin_art_host_vm_protect(void* address, size_t length, int protection,
                               int* host_error) {
  const int saved = errno;
  errno = 0;
  const int result = mprotect(address, length, protection);
  *host_error = result == -1 ? errno : 0;
  errno = saved;
  return result;
}

int darwin_art_host_vm_advise(void* address, size_t length, int advice,
                              int* host_error) {
  const int saved = errno;
  errno = 0;
  const int result = madvise(address, length, advice);
  *host_error = result == -1 ? errno : 0;
  errno = saved;
  return result;
}

void* darwin_art_host_vm_remap_zero(void* address, size_t length,
                                    int protection, int* host_error) {
  const int saved = errno;
  errno = 0;
  void* result = mmap(address, length, protection,
                      MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
  *host_error = result == MAP_FAILED ? errno : 0;
  errno = saved;
  return result;
}

void darwin_art_host_vm_invalidate_icache(void* address, size_t length) {
  sys_icache_invalidate(address, length);
}

int darwin_art_host_vm_close_fd(int fd, int* host_error) {
  const int saved = errno;
  errno = 0;
  const int result = close(fd);
  *host_error = result == -1 ? errno : 0;
  errno = saved;
  return result;
}
