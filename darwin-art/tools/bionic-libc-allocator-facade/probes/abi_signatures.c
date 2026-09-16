#include "darwin_art_bionic_allocator.h"

#include <errno.h>

typedef void* (*MallocSignature)(size_t);
typedef void (*FreeSignature)(void*);
typedef void* (*ReallocSignature)(void*, size_t);
typedef void* (*ReallocarraySignature)(void*, size_t, size_t);
typedef int (*PosixMemalignSignature)(void**, size_t, size_t);
typedef void* (*AlignedAllocSignature)(size_t, size_t);

_Static_assert(_Generic(&darwin_art_bionic_malloc, MallocSignature: 1, default: 0),
               "malloc signature drift");
_Static_assert(_Generic(&darwin_art_bionic_free, FreeSignature: 1, default: 0),
               "free signature drift");
_Static_assert(_Generic(&darwin_art_bionic_realloc, ReallocSignature: 1, default: 0),
               "realloc signature drift");
_Static_assert(_Generic(&darwin_art_bionic_reallocarray,
                        ReallocarraySignature: 1, default: 0),
               "reallocarray signature drift");
_Static_assert(_Generic(&darwin_art_bionic_posix_memalign,
                        PosixMemalignSignature: 1, default: 0),
               "posix_memalign signature drift");
_Static_assert(_Generic(&darwin_art_bionic_aligned_alloc,
                        AlignedAllocSignature: 1, default: 0),
               "aligned_alloc signature drift");
_Static_assert(sizeof(void*) == 8 && sizeof(size_t) == 8 && sizeof(int) == 4,
               "this direct-address audit is arm64-only");
_Static_assert(DARWIN_ART_BIONIC_ENOMEM == 12, "Android ENOMEM drift");
_Static_assert(DARWIN_ART_BIONIC_EINVAL == 22, "Android EINVAL drift");
_Static_assert(ENOMEM == DARWIN_ART_BIONIC_ENOMEM, "target ENOMEM drift");
_Static_assert(EINVAL == DARWIN_ART_BIONIC_EINVAL, "target EINVAL drift");
