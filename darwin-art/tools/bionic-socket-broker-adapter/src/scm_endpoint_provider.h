#ifndef DARWIN_ART_SCM_ENDPOINT_PROVIDER_H_
#define DARWIN_ART_SCM_ENDPOINT_PROVIDER_H_

#include <stdint.h>

// Private runtime ABI, matching engine-sys/scm_endpoint.rs. An installed table
// owns a retained context; every operation/final Description holds its own
// reference. Drain them before context/runtime-image teardown. This contract
// alone does NOT activate managed carriers or private ancillary framing.
enum { DARWIN_ART_SCM_ENDPOINT_ABI_VERSION = 1 };

typedef struct DarwinArtScmPairOfferV1 {
  uint8_t authority[16];
  uint64_t carrier;
  uint8_t holder_a[16];
  uint8_t holder_b[16];
} DarwinArtScmPairOfferV1;

// Real pair owner preallocates both unpublished objects and rollback receipt.
// Install changes both exact objects without allocation/RPC, then receipt may
// be sent. Clear removes partial attrs WITHOUT daemon grant release: Rust owns
// grants until confirmed success. Native receipt retains rollback through
// publication and takes final endpoint-grant cleanup ownership only on success.
typedef struct DarwinArtScmPairInstallerV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void *target;
  int (*install)(void *, const DarwinArtScmPairOfferV1 *);
  void (*clear)(void *);
} DarwinArtScmPairInstallerV1;

typedef struct DarwinArtScmEndpointProviderV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void *context;
  void *(*retain)(void *);
  void (*release)(void *);
  int (*register_pair)(void *, const DarwinArtScmPairInstallerV1 *);
  int (*release_holder)(void *, const uint8_t *);
} DarwinArtScmEndpointProviderV1;

#ifdef __cplusplus
extern "C" {
#endif
// Owner-thread installation/teardown: 0 or positive native errno.
// Uninstall stops new acquisition even when it reports EBUSY. Retry only
// after native callers/endpoint owners have genuinely quiesced.
int darwin_art_bionic_install_scm_endpoint_provider(const DarwinArtScmEndpointProviderV1 *);
int darwin_art_bionic_uninstall_scm_endpoint_provider(void);
#ifdef __cplusplus
}
namespace darwin_art::bionic::scm {
// On success caller owns ONE wrapper context reference. Release it with the
// returned table's release callback. No operation permits image unload before
// actual native caller quiescence, even if its reference count reaches zero.
int AcquireProvider(DarwinArtScmEndpointProviderV1 *owned) noexcept;
}
#endif

#ifdef __cplusplus
static_assert(sizeof(DarwinArtScmPairOfferV1) == 56);
static_assert(sizeof(DarwinArtScmPairInstallerV1) == 32);
static_assert(sizeof(DarwinArtScmEndpointProviderV1) == 48);
#else
_Static_assert(sizeof(DarwinArtScmPairOfferV1) == 56, "pair offer ABI");
_Static_assert(sizeof(DarwinArtScmPairInstallerV1) == 32, "pair installer ABI");
_Static_assert(sizeof(DarwinArtScmEndpointProviderV1) == 48, "provider ABI");
#endif

#endif
