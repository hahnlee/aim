#pragma once

#include "fd_transport.h"

namespace darwin_art::binder {

// The implementation owns the Process reference and (for central Binder
// descriptors) the exact broker Description pin.  The returned host FD is
// transferred to the caller, while the opaque lease remains live until the
// genuine Binder deposit acknowledgement.
int ExportRetainedFileDescriptor(
    int guest_fd, const DarwinArtBinderTransferBinding *binding,
    DarwinArtBinderRetainedExportedDescriptor *result) noexcept;
void ReleaseRetainedFileDescriptorLease(void *lease) noexcept;

} // namespace darwin_art::binder
