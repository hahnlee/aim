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
} DarwinArtAndroidDlExtInfo;

typedef void* (*DarwinArtOpenCallback)(void*, const char*, int,
                                      const DarwinArtAndroidDlExtInfo*, char*, size_t);
typedef void* (*DarwinArtLookupCallback)(void*, void*, const char*, const char*, char*, size_t);
typedef int (*DarwinArtCloseCallback)(void*, void*, char*, size_t);

typedef struct {
  void* context;
  DarwinArtOpenCallback open;
  DarwinArtLookupCallback lookup;
  DarwinArtCloseCallback close;
} DarwinArtLoaderCallbacks;

typedef struct {
  uint32_t provider;
  uint32_t ordinal;
  uintptr_t address;
} DarwinArtDsoResolution;

int darwin_art_loader_bind(const DarwinArtLoaderCallbacks*);
void* darwin_art_bionic_dlopen(const char*, int);
void* darwin_art_bionic_android_dlopen_ext(const char*, int, const DarwinArtAndroidDlExtInfo*);
void* darwin_art_bionic_dlsym(void*, const char*);
int darwin_art_bionic_dlclose(void*);
char* darwin_art_bionic_dlerror(void);
/* Real resident MoltenVK owner only; fallback function pointers are not readiness. */
int darwin_art_bionic_vulkan_provider_ready(void);
/* 0=resolved, 1=unavailable, 2=unsupported version, 3=bad argument.
 * Writes zero on failure when output is valid. Does not acquire a new host DSO. */
int darwin_art_bionic_vulkan_symbol(const char*, const char*, uintptr_t*);

/* 0=resolved, 1=SONAME denied, 2=symbol denied, 3=version mismatch, 4=bad input. */
int darwin_art_dso_resolve(const char*, const char*, const char*, DarwinArtDsoResolution*);

#ifdef __cplusplus
}
#endif
