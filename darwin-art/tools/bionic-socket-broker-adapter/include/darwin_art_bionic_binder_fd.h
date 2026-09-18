#pragma once

#include "darwin_art_bionic_fd_broker.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Trusted runtime boundary for the single Binder device owner in one Android
 * process. Framework/app code cannot install owners or choose object values. */
DarwinArtFdBrokerStatus darwin_art_bionic_binder_fd_install_owner(
    const DarwinArtFdOwnerV1 *callbacks, DarwinArtFdOwnerHandle *owner);
DarwinArtFdBrokerStatus darwin_art_bionic_binder_fd_publish(
    DarwinArtFdOwnerHandle owner, uint64_t object, int *guest_fd);
DarwinArtFdBrokerStatus darwin_art_bionic_binder_fd_uninstall_owner(
    DarwinArtFdOwnerHandle owner);

/* Narrow lifetime seam for providers which retain a broker Description across
 * an asynchronous transport acknowledgement.  The opaque cookie is an
 * acquired Process reference; the broker remains valid until release. */
int darwin_art_bionic_binder_fd_acquire_process(
    void **process_cookie, DarwinArtFdBroker **broker);
void darwin_art_bionic_binder_fd_release_process(void *process_cookie);

#ifdef __cplusplus
} // extern "C"
#endif
