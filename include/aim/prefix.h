#ifndef AIM_PREFIX_H_
#define AIM_PREFIX_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimPrefix AimPrefix;

typedef enum AimPrefixResult {
  AIM_PREFIX_OK = 0,
  AIM_PREFIX_INVALID_ARGUMENT = 1,
  AIM_PREFIX_INVALID_PATH = 2,
  AIM_PREFIX_DUPLICATE_MOUNT = 3,
  AIM_PREFIX_TABLE_SEALED = 4,
  AIM_PREFIX_TABLE_NOT_SEALED = 5,
  AIM_PREFIX_NO_MOUNT = 6,
  AIM_PREFIX_BUFFER_TOO_SMALL = 7,
} AimPrefixResult;

typedef enum AimPrefixMountKind {
  AIM_PREFIX_IMMUTABLE = 1,
  AIM_PREFIX_PRIVATE = 2,
  AIM_PREFIX_SHARED = 3,
  AIM_PREFIX_SYNTHETIC = 4,
} AimPrefixMountKind;

typedef struct AimPrefixResolution {
  uint32_t mount_id;
  uint32_t mount_kind;
  bool writable;
  bool requires_directory;
  size_t normalized_path_length;
  size_t relative_path_length;
} AimPrefixResolution;

AimPrefix* aim_prefix_create(void);
void aim_prefix_destroy(AimPrefix* prefix);

AimPrefixResult aim_prefix_add_mount(
    AimPrefix* prefix,
    uint32_t mount_id,
    AimPrefixMountKind kind,
    bool writable,
    const char* guest_prefix);

AimPrefixResult aim_prefix_seal(AimPrefix* prefix);

// Resolves an Android byte pathname without touching the host filesystem.
// Construction is single-threaded. After seal, resolve calls may run
// concurrently, but add/seal/destroy require all calls to be quiescent.
//
// This is only a namespace routing result, not a filesystem authorization.
// The returned relative path must be walked component-by-component beneath the
// pre-authorized directory FD for mount_id. Callers must preserve
// requires_directory, apply the final-component symlink policy, and must not
// concatenate this value with an unchecked host path.
AimPrefixResult aim_prefix_resolve(
    const AimPrefix* prefix,
    const char* cwd,
    const char* path,
    AimPrefixResolution* resolution,
    char* normalized_path,
    size_t normalized_path_capacity,
    char* relative_path,
    size_t relative_path_capacity);

#ifdef __cplusplus
}
#endif

#endif  // AIM_PREFIX_H_
