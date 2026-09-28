#ifndef AIM_BIONIC_ALLOCATOR_H_
#define AIM_BIONIC_ALLOCATOR_H_

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
  AIM_BIONIC_ENOMEM = 12,
  AIM_BIONIC_EINVAL = 22,
};

enum {
  AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI = 1u << 0,
  AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK = 1u << 1,
  AIM_BIONIC_ALLOC_FULL_RETURN_CODE = 1u << 2,
  AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM = 1u << 3,
};

typedef void (*AimBionicAllocatorFunction)(void);

typedef struct AimBionicAllocationResult {
  void* pointer;
  int32_t bionic_errno;
} AimBionicAllocationResult;

typedef struct AimBionicAllocatorBinding {
  const char* import_name;
  AimBionicAllocatorFunction address;
  uint32_t capabilities;
} AimBionicAllocatorBinding;

/* Direct Android-import signatures. malloc/realloc deliberately do not write
 * host errno; use the result seam until the Bionic TLS errno provider exists. */
void* aim_bionic_malloc(size_t size);
void* aim_bionic_calloc(size_t count, size_t size);
void aim_bionic_free(void* pointer);
void* aim_bionic_realloc(void* pointer, size_t size);
void* aim_bionic_reallocarray(void* pointer, size_t count, size_t size);
void* aim_bionic_aligned_alloc(size_t alignment, size_t size);
int aim_bionic_mallopt(int param, int value);
/* Platform-private allocator controls. Unsupported profiling is reported as
 * false/Android ENOTSUP; it is never represented as an installed profiler. */
bool aim_bionic_android_mallopt(int opcode, void* arg, size_t arg_size);
size_t aim_bionic_malloc_usable_size(const void* pointer);
int aim_bionic_posix_memalign(void** output, size_t alignment, size_t size);
void* aim_bionic_memalign(size_t alignment, size_t size);
char* aim_bionic_strdup(const char* source);
char* aim_bionic_strndup(const char* source, size_t maximum);

/* Non-import seam carrying Android errno numbers without exposing Darwin TLS. */
AimBionicAllocationResult aim_bionic_malloc_result(size_t size);
AimBionicAllocationResult aim_bionic_realloc_result(void* pointer,
                                                                 size_t size);
AimBionicAllocationResult aim_bionic_posix_memalign_result(
    size_t alignment, size_t size);

const AimBionicAllocatorBinding* aim_bionic_allocator_table(size_t* count);
AimBionicAllocatorFunction aim_bionic_allocator_resolve(
    const char* import_name);

#ifdef __cplusplus
}
#endif

#endif
