#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  uint64_t flags;
  void* reserved_addr;
  size_t reserved_size;
  int relro_fd;
  int library_fd;
  int64_t library_fd_offset;
  void* library_namespace;
} AimAndroidDlExtInfo;

typedef void* (*AimOpenCallback)(void*, const char*, int,
                                      const AimAndroidDlExtInfo*, char*, size_t);
typedef void* (*AimLookupCallback)(void*, void*, const char*, const char*, char*, size_t);
typedef int (*AimCloseCallback)(void*, void*, char*, size_t);

typedef struct {
  void* context;
  AimOpenCallback open;
  AimLookupCallback lookup;
  AimCloseCallback close;
} AimLoaderCallbacks;

typedef struct {
  uint32_t provider;
  uint32_t ordinal;
  uintptr_t address;
} AimDsoResolution;

int aim_loader_bind(const AimLoaderCallbacks*);
void* aim_bionic_dlopen(const char*, int);
void* aim_bionic_android_dlopen_ext(const char*, int, const AimAndroidDlExtInfo*);
void* aim_bionic_dlsym(void*, const char*);
int aim_bionic_dlclose(void*);
char* aim_bionic_dlerror(void);
/* Real resident MoltenVK owner only; fallback function pointers are not readiness. */
int aim_bionic_vulkan_provider_ready(void);
/* 0=resolved, 1=unavailable, 2=unsupported version, 3=bad argument.
 * Writes zero on failure when output is valid. Does not acquire a new host DSO. */
int aim_bionic_vulkan_symbol(const char*, const char*, uintptr_t*);

/* 0=resolved, 1=SONAME denied, 2=symbol denied, 3=version mismatch, 4=bad input. */
int aim_dso_resolve(const char*, const char*, const char*, AimDsoResolution*);

#ifdef __cplusplus
}
#endif
