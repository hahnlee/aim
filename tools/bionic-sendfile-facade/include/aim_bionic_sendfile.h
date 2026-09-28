#ifndef AIM_BIONIC_SENDFILE_H_
#define AIM_BIONIC_SENDFILE_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { AIM_BIONIC_SENDFILE_ABI_VERSION = 1 };

typedef struct AimBionicSendfileRequest {
  uint32_t abi_version;
  int32_t output_fd;
  int32_t input_fd;
  uint32_t has_explicit_offset;
  int64_t offset;
  size_t count;
} AimBionicSendfileRequest;

typedef struct AimBionicSendfileResult {
  uint32_t abi_version;
  int32_t android_errno;
  intptr_t transferred;
  int64_t next_offset;
} AimBionicSendfileResult;

typedef enum AimBionicSendfileTransferStatus {
  AIM_BIONIC_SENDFILE_TRANSFER_OK = 0,
  AIM_BIONIC_SENDFILE_TRANSFER_BAD_FD = 1,
  AIM_BIONIC_SENDFILE_TRANSFER_UNAVAILABLE = 2,
} AimBionicSendfileTransferStatus;

/* The owner performs one atomic virtual-fd transfer. A successful partial
 * result is not an error. With an explicit offset it must leave the input
 * descriptor position unchanged and return offset+transferred. */
typedef AimBionicSendfileTransferStatus (*AimBionicSendfileTransfer)(
    void* context, const AimBionicSendfileRequest* request,
    AimBionicSendfileResult* result);

typedef enum AimBionicSendfileLifecycleStatus {
  AIM_BIONIC_SENDFILE_LIFECYCLE_OK = 0,
  AIM_BIONIC_SENDFILE_LIFECYCLE_INVALID_ARGUMENT = 1,
  AIM_BIONIC_SENDFILE_LIFECYCLE_ALREADY_ACTIVE = 2,
} AimBionicSendfileLifecycleStatus;

typedef void (*AimBionicSendfileFunction)(void);

AimBionicSendfileLifecycleStatus aim_bionic_sendfile_activate(
    AimBionicSendfileTransfer transfer, void* context);
/* Stops admission and waits for every in-flight transfer callback. */
AimBionicSendfileLifecycleStatus aim_bionic_sendfile_deactivate(void);

intptr_t aim_bionic_sendfile(int output_fd, int input_fd,
                                    int64_t* offset, size_t count);
AimBionicSendfileFunction aim_bionic_sendfile_resolve(
    const char* soname, const char* symbol, const char* version);

#ifdef __cplusplus
}
#endif

#endif
