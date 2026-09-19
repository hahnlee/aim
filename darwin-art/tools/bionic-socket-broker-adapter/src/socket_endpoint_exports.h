#ifndef DARWIN_ART_SOCKET_ENDPOINT_EXPORTS_H_
#define DARWIN_ART_SOCKET_ENDPOINT_EXPORTS_H_

#include "darwin_art_bionic_fd_broker.h"
#include "scm_endpoint_provider.h"
#include <cstddef>

namespace darwin_art::bionic::scm {
// Trusted adapter boundary. The acquired Process cookie and exact Description
// pin must remain alive for this call. Returns 1 and one owned provider context
// for a managed socket, 0 for an ordinary description, or a negative errno.
// No descriptor lookup, native socket identity or inferred authority is used.
int RetainExportedEndpoint(void *process_cookie,
                           const DarwinArtFdDescriptionSnapshotV1 &snapshot,
                           DarwinArtScmGrantV2 *attributes,
                           DarwinArtScmEndpointProviderV1 *provider) noexcept;
// Consumes host_fd on every outcome. Attributes must be Binder delegation
// bytes from the immutable transport image, never native socket identity.
int ImportBinderEndpoint(int host_fd, const DarwinArtScmBinderBindingV2 &binding,
                         const uint8_t *attributes, std::size_t length) noexcept;
}

#endif
