#include <stddef.h>

/* These definitions exist only so this facade's standalone tests can link.
 * The complete runtime links the strong android-dso-namespace definitions,
 * which must win without creating a second provider owner. */
#define AIM_STANDALONE_WEAK __attribute__((weak))

AIM_STANDALONE_WEAK void* aim_bionic_android_dlopen_ext(
    const char* name, int flags, const void* info) {
  (void)name;
  (void)flags;
  (void)info;
  return NULL;
}

AIM_STANDALONE_WEAK int aim_bionic_dlclose(void* handle) {
  (void)handle;
  return -1;
}

AIM_STANDALONE_WEAK char* aim_bionic_dlerror(void) { return NULL; }

AIM_STANDALONE_WEAK void* aim_bionic_dlopen(const char* name,
                                                          int flags) {
  (void)name;
  (void)flags;
  return NULL;
}

AIM_STANDALONE_WEAK void* aim_bionic_dlsym(void* handle,
                                                         const char* name) {
  (void)handle;
  (void)name;
  return NULL;
}
