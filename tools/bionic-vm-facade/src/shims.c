#include "aim_bionic_vm.h"

#include <errno.h>

extern void aim_bionic_errno_store(int32_t);
#include <stddef.h>

#define WRAP(saved, call)                       \
  do {                                          \
    const int saved = errno;                    \
    const __typeof__(call) result = (call);      \
    errno = saved;                              \
    return result;                              \
  } while (0)

void* aim_bionic_mmap(void* address, size_t length, int protection,
                             int flags, int fd, int64_t offset) {
  WRAP(saved, aim_bionic_vm_mmap_core(address, length, protection, flags,
                                             fd, offset));
}

void* aim_bionic_mmap64(void* address, size_t length, int protection,
                               int flags, int fd, int64_t offset) {
  WRAP(saved, aim_bionic_vm_mmap_core(address, length, protection, flags,
                                             fd, offset));
}

int aim_bionic_munmap(void* address, size_t length) {
  WRAP(saved, aim_bionic_vm_munmap_core(address, length));
}

int aim_bionic_mprotect(void* address, size_t length, int protection) {
  WRAP(saved,
       aim_bionic_vm_mprotect_core(address, length, protection));
}

int aim_bionic_madvise(void* address, size_t length, int advice) {
  WRAP(saved, aim_bionic_vm_madvise_core(address, length, advice));
}

int aim_bionic_msync(void* address, size_t length, int flags) {
  (void)flags;
  // Owned mappings are anonymous/private, so validation plus a no-op is the
  // complete synchronization behavior until file-backed VM mappings exist.
  return aim_bionic_madvise(address, length, 0);
}

int aim_bionic_posix_madvise(void* address, size_t length, int advice) {
  return aim_bionic_madvise(address, length, advice) == 0 ? 0 : 22;
}

int aim_bionic_mincore(void* address, size_t length,
                              unsigned char* residency) {
  if (residency == NULL) {
    aim_bionic_errno_store(14);
    return -1;
  }
  const size_t page_size = aim_host_vm_page_size();
  if (page_size == 0 || !((page_size & (page_size - 1)) == 0)) {
    aim_bionic_errno_store(22);
    return -1;
  }
  if (aim_bionic_madvise(address, length, 0) != 0) return -1;
  // Avoid length + page_size - 1 wrapping for a hostile native caller.
  const size_t pages = length / page_size + (length % page_size != 0 ? 1 : 0);
  for (size_t index = 0; index < pages; ++index) residency[index] = 1;
  return 0;
}

void* aim_bionic_mremap(void* old_address, size_t old_length,
                               size_t new_length, int flags,
                               void* new_address) {
  WRAP(saved, aim_bionic_vm_mremap_core(
                  old_address, old_length, new_length, flags, new_address));
}

int aim_bionic_mlock_unsupported(const void* address, size_t length) {
  (void)address;
  (void)length;
  aim_bionic_errno_store(38);
  return -1;
}

// Locking guest pages is advisory on this host.  SQLCipher and other NDK
// consumers use munlock to release a best-effort mlock; report success while
// leaving ownership and protection to the VM facade.
int aim_bionic_munlock(const void* address, size_t length) {
  (void)address;
  (void)length;
  return 0;
}

static int Compare(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return (unsigned char)*left < (unsigned char)*right
             ? -1
             : ((unsigned char)*left != (unsigned char)*right);
}

typedef struct Binding {
  const char* name;
  AimBionicVmFunction address;
} Binding;

static const Binding kBindings[] = {
    {"madvise", (AimBionicVmFunction)aim_bionic_madvise},
    {"mincore", (AimBionicVmFunction)aim_bionic_mincore},
    {"mlock", (AimBionicVmFunction)aim_bionic_mlock_unsupported},
    {"mmap", (AimBionicVmFunction)aim_bionic_mmap},
    {"mmap64", (AimBionicVmFunction)aim_bionic_mmap64},
    {"mprotect", (AimBionicVmFunction)aim_bionic_mprotect},
    {"mremap", (AimBionicVmFunction)aim_bionic_mremap},
    {"msync", (AimBionicVmFunction)aim_bionic_msync},
    {"munlock", (AimBionicVmFunction)aim_bionic_munlock},
    {"munmap", (AimBionicVmFunction)aim_bionic_munmap},
    {"posix_madvise", (AimBionicVmFunction)aim_bionic_posix_madvise},
};

AimBionicVmFunction aim_bionic_vm_resolve(
    const char* import_name) {
  if (import_name == NULL) return NULL;
  size_t low = 0;
  size_t high = sizeof(kBindings) / sizeof(kBindings[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    const int order = Compare(import_name, kBindings[middle].name);
    if (order == 0) return kBindings[middle].address;
    if (order < 0)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}
