#define _GNU_SOURCE 1
#include <errno.h>
#include <stdint.h>
#include <sys/mman.h>

static void* gRaceMapping;

__attribute__((visibility("default"))) int bionic_vm_fixture_basic(void) {
  const size_t length = 4096;
  void* mapping = mmap(NULL, length, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (mapping == MAP_FAILED) return 1;
  if ((uintptr_t)mapping % 4096 != 0 ||
      ((volatile unsigned char*)mapping)[0] != 0)
    return 23;

  volatile uint32_t* instructions = (volatile uint32_t*)mapping;
  instructions[0] = 0x52800540u; /* mov w0, #42 */
  instructions[1] = 0xd65f03c0u; /* ret */
  if (madvise(mapping, length, MADV_NORMAL) != 0) return 2;
  if (mprotect(mapping, length, PROT_READ | PROT_EXEC) != 0) return 3;

  union {
    void* object;
    int (*function)(void);
  } code = {.object = mapping};
  if (code.function() != 42) return 4;

  errno = 0;
  if (mprotect(mapping, length, PROT_READ | PROT_WRITE | PROT_EXEC) != -1 ||
      errno != EACCES)
    return 5;
  if (mprotect(mapping, length, PROT_READ | PROT_WRITE) != 0) return 6;
  ((volatile unsigned char*)mapping)[0] = 0xaa;
  if (munmap(mapping, length) != 0) return 7;

  mapping = mmap64(NULL, length, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS,
                   -1, 0);
  if (mapping == MAP_FAILED || munmap(mapping, length) != 0) return 8;

  mapping = mmap(NULL, 16 * 1024 * 1024, PROT_NONE,
                 MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  if (mapping == MAP_FAILED || munmap(mapping, 16 * 1024 * 1024) != 0)
    return 47;

  mapping = mmap(NULL, 256 * 1024, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK | MAP_GROWSDOWN, -1,
                 0);
  if (mapping == MAP_FAILED) return 52;
  ((volatile unsigned char*)mapping)[0] = 0x31;
  ((volatile unsigned char*)mapping)[256 * 1024 - 1] = 0x32;
  if (((volatile unsigned char*)mapping)[0] != 0x31 ||
      ((volatile unsigned char*)mapping)[256 * 1024 - 1] != 0x32 ||
      munmap(mapping, 256 * 1024) != 0)
    return 53;

  mapping = mmap(NULL, length, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (mapping == MAP_FAILED) return 37;
  ((volatile unsigned char*)mapping)[0] = 0x5a;
  mapping = mremap(mapping, length, length * 2, MREMAP_MAYMOVE);
  if (mapping == MAP_FAILED || ((volatile unsigned char*)mapping)[0] != 0x5a)
    return 38;
  if (munmap(mapping, length * 2) != 0) return 39;

  errno = 0;
  if (mmap(NULL, 0, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) !=
          MAP_FAILED ||
      errno != EINVAL)
    return 9;
  errno = 0;
  if (mmap(NULL, SIZE_MAX, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) !=
          MAP_FAILED ||
      errno != EOVERFLOW)
    return 10;
  mapping = mmap(NULL, length, PROT_READ | PROT_WRITE,
                 MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  if (mapping == MAP_FAILED) return 11;
  ((volatile unsigned char*)mapping)[0] = 0x6d;
  if (((volatile unsigned char*)mapping)[0] != 0x6d ||
      munmap(mapping, length) != 0)
    return 41;
  errno = 0;
  if (mmap((void*)4096, length, PROT_READ,
           MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) != MAP_FAILED ||
      errno != ENOMEM)
    return 12;
  errno = 0;
  if (mmap(NULL, length, PROT_READ, MAP_PRIVATE, -1, 0) != MAP_FAILED ||
      errno != EBADF)
    return 13;
  errno = 0;
  if (mmap(NULL, length, PROT_READ | PROT_WRITE | PROT_EXEC,
           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) != MAP_FAILED ||
      errno != EACCES)
    return 25;
  errno = 0;
  if (mmap(NULL, length, 0x8, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) !=
          MAP_FAILED ||
      errno != EINVAL)
    return 26;
  errno = 0;
  if (mmap64(NULL, length, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1,
             16384) != MAP_FAILED ||
      errno != EOPNOTSUPP)
    return 14;
  errno = 0;
  if (mmap64(NULL, length, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 1) !=
          MAP_FAILED ||
      errno != EINVAL)
    return 21;

  mapping = mmap(NULL, length, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, 12345, 0);
  if (mapping == MAP_FAILED) return 15;
  errno = 0;
  if (mprotect(mapping, 0, PROT_READ) != -1 || errno != EINVAL) return 27;
  errno = 0;
  if (madvise(mapping, 0, MADV_NORMAL) != -1 || errno != EINVAL) return 28;
  errno = 0;
  if (munmap(mapping, 0) != -1 || errno != EINVAL) return 29;
  errno = 0;
  if (mprotect((unsigned char*)mapping + 1, length, PROT_READ) != -1 ||
      errno != EINVAL)
    return 16;
  errno = 0;
  if (mprotect((void*)16384, 16384, PROT_READ) != -1 || errno != ENOMEM)
    return 35;
  errno = 0;
  if (madvise((void*)16384, 16384, MADV_NORMAL) != -1 || errno != ENOMEM)
    return 36;
  ((volatile unsigned char*)mapping)[0] = 0x7b;
  if (madvise(mapping, length, MADV_DONTNEED) != 0 ||
      ((volatile unsigned char*)mapping)[0] != 0)
    return 18;
  if (madvise(mapping, length, MADV_FREE) != 0) return 22;
  errno = 0;
  if (madvise(mapping, length, 999) != -1 || errno != EINVAL) return 19;
  if (munmap(mapping, length) != 0) return 20;

  mapping = mmap(NULL, 32768, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (mapping == MAP_FAILED) return 47;
  ((volatile unsigned char*)mapping)[0] = 0x5a;
  ((volatile unsigned char*)mapping)[16384] = 0x6b;
  if (madvise((unsigned char*)mapping + 16384, 16384, MADV_DONTNEED) != 0)
    return 48;
  if (((volatile unsigned char*)mapping)[0] != 0x5a ||
      ((volatile unsigned char*)mapping)[16384] != 0)
    return 49;
  if (mprotect((unsigned char*)mapping + 16384, 16384, PROT_NONE) != 0 ||
      madvise((unsigned char*)mapping + 16384, 16384, MADV_DONTNEED) != 0 ||
      mprotect((unsigned char*)mapping + 16384, 16384,
               PROT_READ | PROT_WRITE) != 0 ||
      ((volatile unsigned char*)mapping)[16384] != 0)
    return 50;
  if (munmap(mapping, 32768) != 0) return 51;

  mapping = mmap(NULL, length, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (mapping == MAP_FAILED) return 43;
  ((volatile unsigned char*)mapping)[0] = 0x7e;
  void* fixed = mmap(mapping, length, PROT_NONE,
                     MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
  if (fixed != mapping) return 44;
  if (mprotect(fixed, length, PROT_READ | PROT_WRITE) != 0 ||
      ((volatile unsigned char*)fixed)[0] != 0)
    return 45;
  if (munmap(fixed, length) != 0) return 46;

  const size_t host_page = 16384;
  mapping = mmap(NULL, host_page * 2, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (mapping == MAP_FAILED) return 17;
  unsigned char residency[2] = {0, 0};
  if (mincore(mapping, host_page + 1, residency) != 0 ||
      residency[0] != 1 || residency[1] != 1)
    return 55;
  void* second_page = (unsigned char*)mapping + host_page;
  if (mprotect(second_page, host_page, PROT_READ) != 0 ||
      mprotect(second_page, host_page, PROT_READ | PROT_WRITE) != 0)
    return 34;
  if (munmap(mapping, host_page) != 0 || munmap(second_page, host_page) != 0)
    return 40;
  return 42;
}

__attribute__((visibility("default"))) int bionic_vm_fixture_race_setup(void) {
  gRaceMapping = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  return gRaceMapping == MAP_FAILED ? 30 : 42;
}

__attribute__((visibility("default"))) int bionic_vm_fixture_race_protect(void) {
  if (mprotect(gRaceMapping, 4096, PROT_READ) == -1) {
    return errno == ENOMEM ? 42 : 31;
  }
  if (mprotect(gRaceMapping, 4096, PROT_READ | PROT_WRITE) == -1) {
    return errno == ENOMEM ? 42 : 32;
  }
  return 42;
}

__attribute__((visibility("default"))) int bionic_vm_fixture_race_unmap(void) {
  return munmap(gRaceMapping, 4096) == 0 ? 42 : 33;
}
