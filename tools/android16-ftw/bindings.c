// AOSP owns traversal policy. These imports only select the guest providers;
// no Android path, directory token or stat buffer reaches host libc directly.
#include "aim_bionic_fs.h"
#include "aim_bionic_allocator.h"
#include "aim_bionic_libc_leaf.h"
#include "aim_bionic_errno.h"
#include "aim_bionic_process_state.h"
#include <stdarg.h>

int32_t* aim_ftw_import___errno(void) {
  return aim_bionic___errno();
}
int aim_ftw_import_access(const char* path, int mode) {
  return aim_bionic_access(path, mode);
}
int aim_ftw_import_close(int fd) {
  return aim_bionic_close(fd);
}
int aim_ftw_import_closedir(void* directory) {
  return aim_bionic_closedir(directory);
}
int aim_ftw_import_dirfd(void* directory) {
  return aim_bionic_dirfd(directory);
}
int aim_ftw_import_fchdir(int fd) {
  return aim_bionic_fchdir(fd);
}
int aim_ftw_import_fstat(int fd, AimAndroidStat* status) {
  return aim_bionic_fstat(fd, status);
}
int aim_ftw_import_fstatat(int fd, const char* path,
                               AimAndroidStat* status, int flags) {
  return aim_bionic_fstatat(fd, path, status, flags);
}
int aim_ftw_import_getpagesize(void) {
  return aim_bionic_getpagesize();
}
int aim_ftw_import_open(const char* path, int flags, ...) {
  // Android O_CREAT and O_TMPFILE require the promoted mode argument.
  unsigned int mode = 0;
  if ((flags & 64) != 0 || (flags & 0x410000) == 0x410000) {
    va_list arguments;
    va_start(arguments, flags);
    mode = va_arg(arguments, unsigned int);
    va_end(arguments);
  }
  return aim_bionic_open(path, flags, mode);
}
void* aim_ftw_import_opendir(const char* path) {
  return aim_bionic_opendir(path);
}
AimAndroidDirent* aim_ftw_import_readdir(void* directory) {
  return aim_bionic_readdir(directory);
}
void* aim_ftw_import_malloc(size_t size) {
  const AimBionicAllocationResult result =
      aim_bionic_malloc_result(size);
  if (result.bionic_errno != 0)
    aim_bionic_errno_store(result.bionic_errno);
  return result.pointer;
}
void* aim_ftw_import_calloc(size_t count, size_t size) {
  void* result = aim_bionic_calloc(count, size);
  if (result == NULL)
    aim_bionic_errno_store(AIM_BIONIC_ENOMEM);
  return result;
}
void aim_ftw_import_free(void* pointer) {
  aim_bionic_free(pointer);
}
void* aim_ftw_import_reallocarray(void* pointer, size_t count, size_t size) {
  return aim_bionic_reallocarray(pointer, count, size);
}
void* aim_ftw_import_memcpy(void* destination, const void* source,
                                 size_t length) {
  return aim_bionic_memcpy(destination, source, length);
}
void* aim_ftw_import_memmove(void* destination, const void* source,
                                  size_t length) {
  return aim_bionic_memmove(destination, source, length);
}
void* aim_ftw_import_memset(void* destination, int value, size_t length) {
  return aim_bionic_memset(destination, value, length);
}
void* aim_ftw_import_memset_explicit(void* destination, int value,
                                         size_t length) {
  return aim_bionic_memset_explicit(destination, value, length);
}
void aim_ftw_import_qsort(void* base, size_t count, size_t size,
                               int (*compare)(const void*, const void*)) {
  aim_bionic_qsort(base, count, size, compare);
}
size_t aim_ftw_import_strlen(const char* string) {
  return aim_bionic_strlen(string);
}
char* aim_ftw_import_strrchr(const char* string, int character) {
  // Bionic's C signature returns char*, including for a const input pointer.
  return (char*)aim_bionic_strrchr(string, character);
}
