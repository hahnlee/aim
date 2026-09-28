#pragma once

#include_next <sys/mman.h>

#include <cerrno>
#include <cstddef>

// Darwin has no Linux mremap. ART's remaining uses are optional zygote/JIT
// sharing optimizations whose callers already handle MAP_FAILED safely.
#ifndef MREMAP_MAYMOVE
#define MREMAP_MAYMOVE 1
#endif
#ifndef MREMAP_FIXED
#define MREMAP_FIXED 2
#endif
#ifndef MREMAP_DONTUNMAP
#define MREMAP_DONTUNMAP 4
#endif

static inline void* mremap(void*, std::size_t, std::size_t, int, ...) {
  errno = ENOTSUP;
  return MAP_FAILED;
}
