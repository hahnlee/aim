#include <stddef.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/types.h>

typedef void* (*Mmap)(void*, size_t, int, int, int, off_t);
typedef void* (*Mmap64)(void*, size_t, int, int, int, off64_t);
typedef int (*Munmap)(void*, size_t);
typedef int (*Mprotect)(void*, size_t, int);
typedef int (*Madvise)(void*, size_t, int);

_Static_assert(sizeof(size_t) == 8 && sizeof(off_t) == 8 &&
                   sizeof(off64_t) == 8,
               "Android arm64 VM scalar ABI drift");
_Static_assert(_Generic(&mmap, Mmap: 1, default: 0), "mmap ABI");
_Static_assert(_Generic(&mmap64, Mmap64: 1, default: 0), "mmap64 ABI");
_Static_assert(_Generic(&munmap, Munmap: 1, default: 0), "munmap ABI");
_Static_assert(_Generic(&mprotect, Mprotect: 1, default: 0), "mprotect ABI");
_Static_assert(_Generic(&madvise, Madvise: 1, default: 0), "madvise ABI");
_Static_assert(PROT_NONE == 0 && PROT_READ == 1 && PROT_WRITE == 2 &&
                   PROT_EXEC == 4,
               "Android PROT drift");
_Static_assert(MAP_PRIVATE == 2 && MAP_FIXED == 0x10 &&
                   MAP_ANONYMOUS == 0x20 && MAP_GROWSDOWN == 0x100 &&
                   MAP_STACK == 0x20000,
               "Android MAP drift");
_Static_assert(MADV_NORMAL == 0 && MADV_RANDOM == 1 &&
                   MADV_SEQUENTIAL == 2 && MADV_WILLNEED == 3 &&
                   MADV_DONTNEED == 4 && MADV_FREE == 8,
               "Android MADV drift");
int AimProbeMapFailed(void) { return (intptr_t)MAP_FAILED == -1 ? 0 : 1; }
