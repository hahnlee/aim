#include "aim_bionic_allocator.h"

#include <errno.h>
#include <malloc/malloc.h>
#include <stdlib.h>

_Static_assert(sizeof(void*) == 8, "Android arm64 and Darwin arm64 use 64-bit pointers");
_Static_assert(sizeof(size_t) == 8, "Android arm64 and Darwin arm64 use 64-bit size_t");
_Static_assert(sizeof(int) == 4, "Android arm64 and Darwin arm64 use 32-bit int");

extern void aim_bionic_errno_store(int32_t android_errno);

static void* DarwinMalloc(size_t size) {
  const int saved_errno = errno;
  void* result = malloc(size);
  errno = saved_errno;
  return result;
}

static void DarwinFree(void* pointer) {
  const int saved_errno = errno;
  free(pointer);
  errno = saved_errno;
}

static void* DarwinRealloc(void* pointer, size_t size) {
  const int saved_errno = errno;
  void* result = realloc(pointer, size);
  errno = saved_errno;
  return result;
}

static int DarwinPosixMemalign(void** output, size_t alignment, size_t size) {
  const int saved_errno = errno;
  const int result = posix_memalign(output, alignment, size);
  errno = saved_errno;
  return result;
}

static AimBionicAllocationResult AllocationResult(void* pointer) {
  AimBionicAllocationResult result;
  result.pointer = pointer;
  result.bionic_errno = pointer == NULL ? AIM_BIONIC_ENOMEM : 0;
  return result;
}

AimBionicAllocationResult aim_bionic_malloc_result(size_t size) {
  /* Bionic guarantees a non-null, freeable allocation for malloc(0). */
  return AllocationResult(DarwinMalloc(size == 0 ? 1 : size));
}

void* aim_bionic_malloc(size_t size) {
  return aim_bionic_malloc_result(size).pointer;
}

void* aim_bionic_calloc(size_t count, size_t size) {
  const int saved_errno = errno;
  void* result = calloc(count, size);
  errno = saved_errno;
  return result;
}

void aim_bionic_free(void* pointer) {
  DarwinFree(pointer);
}

AimBionicAllocationResult aim_bionic_realloc_result(void* pointer,
                                                                 size_t size) {
  if (pointer == NULL) return aim_bionic_malloc_result(size);
  if (size == 0) {
    DarwinFree(pointer);
    AimBionicAllocationResult result = {NULL, 0};
    return result;
  }
  return AllocationResult(DarwinRealloc(pointer, size));
}

void* aim_bionic_realloc(void* pointer, size_t size) {
  return aim_bionic_realloc_result(pointer, size).pointer;
}

void* aim_bionic_reallocarray(void* pointer, size_t count, size_t size) {
  if (count != 0 && size > SIZE_MAX / count) {
    aim_bionic_errno_store(AIM_BIONIC_ENOMEM);
    return NULL;
  }
  const size_t total = count * size;
  void* result = aim_bionic_realloc(pointer, total);
  if (result == NULL && total != 0)
    aim_bionic_errno_store(AIM_BIONIC_ENOMEM);
  return result;
}

size_t aim_bionic_malloc_usable_size(const void* pointer) {
  return pointer == NULL ? 0 : malloc_size(pointer);
}

static int ValidPosixAlignment(size_t alignment) {
  return alignment >= sizeof(void*) && (alignment & (alignment - 1)) == 0;
}

void* aim_bionic_aligned_alloc(size_t alignment, size_t size) {
  if (!ValidPosixAlignment(alignment) || size % alignment != 0) return NULL;
  void* pointer = NULL;
  const int backend_result = DarwinPosixMemalign(
      &pointer, alignment, size == 0 ? alignment : size);
  return backend_result == 0 ? pointer : NULL;
}

AimBionicAllocationResult aim_bionic_posix_memalign_result(
    size_t alignment, size_t size) {
  if (!ValidPosixAlignment(alignment)) {
    AimBionicAllocationResult invalid = {NULL, AIM_BIONIC_EINVAL};
    return invalid;
  }

  void* pointer = NULL;
  /* Keep the Bionic size-zero result non-null and freeable. */
  const int backend_result =
      DarwinPosixMemalign(&pointer, alignment, size == 0 ? 1 : size);
  if (backend_result != 0 || pointer == NULL) {
    AimBionicAllocationResult exhausted = {NULL, AIM_BIONIC_ENOMEM};
    return exhausted;
  }
  AimBionicAllocationResult success = {pointer, 0};
  return success;
}

