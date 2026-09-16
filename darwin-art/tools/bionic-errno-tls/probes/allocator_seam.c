#include "darwin_art_bionic_allocator.h"
#include "darwin_art_bionic_errno.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>

#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)

int main(void) {
  errno = 28001;
  darwin_art_bionic_errno_store(71);
  DarwinArtBionicAllocationResult success = darwin_art_bionic_malloc_result(32);
  CHECK(success.pointer != NULL && success.bionic_errno == 0);
  darwin_art_bionic_errno_publish_result(success.bionic_errno);
  CHECK(darwin_art_bionic_errno_load() == 71);
  darwin_art_bionic_free(success.pointer);
  CHECK(errno == 28001);

  volatile size_t impossible = SIZE_MAX;
  DarwinArtBionicAllocationResult failure =
      darwin_art_bionic_malloc_result(impossible);
  CHECK(failure.pointer == NULL && failure.bionic_errno == 12);
  darwin_art_bionic_errno_publish_result(failure.bionic_errno);
  CHECK(darwin_art_bionic_errno_load() == 12);
  CHECK(errno == 28001);
  CHECK(darwin_art_bionic_mallopt(-101, 0) == 1);
  CHECK(darwin_art_bionic_errno_load() == 12 && errno == 28001);
  CHECK(!darwin_art_bionic_android_mallopt(1, NULL, 0));
  CHECK(darwin_art_bionic_errno_load() == 95 && errno == 28001);
  CHECK(!darwin_art_bionic_android_mallopt(1, NULL, 1));
  CHECK(darwin_art_bionic_errno_load() == 22 && errno == 28001);
  puts("allocator errno result seam: allocation/purge/unsupported profiling PASS");
  return 0;
}
