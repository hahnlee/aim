#pragma once

#include "aim_bionic_fd_broker.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Trusted runtime boundary for the single Binder device owner in one Android
 * process. Framework/app code cannot install owners or choose object values. */
AimFdBrokerStatus aim_bionic_binder_fd_install_owner(
    const AimFdOwnerV1 *callbacks, AimFdOwnerHandle *owner);
AimFdBrokerStatus aim_bionic_binder_fd_publish(
    AimFdOwnerHandle owner, uint64_t object, int *guest_fd);
AimFdBrokerStatus aim_bionic_binder_fd_uninstall_owner(
    AimFdOwnerHandle owner);

/* Narrow lifetime seam for providers which retain a broker Description across
 * an asynchronous transport acknowledgement.  The opaque cookie is an
 * acquired Process reference; the broker remains valid until release. */
int aim_bionic_binder_fd_acquire_process(
    void **process_cookie, AimFdBroker **broker);
void aim_bionic_binder_fd_release_process(void *process_cookie);

#ifdef __cplusplus
} // extern "C"
#endif
