#include "darwin_art_bionic_stdio.h"
#include <stdlib.h>

// Bionic fortify.cpp, revision 09a271af557444c9a6b3f3146d6d474156fd6cdb:
// multiplication overflow belongs to fread/fwrite's error path; a finite
// buffer overrun is fatal before stream I/O. Android crash-message reporting
// remains separate from this Darwin termination boundary.
size_t darwin_art_bionic___fread_chk(void* buffer, size_t size, size_t count,
                                    DarwinArtAndroidFile* stream, size_t capacity) {
  size_t total;
  if (!__builtin_mul_overflow(size, count, &total) && total > capacity) abort();
  return darwin_art_bionic_fread(buffer, size, count, stream);
}

size_t darwin_art_bionic___fwrite_chk(const void* buffer, size_t size, size_t count,
                                     DarwinArtAndroidFile* stream, size_t capacity) {
  size_t total;
  if (!__builtin_mul_overflow(size, count, &total) && total > capacity) abort();
  return darwin_art_bionic_fwrite(buffer, size, count, stream);
}
