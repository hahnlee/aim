#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

extern void aim_bionic_errno_store(int32_t android_errno);

/*
 * Isolated stdio audits do not link the complete runtime provider closure.
 * Weak definitions make that boundary explicit while allowing integration
 * audits to replace every entry with the real provider implementation.
 */
#define AUDIT_WEAK __attribute__((weak))

AUDIT_WEAK void* aim_bionic_realloc(void* pointer, size_t size) {
  return realloc(pointer, size);
}

AUDIT_WEAK int aim_bionic_atoi(const char* text) {
  if (text == NULL) return 0;
  int sign = 1;
  if (*text == '-') {
    sign = -1;
    ++text;
  } else if (*text == '+') {
    ++text;
  }
  int value = 0;
  while (*text >= '0' && *text <= '9') {
    value = value * 10 + (*text++ - '0');
  }
  return sign * value;
}

AUDIT_WEAK int aim_bionic_open(const char* path, int flags,
                                       uint32_t mode) {
  (void)path;
  (void)flags;
  (void)mode;
  aim_bionic_errno_store(2);
  return -1;
}

AUDIT_WEAK intptr_t aim_bionic_read(int fd, void* buffer,
                                            size_t count) {
  (void)fd;
  (void)buffer;
  (void)count;
  aim_bionic_errno_store(9);
  return -1;
}

AUDIT_WEAK intptr_t aim_bionic_write(int fd, const void* buffer,
                                             size_t count) {
  (void)fd;
  (void)buffer;
  (void)count;
  aim_bionic_errno_store(9);
  return -1;
}

AUDIT_WEAK int64_t aim_bionic_lseek(int fd, int64_t offset,
                                            int whence) {
  (void)fd;
  (void)offset;
  (void)whence;
  aim_bionic_errno_store(9);
  return -1;
}

AUDIT_WEAK int aim_bionic_close(int fd) {
  (void)fd;
  aim_bionic_errno_store(9);
  return -1;
}

AUDIT_WEAK void* aim_bionic_wide_stdio_install(const void* backend) {
  (void)backend;
  return (void*)(uintptr_t)1;
}

AUDIT_WEAK int aim_bionic_wide_stdio_uninstall(void** activation) {
  if (activation == NULL) return -1;
  *activation = NULL;
  return 0;
}

AUDIT_WEAK int aim_bionic_wide_stdio_reset(void* file) {
  (void)file;
  return 0;
}

AUDIT_WEAK int aim_bionic_wide_stdio_forget(void* file) {
  (void)file;
  return 0;
}
