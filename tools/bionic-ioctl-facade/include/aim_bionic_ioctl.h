#ifndef AIM_BIONIC_IOCTL_H_
#define AIM_BIONIC_IOCTL_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { AIM_BIONIC_IOCTL_FD_INFO_ABI_VERSION = 1 };

typedef enum AimBionicIoctlFdKind {
  AIM_BIONIC_IOCTL_FD_OTHER = 0,
  AIM_BIONIC_IOCTL_FD_RANDOM_DEVICE = 1,
} AimBionicIoctlFdKind;

typedef struct AimBionicIoctlFdInfo {
  uint32_t abi_version;
  AimBionicIoctlFdKind kind;
} AimBionicIoctlFdInfo;

typedef enum AimBionicIoctlFdLookupStatus {
  AIM_BIONIC_IOCTL_FD_FOUND = 0,
  AIM_BIONIC_IOCTL_FD_BAD = 1,
  AIM_BIONIC_IOCTL_FD_CAPABILITY_UNAVAILABLE = 2,
} AimBionicIoctlFdLookupStatus;

typedef AimBionicIoctlFdLookupStatus (*AimBionicIoctlFdLookup)(
    void* context, int32_t fd, AimBionicIoctlFdInfo* info);
/* The callback and its context remain valid until deactivate returns. It must
 * not invoke the activation/deactivation lifecycle recursively. */

typedef enum AimBionicIoctlLifecycleStatus {
  AIM_BIONIC_IOCTL_LIFECYCLE_OK = 0,
  AIM_BIONIC_IOCTL_LIFECYCLE_INVALID_ARGUMENT = 1,
  AIM_BIONIC_IOCTL_LIFECYCLE_ALREADY_ACTIVE = 2,
} AimBionicIoctlLifecycleStatus;

typedef void (*AimBionicIoctlFunction)(void);

AimBionicIoctlLifecycleStatus aim_bionic_ioctl_activate(
    AimBionicIoctlFdLookup lookup, void* context);
/* Stops new calls and waits until every in-flight lookup has returned. */
AimBionicIoctlLifecycleStatus aim_bionic_ioctl_deactivate(void);

int aim_bionic_ioctl(int fd, int request, ...);
AimBionicIoctlFunction aim_bionic_ioctl_resolve(
    const char* soname, const char* symbol, const char* version);
const char* aim_bionic_ioctl_capability(const char* capability);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // AIM_BIONIC_IOCTL_H_
