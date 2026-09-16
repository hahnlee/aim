#include "darwin_art_bionic_allocator.h"

#include <errno.h>
#include <malloc/malloc.h>
#include <stdlib.h>

_Static_assert(sizeof(void*) == 8, "Android arm64 and Darwin arm64 use 64-bit pointers");
_Static_assert(sizeof(size_t) == 8, "Android arm64 and Darwin arm64 use 64-bit size_t");
_Static_assert(sizeof(int) == 4, "Android arm64 and Darwin arm64 use 32-bit int");

extern void darwin_art_bionic_errno_store(int32_t android_errno);

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

static DarwinArtBionicAllocationResult AllocationResult(void* pointer) {
  DarwinArtBionicAllocationResult result;
  result.pointer = pointer;
  result.bionic_errno = pointer == NULL ? DARWIN_ART_BIONIC_ENOMEM : 0;
  return result;
}

DarwinArtBionicAllocationResult darwin_art_bionic_malloc_result(size_t size) {
  /* Bionic guarantees a non-null, freeable allocation for malloc(0). */
  return AllocationResult(DarwinMalloc(size == 0 ? 1 : size));
}

void* darwin_art_bionic_malloc(size_t size) {
  return darwin_art_bionic_malloc_result(size).pointer;
}

void* darwin_art_bionic_calloc(size_t count, size_t size) {
  const int saved_errno = errno;
  void* result = calloc(count, size);
  errno = saved_errno;
  return result;
}

void darwin_art_bionic_free(void* pointer) {
  DarwinFree(pointer);
}

DarwinArtBionicAllocationResult darwin_art_bionic_realloc_result(void* pointer,
                                                                 size_t size) {
  if (pointer == NULL) return darwin_art_bionic_malloc_result(size);
  if (size == 0) {
    DarwinFree(pointer);
    DarwinArtBionicAllocationResult result = {NULL, 0};
    return result;
  }
  return AllocationResult(DarwinRealloc(pointer, size));
}

void* darwin_art_bionic_realloc(void* pointer, size_t size) {
  return darwin_art_bionic_realloc_result(pointer, size).pointer;
}

void* darwin_art_bionic_reallocarray(void* pointer, size_t count, size_t size) {
  if (count != 0 && size > SIZE_MAX / count) {
    darwin_art_bionic_errno_store(DARWIN_ART_BIONIC_ENOMEM);
    return NULL;
  }
  const size_t total = count * size;
  void* result = darwin_art_bionic_realloc(pointer, total);
  if (result == NULL && total != 0)
    darwin_art_bionic_errno_store(DARWIN_ART_BIONIC_ENOMEM);
  return result;
}

size_t darwin_art_bionic_malloc_usable_size(const void* pointer) {
  return pointer == NULL ? 0 : malloc_size(pointer);
}

static int ValidPosixAlignment(size_t alignment) {
  return alignment >= sizeof(void*) && (alignment & (alignment - 1)) == 0;
}

void* darwin_art_bionic_aligned_alloc(size_t alignment, size_t size) {
  if (!ValidPosixAlignment(alignment) || size % alignment != 0) return NULL;
  void* pointer = NULL;
  const int backend_result = DarwinPosixMemalign(
      &pointer, alignment, size == 0 ? alignment : size);
  return backend_result == 0 ? pointer : NULL;
}

DarwinArtBionicAllocationResult darwin_art_bionic_posix_memalign_result(
    size_t alignment, size_t size) {
  if (!ValidPosixAlignment(alignment)) {
    DarwinArtBionicAllocationResult invalid = {NULL, DARWIN_ART_BIONIC_EINVAL};
    return invalid;
  }

  void* pointer = NULL;
  /* Keep the Bionic size-zero result non-null and freeable. */
  const int backend_result =
      DarwinPosixMemalign(&pointer, alignment, size == 0 ? 1 : size);
  if (backend_result != 0 || pointer == NULL) {
    DarwinArtBionicAllocationResult exhausted = {NULL, DARWIN_ART_BIONIC_ENOMEM};
    return exhausted;
  }
  DarwinArtBionicAllocationResult success = {pointer, 0};
  return success;
}

int darwin_art_bionic_posix_memalign(void** output, size_t alignment, size_t size) {
  if (output == NULL) return DARWIN_ART_BIONIC_EINVAL;
  DarwinArtBionicAllocationResult result =
      darwin_art_bionic_posix_memalign_result(alignment, size);
  if (result.bionic_errno == 0) *output = result.pointer;
  return result.bionic_errno;
}

void* darwin_art_bionic_memalign(size_t alignment, size_t size) {
  void* result = NULL;
  if (darwin_art_bionic_posix_memalign(&result, alignment, size) != 0) {
    return NULL;
  }
  return result;
}

char* darwin_art_bionic_strdup(const char* source) {
  if (source == NULL) return NULL;
  size_t length = 0;
  while (source[length] != '\0') ++length;
  char* result = (char*)darwin_art_bionic_malloc(length + 1);
  if (result == NULL) return NULL;
  for (size_t index = 0; index <= length; ++index) result[index] = source[index];
  return result;
}

char* darwin_art_bionic_strndup(const char* source, size_t maximum) {
  if (source == NULL) return NULL;
  size_t length = 0;
  while (length < maximum && source[length] != '\0') ++length;
  char* result = (char*)darwin_art_bionic_malloc(length + 1);
  if (result == NULL) return NULL;
  for (size_t index = 0; index < length; ++index) result[index] = source[index];
  result[length] = '\0';
  return result;
}

char* darwin_art_bionic_tempnam_unsupported(const char* directory,
                                            const char* prefix) {
  (void)directory;
  (void)prefix;
  darwin_art_bionic_errno_store(38);
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

typedef struct DarwinArtAndroidMallinfo {
  size_t values[10];
} DarwinArtAndroidMallinfo;

DarwinArtAndroidMallinfo darwin_art_bionic_mallinfo(void) {
  DarwinArtAndroidMallinfo value = {{0}};
  return value;
}

static const DarwinArtBionicAllocatorBinding kBindings[] = {
    {"aligned_alloc",
     (DarwinArtBionicAllocatorFunction)darwin_art_bionic_aligned_alloc,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"calloc", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_calloc,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"free", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_free,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"mallinfo", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_mallinfo,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"malloc", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_malloc,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"malloc_usable_size",
     (DarwinArtBionicAllocatorFunction)darwin_art_bionic_malloc_usable_size,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"mallopt", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_mallopt,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"memalign", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_memalign,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"posix_memalign",
     (DarwinArtBionicAllocatorFunction)darwin_art_bionic_posix_memalign,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_FULL_RETURN_CODE},
    {"realloc", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_realloc,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"reallocarray",
     (DarwinArtBionicAllocatorFunction)darwin_art_bionic_reallocarray,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"strdup", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_strdup,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"strndup", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_strndup,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_DARWIN_OWNS_BLOCK |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
    {"tempnam", (DarwinArtBionicAllocatorFunction)darwin_art_bionic_tempnam_unsupported,
     DARWIN_ART_BIONIC_ALLOC_FIXED_REGISTER_ABI |
         DARWIN_ART_BIONIC_ALLOC_NEEDS_ERRNO_RESULT_SEAM},
};

const DarwinArtBionicAllocatorBinding* darwin_art_bionic_allocator_table(size_t* count) {
  if (count != NULL) *count = sizeof(kBindings) / sizeof(kBindings[0]);
  return kBindings;
}

DarwinArtBionicAllocatorFunction darwin_art_bionic_allocator_resolve(
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
