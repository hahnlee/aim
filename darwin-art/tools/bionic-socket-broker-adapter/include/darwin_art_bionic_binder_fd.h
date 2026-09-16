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

#ifdef __cplusplus
} // extern "C"
#endif
