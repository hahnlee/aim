#include "darwin_art_bionic_allocator.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition)                                                        \
  do {                                                                          \
    if (!(condition)) {                                                         \
      fprintf(stderr, "allocator differential failed at line %d: %s\n",       \
              __LINE__, #condition);                                            \
      return 1;                                                                 \
    }                                                                           \
  } while (0)

static int32_t g_bionic_errno;

void darwin_art_bionic_errno_store(int32_t android_errno) {
  g_bionic_errno = android_errno;
}

extern char* darwin_art_bionic_tempnam_unsupported(const char*, const char*);

static int CheckUnsupportedTempnam(void) {
  g_bionic_errno = 0;
  CHECK(darwin_art_bionic_tempnam_unsupported("/tmp", "audit") == NULL);
  CHECK(g_bionic_errno == 38);
  return 0;
}

static int CheckHostErrnoIsolation(void) {
  volatile size_t impossible = SIZE_MAX;

  errno = 31337;
  DarwinArtBionicAllocationResult malloc_failure =
      darwin_art_bionic_malloc_result(impossible);
  CHECK(malloc_failure.pointer == NULL);
  CHECK(malloc_failure.bionic_errno == DARWIN_ART_BIONIC_ENOMEM);
  CHECK(errno == 31337);

  unsigned char* original = darwin_art_bionic_malloc(64);
  CHECK(original != NULL);
  memset(original, 0xa7, 64);
  errno = 31338;
  DarwinArtBionicAllocationResult realloc_failure =
      darwin_art_bionic_realloc_result(original, impossible);
  CHECK(realloc_failure.pointer == NULL);
  CHECK(realloc_failure.bionic_errno == DARWIN_ART_BIONIC_ENOMEM);
  CHECK(errno == 31338);
  for (size_t index = 0; index < 64; ++index) CHECK(original[index] == 0xa7);
  darwin_art_bionic_free(original);
  CHECK(errno == 31338);
  return 0;
}

static int CheckSizeZeroAndRealloc(void) {
  DarwinArtBionicAllocationResult zero = darwin_art_bionic_malloc_result(0);
  CHECK(zero.pointer != NULL);
  CHECK(zero.bionic_errno == 0);
  darwin_art_bionic_free(zero.pointer);

  DarwinArtBionicAllocationResult null_zero =
      darwin_art_bionic_realloc_result(NULL, 0);
  CHECK(null_zero.pointer != NULL);
  CHECK(null_zero.bionic_errno == 0);
  darwin_art_bionic_free(null_zero.pointer);

  unsigned char* block = darwin_art_bionic_malloc(32);
  CHECK(block != NULL);
  memset(block, 0x5c, 32);
  DarwinArtBionicAllocationResult grown =
      darwin_art_bionic_realloc_result(block, 128);
  CHECK(grown.pointer != NULL);
  CHECK(grown.bionic_errno == 0);
  for (size_t index = 0; index < 32; ++index)
    CHECK(((unsigned char*)grown.pointer)[index] == 0x5c);

  DarwinArtBionicAllocationResult freed =
      darwin_art_bionic_realloc_result(grown.pointer, 0);
  CHECK(freed.pointer == NULL);
  CHECK(freed.bionic_errno == 0);
  return 0;
}

static int CheckReallocarrayOverflow(void) {
  unsigned char* original = darwin_art_bionic_malloc(64);
  CHECK(original != NULL);
  memset(original, 0x6d, 64);
  errno = 22001;
  g_bionic_errno = 0;
  void* result = darwin_art_bionic_reallocarray(original, SIZE_MAX, 2);
  CHECK(result == NULL);
  CHECK(g_bionic_errno == DARWIN_ART_BIONIC_ENOMEM);
  CHECK(errno == 22001);
  for (size_t index = 0; index < 64; ++index) CHECK(original[index] == 0x6d);
  darwin_art_bionic_free(original);

  unsigned char* small = darwin_art_bionic_malloc(8);
  CHECK(small != NULL);
  memset(small, 0x3a, 8);
  g_bionic_errno = 0;
  unsigned char* grown = darwin_art_bionic_reallocarray(small, 4, 8);
  CHECK(grown != NULL);
  CHECK(g_bionic_errno == 0);
  for (size_t index = 0; index < 8; ++index) CHECK(grown[index] == 0x3a);
  darwin_art_bionic_free(grown);
  return 0;
}

static int CheckPosixMemalign(void) {
  const size_t invalid[] = {0, 1, 2, 3, 4, 6, 7, 12, 24};
  for (size_t index = 0; index < sizeof(invalid) / sizeof(invalid[0]); ++index) {
    void* sentinel = (void*)(uintptr_t)0x1234;
    errno = 20000 + (int)index;
    CHECK(darwin_art_bionic_posix_memalign(&sentinel, invalid[index], 17) ==
          DARWIN_ART_BIONIC_EINVAL);
    CHECK(sentinel == (void*)(uintptr_t)0x1234);
    CHECK(errno == 20000 + (int)index);
  }

  const size_t valid[] = {sizeof(void*), 16, 32, 64, 256, 4096};
  for (size_t index = 0; index < sizeof(valid) / sizeof(valid[0]); ++index) {
    for (size_t size_index = 0; size_index < 2; ++size_index) {
      const size_t size = size_index == 0 ? 0 : 113;
      void* pointer = NULL;
      errno = 21000 + (int)index;
      CHECK(darwin_art_bionic_posix_memalign(&pointer, valid[index], size) == 0);
      CHECK(pointer != NULL);
      CHECK((uintptr_t)pointer % valid[index] == 0);
      CHECK(errno == 21000 + (int)index);
      darwin_art_bionic_free(pointer);
    }
  }

  volatile size_t impossible = SIZE_MAX;
  void* unchanged = (void*)(uintptr_t)0x5678;
  errno = 21999;
  CHECK(darwin_art_bionic_posix_memalign(&unchanged, 64, impossible) ==
        DARWIN_ART_BIONIC_ENOMEM);
  CHECK(unchanged == (void*)(uintptr_t)0x5678);
  CHECK(errno == 21999);

  CHECK(darwin_art_bionic_posix_memalign(NULL, 16, 16) ==
        DARWIN_ART_BIONIC_EINVAL);
  return 0;
}

static int CheckAlignedAlloc(void) {
  CHECK(darwin_art_bionic_aligned_alloc(3, 12) == NULL);
  CHECK(darwin_art_bionic_aligned_alloc(16, 17) == NULL);
  for (size_t alignment = 8; alignment <= 4096; alignment *= 2) {
    void* pointer = darwin_art_bionic_aligned_alloc(alignment, alignment * 2);
    CHECK(pointer != NULL);
    CHECK((uintptr_t)pointer % alignment == 0);
    darwin_art_bionic_free(pointer);
  }
  void* zero = darwin_art_bionic_aligned_alloc(64, 0);
  CHECK(zero != NULL);
  CHECK((uintptr_t)zero % 64 == 0);
  darwin_art_bionic_free(zero);
  return 0;
}

static int CheckResolver(void) {
  typedef void* (*MallocFunction)(size_t);
  typedef void (*FreeFunction)(void*);
  typedef void* (*ReallocFunction)(void*, size_t);
  typedef int (*PosixMemalignFunction)(void**, size_t, size_t);
  size_t count = 0;
  const DarwinArtBionicAllocatorBinding* table =
      darwin_art_bionic_allocator_table(&count);
  CHECK(table != NULL);
  const char* expected[] = {
      "aligned_alloc",      "calloc",          "free",    "mallinfo",
      "malloc",             "malloc_usable_size", "mallopt", "memalign",
      "posix_memalign",     "realloc", "reallocarray", "strdup", "strndup",
      "tempnam",
  };
  CHECK(count == sizeof(expected) / sizeof(expected[0]));
  for (size_t index = 0; index < count; ++index) {
    CHECK(strcmp(table[index].import_name, expected[index]) == 0);
    CHECK(darwin_art_bionic_allocator_resolve(expected[index]) ==
          table[index].address);
  }
  CHECK(darwin_art_bionic_allocator_resolve(NULL) == NULL);
  CHECK(darwin_art_bionic_allocator_resolve("malloc_size") == NULL);

  MallocFunction resolved_malloc = (MallocFunction)table[4].address;
  FreeFunction resolved_free = (FreeFunction)table[2].address;
  PosixMemalignFunction resolved_posix_memalign =
      (PosixMemalignFunction)table[8].address;
  ReallocFunction resolved_realloc = (ReallocFunction)table[9].address;
  void* direct = resolved_malloc(29);
  CHECK(direct != NULL);
  direct = resolved_realloc(direct, 57);
  CHECK(direct != NULL);
  resolved_free(direct);
  void* aligned = NULL;
  CHECK(resolved_posix_memalign(&aligned, 64, 19) == 0);
  CHECK((uintptr_t)aligned % 64 == 0);
  resolved_free(aligned);

  CHECK((table[2].capabilities & DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE) != 0);
  CHECK((table[4].capabilities &
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM) != 0);
  CHECK((table[8].capabilities & DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE) != 0);
  CHECK((table[9].capabilities &
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM) != 0);
  CHECK((table[10].capabilities &
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM) != 0);
  for (size_t index = 0; index < count; ++index) {
    CHECK((table[index].capabilities &
           DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI) != 0);
    if (strcmp(table[index].import_name, "mallinfo") != 0 &&
        strcmp(table[index].import_name, "malloc_usable_size") != 0 &&
        strcmp(table[index].import_name, "mallopt") != 0 &&
        strcmp(table[index].import_name, "tempnam") != 0) {
      CHECK((table[index].capabilities &
             DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK) != 0);
    }
  }
  return 0;
}

int main(void) {
  unsigned char* live = darwin_art_bionic_malloc(4096);
  CHECK(live != NULL);
  memset(live, 0x5a, 4096);
  errno = EDOM;
  g_bionic_errno = 7;
  CHECK(darwin_art_bionic_mallopt(-101, 123) == 1);
  CHECK(darwin_art_bionic_mallopt(-104, 0) == 1);
  CHECK(errno == EDOM && g_bionic_errno == 7);
  for (size_t i = 0; i < 4096; ++i) CHECK(live[i] == 0x5a);
  CHECK(darwin_art_bionic_mallopt(123456, 0) == 0);
  CHECK(!darwin_art_bionic_android_mallopt(1, live, 0));
  CHECK(g_bionic_errno == 22);
  CHECK(!darwin_art_bionic_android_mallopt(1, NULL, 0));
  CHECK(g_bionic_errno == 95);
  CHECK(!darwin_art_bionic_android_mallopt(123456, NULL, 0));
  CHECK(g_bionic_errno == 95);
  darwin_art_bionic_free(live);
  CHECK(CheckSizeZeroAndRealloc() == 0);
  CHECK(CheckReallocarrayOverflow() == 0);
  CHECK(CheckPosixMemalign() == 0);
  CHECK(CheckAlignedAlloc() == 0);
  CHECK(CheckHostErrnoIsolation() == 0);
  CHECK(CheckUnsupportedTempnam() == 0);
  CHECK(CheckResolver() == 0);
  puts("allocator differential: PASS");
  return 0;
}
