#ifndef DARWIN_ART_BIONIC_ALLOCATOR_H_
#define DARWIN_ART_BIONIC_ALLOCATOR_H_

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
  DARWIN_ART_BIONIC_ENOMEM = 12,
  DARWIN_ART_BIONIC_EINVAL = 22,
};

enum {
  DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI = 1u << 0,
  DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK = 1u << 1,
  DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE = 1u << 2,
  DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM = 1u << 3,
};

typedef void (*DarwinArtBionicAllocatorFunction)(void);

typedef struct DarwinArtBionicAllocationResult {
  void* pointer;
  int32_t bionic_errno;
} DarwinArtBionicAllocationResult;

typedef struct DarwinArtBionicAllocatorBinding {
  const char* import_name;
  DarwinArtBionicAllocatorFunction address;
  uint32_t capabilities;
} DarwinArtBionicAllocatorBinding;

/* Direct Android-import signatures. malloc/realloc deliberately do not write
 * host errno; use the result seam until the Bionic TLS errno provider exists. */
void* darwin_art_bionic_malloc(size_t size);
void* darwin_art_bionic_calloc(size_t count, size_t size);
void darwin_art_bionic_free(void* pointer);
void* darwin_art_bionic_realloc(void* pointer, size_t size);
void* darwin_art_bionic_reallocarray(void* pointer, size_t count, size_t size);
void* darwin_art_bionic_aligned_alloc(size_t alignment, size_t size);
int darwin_art_bionic_mallopt(int param, int value);
/* Platform-private allocator controls. Unsupported profiling is reported as
 * false/Android ENOTSUP; it is never represented as an installed profiler. */
bool darwin_art_bionic_android_mallopt(int opcode, void* arg, size_t arg_size);
size_t darwin_art_bionic_malloc_usable_size(const void* pointer);
int darwin_art_bionic_posix_memalign(void** output, size_t alignment, size_t size);
void* darwin_art_bionic_memalign(size_t alignment, size_t size);
char* darwin_art_bionic_strdup(const char* source);
char* darwin_art_bionic_strndup(const char* source, size_t maximum);

/* Non-import seam carrying Android errno numbers without exposing Darwin TLS. */
DarwinArtBionicAllocationResult darwin_art_bionic_malloc_result(size_t size);
DarwinArtBionicAllocationResult darwin_art_bionic_realloc_result(void* pointer,
                                                                 size_t size);
DarwinArtBionicAllocationResult darwin_art_bionic_posix_memalign_result(
    size_t alignment, size_t size);

const DarwinArtBionicAllocatorBinding* darwin_art_bionic_allocator_table(size_t* count);
DarwinArtBionicAllocatorFunction darwin_art_bionic_allocator_resolve(
    const char* import_name);

#ifdef __cplusplus
}
#endif

#endif
