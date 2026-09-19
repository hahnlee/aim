#ifndef DARWIN_ART_SCM_ENDPOINT_PROVIDER_H_
#define DARWIN_ART_SCM_ENDPOINT_PROVIDER_H_

#include <stdint.h>

// Private runtime ABI, matching engine-sys/scm_endpoint.rs. An installed table
// owns a retained context; every operation/final Description holds its own
// reference. Drain them before context/runtime-image teardown. This contract
// alone does NOT activate managed carriers or private ancillary framing.
enum { DARWIN_ART_SCM_ENDPOINT_ABI_VERSION = 2 };

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

enum { DARWIN_ART_SCM_MAX_PAYLOADS = 16 };

// Only the authenticated daemon reply may create these attributes. Native
// socket identities and receiving-carrier side are never grant inputs.
typedef struct DarwinArtScmGrantV2 {
  uint8_t authority[16];
  uint64_t carrier;
  uint8_t holder[16];
  uint32_t side;
  uint32_t reserved;
} DarwinArtScmGrantV2;

typedef struct DarwinArtScmManagedPayloadV2 {
  uint64_t ordinal;
  uint8_t holder[16];
} DarwinArtScmManagedPayloadV2;

typedef struct DarwinArtScmPrepareRequestV2 {
  uint8_t carrier_holder[16];
  const int *payload_fds; // borrowed through the genuine deposit ACK
  uint32_t payload_count;
  uint32_t managed_count;
  const DarwinArtScmManagedPayloadV2 *managed;
} DarwinArtScmPrepareRequestV2;

typedef struct DarwinArtScmPreparedV2 {
  uint8_t authority[16];
  uint64_t ticket;
  int metadata_fd; // owned by caller on success, -1 on failure
  int guardian_fd; // owned; retain through enqueue, or close on failure
  uint32_t payload_count;
  uint32_t reserved;
} DarwinArtScmPreparedV2;

typedef struct DarwinArtScmAdmitRequestV2 {
  uint8_t carrier_holder[16];
  int metadata_fd; // borrowed received metadata, offset preserved
  uint32_t payload_count; // actual complete received payload group
  const uint64_t *publish_ordinals; // all guest rights selected for publication
  uint32_t publish_count;
  uint32_t reserved;
} DarwinArtScmAdmitRequestV2;

typedef struct DarwinArtScmClaimV2 {
  uint64_t ordinal;
  DarwinArtScmGrantV2 grant;
} DarwinArtScmClaimV2;

typedef struct DarwinArtScmCredentialsV2 {
  int32_t process_id;
  uint32_t user_id;
  uint32_t group_id;
} DarwinArtScmCredentialsV2;

typedef struct DarwinArtScmAdmissionV2 {
  uint8_t authority[16];
  uint64_t ticket;
  uint32_t claim_count;
  uint32_t reserved;
  DarwinArtScmCredentialsV2 credentials;
  DarwinArtScmClaimV2 claims[DARWIN_ART_SCM_MAX_PAYLOADS];
} DarwinArtScmAdmissionV2;

// Admit owns no guest publication. Keep guardian and all output objects
// unpublished until a Finished settlement is confirmed. On later publication
// failure release each fresh holder explicitly; an Aborted settle is then no
// longer valid. Native raw FDs remain caller-owned on every callback failure.
typedef struct DarwinArtScmBinderBindingV2 {
  uint64_t source_connection;
  uint64_t transfer;
  uint64_t ordinal;
  uint64_t object_offset;
} DarwinArtScmBinderBindingV2;

enum { DARWIN_ART_SCM_FINISHED = 1, DARWIN_ART_SCM_ABORTED = 2 };

typedef struct DarwinArtScmEndpointProviderV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void *context;
  void *(*retain)(void *);
  void (*release)(void *);
  int (*register_pair)(void *, const DarwinArtScmPairInstallerV1 *);
  int (*release_holder)(void *, const uint8_t *);
  int (*prepare)(void *, const DarwinArtScmPrepareRequestV2 *, DarwinArtScmPreparedV2 *);
  int (*admit)(void *, const DarwinArtScmAdmitRequestV2 *, DarwinArtScmAdmissionV2 *);
  int (*settle)(void *, const uint8_t *, uint64_t, uint32_t);
  int (*bind_binder)(void *, const uint8_t *, const DarwinArtScmBinderBindingV2 *, uint8_t *);
  int (*cancel_binder)(void *, const DarwinArtScmBinderBindingV2 *);
  int (*claim_binder)(void *, const DarwinArtScmBinderBindingV2 *, const uint8_t *, uint32_t, DarwinArtScmGrantV2 *);
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
static_assert(sizeof(DarwinArtScmGrantV2) == 48);
static_assert(sizeof(DarwinArtScmPrepareRequestV2) == 40);
static_assert(sizeof(DarwinArtScmPreparedV2) == 40);
static_assert(sizeof(DarwinArtScmAdmitRequestV2) == 40);
static_assert(sizeof(DarwinArtScmClaimV2) == 56);
static_assert(sizeof(DarwinArtScmCredentialsV2) == 12);
static_assert(sizeof(DarwinArtScmAdmissionV2) == 944);
static_assert(sizeof(DarwinArtScmPairOfferV1) == 56);
static_assert(sizeof(DarwinArtScmPairInstallerV1) == 32);
static_assert(sizeof(DarwinArtScmEndpointProviderV1) == 96);
#else
_Static_assert(sizeof(DarwinArtScmPairOfferV1) == 56, "pair offer ABI");
_Static_assert(sizeof(DarwinArtScmPairInstallerV1) == 32, "pair installer ABI");
_Static_assert(sizeof(DarwinArtScmEndpointProviderV1) == 96, "provider ABI");
#endif

#endif
