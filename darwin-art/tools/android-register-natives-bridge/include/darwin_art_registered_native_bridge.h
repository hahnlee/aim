#ifndef DARWIN_ART_REGISTERED_NATIVE_BRIDGE_H_
#define DARWIN_ART_REGISTERED_NATIVE_BRIDGE_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define DARWIN_ART_REGISTERED_NATIVE_BRIDGE_ABI_VERSION 1u

typedef enum DarwinArtJniCallType {
  DARWIN_ART_JNI_CALL_REGULAR = 1,
  DARWIN_ART_JNI_CALL_CRITICAL_NATIVE = 2,
} DarwinArtJniCallType;

typedef enum DarwinArtRegisteredNativeResolution {
  DARWIN_ART_REGISTERED_NATIVE_ERROR = -1,
  DARWIN_ART_REGISTERED_NATIVE_DIRECT = 0,
  DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE = 1,
} DarwinArtRegisteredNativeResolution;

// A monotonically increasing generation is required whenever an image address
// range is reused. image_id identifies the loader object; generation prevents
// a stale trampoline from being reused after dlclose/load.
typedef struct DarwinArtAndroidFunctionOwnerV1 {
  uint64_t image_id;
  uint64_t generation;
  uintptr_t executable_begin;
  uintptr_t executable_end;
} DarwinArtAndroidFunctionOwnerV1;

// Return positive only after acquiring a lease, zero for an unowned pointer,
// and negative on lookup failure. Zero/negative results must not acquire a lease.
// Even a malformed positive owner record is passed to release_owner.
typedef int (*DarwinArtLookupAndroidFunctionOwnerV1)(
    void* context,
    const void* function,
    DarwinArtAndroidFunctionOwnerV1* owner_out);

// A successful lookup owns an image lease. On cache publication it is retained
// until retirement/destruction, and released AFTER destroying the thunk. Other
// lookups release before returning. The loader must prevent unmapping until the
// matching release completes; retirement must precede waiting for cache leases.
typedef void (*DarwinArtReleaseAndroidFunctionOwnerV1)(
    void* context,
    const DarwinArtAndroidFunctionOwnerV1* owner);

// Callbacks must not throw. Build/destroy run outside the cache mutex. Concurrent
// misses may build duplicates; each result must have independent ownership.
typedef void* (*DarwinArtBuildRegisteredNativeThunkV1)(
    void* context,
    const void* android_function,
    const DarwinArtAndroidFunctionOwnerV1* owner,
    const char* shorty,
    uint32_t shorty_length,
    DarwinArtJniCallType call_type);

typedef void (*DarwinArtDestroyRegisteredNativeThunkV1)(void* context,
                                                        void* thunk);

typedef struct DarwinArtRegisteredNativeThunkFactoryV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void* context;
  DarwinArtLookupAndroidFunctionOwnerV1 lookup_owner;
  DarwinArtReleaseAndroidFunctionOwnerV1 release_owner;
  DarwinArtBuildRegisteredNativeThunkV1 build_thunk;
  DarwinArtDestroyRegisteredNativeThunkV1 destroy_thunk;
} DarwinArtRegisteredNativeThunkFactoryV1;

typedef struct DarwinArtRegisteredNativeCache DarwinArtRegisteredNativeCache;

// Allocation failures during create/resolve return null/ERROR. Retirement,
// destruction and size have no error channel and fail-stop on internal failure;
// they must never report successful retirement while resources remain live.

DarwinArtRegisteredNativeCache* darwin_art_registered_native_cache_create(
    const DarwinArtRegisteredNativeThunkFactoryV1* factory);

// Caller must stop cache operations and quiesce all returned callable entries
// before destruction; the cache does not synchronize executing native calls.
void darwin_art_registered_native_cache_destroy(
    DarwinArtRegisteredNativeCache* cache);

// NativeBridge v8 ownership callback equivalent. Only executable addresses in
// a currently live Android ELF image are accepted.
bool darwin_art_is_android_function_pointer(
    DarwinArtRegisteredNativeCache* cache,
    const void* function);

// NativeBridge v7 getTrampolineForFunctionPointer equivalent. The cache key is
// (image_id, generation, function, shorty bytes, JNI call type).
void* darwin_art_get_registered_native_trampoline(
    DarwinArtRegisteredNativeCache* cache,
    const void* android_function,
    const char* shorty,
    uint32_t shorty_length,
    DarwinArtJniCallType call_type);

// Models the exact ART RegisterNatives OR condition. A non-bridged Darwin
// pointer is returned directly; a bridged namespace or owned Android pointer
// must resolve through the trampoline cache or registration fails.
DarwinArtRegisteredNativeResolution darwin_art_resolve_registered_native(
    DarwinArtRegisteredNativeCache* cache,
    const void* function,
    bool class_loader_namespace_is_bridged,
    const char* shorty,
    uint32_t shorty_length,
    DarwinArtJniCallType call_type,
    const void** callable_out);

// Called by the loader when the exact Android ELF image generation is closed.
// ART has no per-method native-bridge release callback; UnregisterNatives only
// resets ArtMethod entrypoints, so cache entries live until image retirement.
// Caller must quiesce published callables before retiring. Pending builders may
// finish but cannot publish this generation afterward. Retired generations stay
// rejected until cache destruction; loader image leases still govern unmapping.
size_t darwin_art_registered_native_cache_retire_image(
    DarwinArtRegisteredNativeCache* cache,
    uint64_t image_id,
    uint64_t generation);

size_t darwin_art_registered_native_cache_size(
    DarwinArtRegisteredNativeCache* cache);

#ifdef __cplusplus
}
#endif

#endif  // DARWIN_ART_REGISTERED_NATIVE_BRIDGE_H_
