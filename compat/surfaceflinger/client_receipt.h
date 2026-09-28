#pragma once
#include "commit_receipt.h"
namespace aim::surfaceflinger {
// Called only after a publication attempt. Failure to obtain a complete valid
// receipt is Unknown, never a safe rejection. Import consumes the native pipe
// FD on every outcome (matching the bionic SCM broker), including failure.
AimSurfaceFlingerReceipt ReadClientReceipt(
    int socket_fd, int (*import_descriptor)(int));
}
