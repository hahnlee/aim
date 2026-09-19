#pragma once
#include "../bionic-socket-broker-adapter/src/scm_endpoint_provider.h"
#include <atomic>
#include <cerrno>
#include <cstring>

// Test-only registration fixture. It exercises production native ownership,
// not daemon authentication/custody; rights-bearing operations fail explicitly.
namespace darwin_art::test {
struct ScmPairFixture {
  std::atomic<int> references{1};
  std::atomic<uint64_t> next{1};
  static void *Retain(void *p) { ++static_cast<ScmPairFixture *>(p)->references; return p; }
  static void Release(void *p) { --static_cast<ScmPairFixture *>(p)->references; }
  static int Register(void *p, const DarwinArtScmPairInstallerV1 *installer) {
    if (installer == nullptr || installer->install == nullptr) return EINVAL;
    const auto id = static_cast<ScmPairFixture *>(p)->next.fetch_add(1);
    DarwinArtScmPairOfferV1 offer{};
    offer.authority[0] = 0x71;
    offer.carrier = id;
    const uint64_t a = id * 2, b = a + 1;
    std::memcpy(offer.holder_a, &a, sizeof(a));
    std::memcpy(offer.holder_b, &b, sizeof(b));
    return installer->install(installer->target, &offer);
  }
  static int ReleaseHolder(void *, const uint8_t *) { return 0; }
  static int Prepare(void *, const DarwinArtScmPrepareRequestV2 *, DarwinArtScmPreparedV2 *) { return ENOSYS; }
  static int Admit(void *, const DarwinArtScmAdmitRequestV2 *, DarwinArtScmAdmissionV2 *) { return ENOSYS; }
  static int Settle(void *, const uint8_t *, uint64_t, uint32_t) { return ENOSYS; }
  static int Bind(void *, const uint8_t *, const DarwinArtScmBinderBindingV2 *, uint8_t *) { return ENOSYS; }
  static int Cancel(void *, const DarwinArtScmBinderBindingV2 *) { return ENOSYS; }
  static int Claim(void *, const DarwinArtScmBinderBindingV2 *, const uint8_t *, uint32_t, DarwinArtScmGrantV2 *) { return ENOSYS; }
  int Install() {
    const DarwinArtScmEndpointProviderV1 table{
        DARWIN_ART_SCM_ENDPOINT_ABI_VERSION, sizeof(table), this, Retain, Release,
        Register, ReleaseHolder, Prepare, Admit, Settle, Bind, Cancel, Claim};
    return darwin_art_bionic_install_scm_endpoint_provider(&table);
  }
};
}
