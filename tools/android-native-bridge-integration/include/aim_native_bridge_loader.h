#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum AimJniCallType {
  AIM_JNI_CALL_REGULAR = 1,
  AIM_JNI_CALL_CRITICAL = 2,
};

typedef uint64_t AimLoaderNamespace;
typedef void* AimLoaderHandle;

/*
 * Atomic C ABI expected by the Darwin libnativeloader/nativebridge adapter.
 * The adapter owns Java ClassLoader weak-global identity and maps it to one
 * persistent loader namespace. The Rust ELF loader owns all returned handles.
 */
typedef struct AimElfLoaderV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void* context;

  AimLoaderNamespace (*create_namespace)(
      void* context, AimLoaderNamespace parent, const char* search_path,
      const char* permitted_path, char* error, size_t error_capacity);
  AimLoaderHandle (*open)(
      void* context, AimLoaderNamespace name_space, const char* path, int flags,
      char* error, size_t error_capacity);
  int (*close)(void* context, AimLoaderHandle handle, char* error, size_t error_capacity);
  void* (*get_trampoline)(
      void* context, AimLoaderHandle handle, const char* symbol,
      const char* shorty, uint32_t shorty_length, enum AimJniCallType call_type,
      char* error, size_t error_capacity);
  int (*owns_function_pointer)(void* context, const void* address);
} AimElfLoaderV1;

enum { AIM_ELF_LOADER_ABI_V1 = 1 };

#ifdef __cplusplus
}
#endif