int aim_bionic_posix_memalign(void** output, size_t alignment, size_t size) {
  if (output == NULL) return AIM_BIONIC_EINVAL;
  AimBionicAllocationResult result =
      aim_bionic_posix_memalign_result(alignment, size);
  if (result.bionic_errno == 0) *output = result.pointer;
  return result.bionic_errno;
}

void* aim_bionic_memalign(size_t alignment, size_t size) {
  void* result = NULL;
  if (aim_bionic_posix_memalign(&result, alignment, size) != 0) {
    return NULL;
  }
  return result;
}

char* aim_bionic_strdup(const char* source) {
  if (source == NULL) return NULL;
  size_t length = 0;
  while (source[length] != '\0') ++length;
  char* result = (char*)aim_bionic_malloc(length + 1);
  if (result == NULL) return NULL;
  for (size_t index = 0; index <= length; ++index) result[index] = source[index];
  return result;
}

char* aim_bionic_strndup(const char* source, size_t maximum) {
  if (source == NULL) return NULL;
  size_t length = 0;
  while (length < maximum && source[length] != '\0') ++length;
  char* result = (char*)aim_bionic_malloc(length + 1);
  if (result == NULL) return NULL;
  for (size_t index = 0; index < length; ++index) result[index] = source[index];
  result[length] = '\0';
  return result;
}

char* aim_bionic_tempnam_unsupported(const char* directory,
                                            const char* prefix) {
  (void)directory;
  (void)prefix;
  aim_bionic_errno_store(38);
  return NULL;
}

static int NameCompare(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return (unsigned char)*left < (unsigned char)*right
             ? -1
             : ((unsigned char)*left != (unsigned char)*right);
}

typedef struct AimAndroidMallinfo {
  size_t values[10];
} AimAndroidMallinfo;

AimAndroidMallinfo aim_bionic_mallinfo(void) {
  AimAndroidMallinfo value = {{0}};
  return value;
}

static const AimBionicAllocatorBinding kBindings[] = {
    {"aligned_alloc",
     (AimBionicAllocatorFunction)aim_bionic_aligned_alloc,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"calloc", (AimBionicAllocatorFunction)aim_bionic_calloc,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"free", (AimBionicAllocatorFunction)aim_bionic_free,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"mallinfo", (AimBionicAllocatorFunction)aim_bionic_mallinfo,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"malloc", (AimBionicAllocatorFunction)aim_bionic_malloc,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"malloc_usable_size",
     (AimBionicAllocatorFunction)aim_bionic_malloc_usable_size,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"mallopt", (AimBionicAllocatorFunction)aim_bionic_mallopt,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"memalign", (AimBionicAllocatorFunction)aim_bionic_memalign,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"posix_memalign",
     (AimBionicAllocatorFunction)aim_bionic_posix_memalign,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"realloc", (AimBionicAllocatorFunction)aim_bionic_realloc,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"reallocarray",
     (AimBionicAllocatorFunction)aim_bionic_reallocarray,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"strdup", (AimBionicAllocatorFunction)aim_bionic_strdup,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"strndup", (AimBionicAllocatorFunction)aim_bionic_strndup,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"tempnam", (AimBionicAllocatorFunction)aim_bionic_tempnam_unsupported,
     AIM_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         AIM_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
};

const AimBionicAllocatorBinding* aim_bionic_allocator_table(size_t* count) {
  if (count != NULL) *count = sizeof(kBindings) / sizeof(kBindings[0]);
  return kBindings;
}

AimBionicAllocatorFunction aim_bionic_allocator_resolve(
    const char* import_name) {
  if (import_name == NULL) return NULL;
  size_t low = 0;
  size_t high = sizeof(kBindings) / sizeof(kBindings[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    const int order = NameCompare(import_name, kBindings[middle].import_name);
    if (order == 0) return kBindings[middle].address;
    if (order < 0)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}
