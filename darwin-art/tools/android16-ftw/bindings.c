// AOSP owns traversal policy. These imports only select the guest providers;
// no Android path, directory token or stat buffer reaches host libc directly.
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_allocator.h"
#include "darwin_art_bionic_libc_leaf.h"
#include "darwin_art_bionic_errno.h"
#include "darwin_art_bionic_process_state.h"
#include <stdarg.h>

int32_t* darwin_art_ftw_import___errno(void) {
  return darwin_art_bionic___errno();
}
int darwin_art_ftw_import_access(const char* path, int mode) {
  return darwin_art_bionic_access(path, mode);
}
int darwin_art_ftw_import_close(int fd) {
  return darwin_art_bionic_close(fd);
}
int darwin_art_ftw_import_closedir(void* directory) {
  return darwin_art_bionic_closedir(directory);
}
int darwin_art_ftw_import_dirfd(void* directory) {
  return darwin_art_bionic_dirfd(directory);
}
int darwin_art_ftw_import_fchdir(int fd) {
  return darwin_art_bionic_fchdir(fd);
}
int darwin_art_ftw_import_fstat(int fd, DarwinArtAndroidStat* status) {
  return darwin_art_bionic_fstat(fd, status);
}
int darwin_art_ftw_import_fstatat(int fd, const char* path,
                               DarwinArtAndroidStat* status, int flags) {
  return darwin_art_bionic_fstatat(fd, path, status, flags);
}
int darwin_art_ftw_import_getpagesize(void) {
  return darwin_art_bionic_getpagesize();
}
int darwin_art_ftw_import_open(const char* path, int flags, ...) {
  // Android O_CREAT and O_TMPFILE require the promoted mode argument.
  unsigned int mode = 0;
  if ((flags & 64) != 0 || (flags & 0x410000) == 0x410000) {
    va_list arguments;
    va_start(arguments, flags);
    mode = va_arg(arguments, unsigned int);
    va_end(arguments);
  }
  return darwin_art_bionic_open(path, flags, mode);
}
void* darwin_art_ftw_import_opendir(const char* path) {
  return darwin_art_bionic_opendir(path);
}
DarwinArtAndroidDirent* darwin_art_ftw_import_readdir(void* directory) {
  return darwin_art_bionic_readdir(directory);
}
void* darwin_art_ftw_import_malloc(size_t size) {
  const DarwinArtBionicAllocationResult result =
      darwin_art_bionic_malloc_result(size);
  if (result.bionic_errno != 0)
    darwin_art_bionic_errno_store(result.bionic_errno);
  return result.pointer;
}
void* darwin_art_ftw_import_calloc(size_t count, size_t size) {
  void* result = darwin_art_bionic_calloc(count, size);
  if (result == NULL)
    darwin_art_bionic_errno_store(DARWIN_ART_BIONIC_ENOMEM);
  return result;
}
void darwin_art_ftw_import_free(void* pointer) {
  darwin_art_bionic_free(pointer);
}
void* darwin_art_ftw_import_reallocarray(void* pointer, size_t count, size_t size) {
  return darwin_art_bionic_reallocarray(pointer, count, size);
}
void* darwin_art_ftw_import_memcpy(void* destination, const void* source,
                                 size_t length) {
  return darwin_art_bionic_memcpy(destination, source, length);
}
void* darwin_art_ftw_import_memmove(void* destination, const void* source,
                                  size_t length) {
  return darwin_art_bionic_memmove(destination, source, length);
}
void* darwin_art_ftw_import_memset(void* destination, int value, size_t length) {
  return darwin_art_bionic_memset(destination, value, length);
}
void* darwin_art_ftw_import_memset_explicit(void* destination, int value,
                                         size_t length) {
  return darwin_art_bionic_memset_explicit(destination, value, length);
}
void darwin_art_ftw_import_qsort(void* base, size_t count, size_t size,
                               int (*compare)(const void*, const void*)) {
  darwin_art_bionic_qsort(base, count, size, compare);
}
size_t darwin_art_ftw_import_strlen(const char* string) {
  return darwin_art_bionic_strlen(string);
}
char* darwin_art_ftw_import_strrchr(const char* string, int character) {
  // Bionic's C signature returns char*, including for a const input pointer.
  return (char*)darwin_art_bionic_strrchr(string, character);
}
