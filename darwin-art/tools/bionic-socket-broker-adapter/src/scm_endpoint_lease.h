#ifndef DARWIN_ART_SCM_ENDPOINT_LEASE_H_
#define DARWIN_ART_SCM_ENDPOINT_LEASE_H_

#include "scm_endpoint_provider.h"
#include <array>
#include <cerrno>
#include <cstdio>
#include <cstring>

namespace darwin_art::bionic::scm {

// Inline exact-Description state. Empty ordinary descriptors do no RPC.
// Final object close MUST run outside broker/inheritance locks while the
// runtime image and retained context are still alive.
class EndpointLease final {
public:
  struct Attributes {
    std::array<uint8_t, 16> authority;
    uint64_t carrier;
    std::array<uint8_t, 16> holder;
    uint32_t side;
  };
  EndpointLease() noexcept = default;
  EndpointLease(const EndpointLease &) = delete;
  EndpointLease &operator=(const EndpointLease &) = delete;
  ~EndpointLease() noexcept { Reset(); }
  const Attributes *attributes() const noexcept { return installed_ ? &attributes_ : nullptr; }

private:
  friend class PairInstallReceipt;
  void Set(const DarwinArtScmEndpointProviderV1 &provider, void *retained,
           const DarwinArtScmPairOfferV1 &offer, uint32_t side) noexcept {
    provider_ = provider;
    provider_.context = retained;
    std::memcpy(attributes_.authority.data(), offer.authority, 16);
    attributes_.carrier = offer.carrier;
    std::memcpy(attributes_.holder.data(), side == 0 ? offer.holder_a : offer.holder_b, 16);
    attributes_.side = side;
    installed_ = true;
    owns_grant_ = false;
  }
  void Reset() noexcept {
    if (!installed_) return;
    const int saved_errno = errno;
    const auto provider = provider_;
    const auto holder = attributes_.holder;
    const bool release_grant = owns_grant_;
    // Detach before any callback; reentry cannot repeat cleanup.
    installed_ = false;
    owns_grant_ = false;
    provider_ = {};
    attributes_ = {};
    if (release_grant) {
      const int status = provider.release_holder(provider.context, holder.data());
      if (status != 0) std::fprintf(stderr, "SCM endpoint final release failed: %d\n", status);
    }
    provider.release(provider.context);
    errno = saved_errno;
  }
  Attributes attributes_{};
  DarwinArtScmEndpointProviderV1 provider_{};
  bool installed_ = false;
  bool owns_grant_ = false;
};

// Preallocated caller-owned receipt. Targets remain unpublished AND alive
// through Register/Commit/destruction. Do not attach it to the legacy partial
// socketpair publication path: atomic native slot reservation is still needed.
class PairInstallReceipt final {
public:
  PairInstallReceipt(EndpointLease &first, EndpointLease &second,
                     const DarwinArtScmEndpointProviderV1 &provider) noexcept
      : first_(first), second_(second), provider_(provider) {
    if (Valid(provider_)) provider_.context = provider_.retain(provider_.context);
    else provider_.context = nullptr;
  }
  PairInstallReceipt(const PairInstallReceipt &) = delete;
  PairInstallReceipt &operator=(const PairInstallReceipt &) = delete;
  ~PairInstallReceipt() noexcept {
    const int saved_errno = errno;
    if (armed_) Clear();
    if (provider_.context != nullptr) provider_.release(provider_.context);
    errno = saved_errno;
  }
  int Register() noexcept {
    if (provider_.context == nullptr || armed_ || confirmed_ ||
        first_.installed_ || second_.installed_ || &first_ == &second_) return EINVAL;
    const DarwinArtScmPairInstallerV1 installer{
        DARWIN_ART_SCM_ENDPOINT_ABI_VERSION, sizeof(DarwinArtScmPairInstallerV1),
        this, &Install, &Rollback};
    const int status = provider_.register_pair(provider_.context, &installer);
    if (status != 0) {
      // Rust owns grant cleanup on registration failure, including ambiguous
      // confirmation. This clears native attrs/context references only.
      Clear();
      return status;
    }
    if (!armed_ || !first_.installed_ || !second_.installed_) return EPROTO;
    first_.owns_grant_ = true;
    second_.owns_grant_ = true;
    confirmed_ = true;
    return 0;
  }
  // Called ONLY after actual complete guest publication. Failure leaves this
  // receipt armed, so it closes BOTH grant owners rather than a valid prefix.
  bool Commit() noexcept {
    if (!confirmed_ || !armed_) return false;
    armed_ = false;
    return true;
  }

private:
  static bool Valid(const DarwinArtScmEndpointProviderV1 &provider) noexcept {
    return provider.abi_version == DARWIN_ART_SCM_ENDPOINT_ABI_VERSION &&
           provider.struct_size == sizeof(provider) && provider.context != nullptr &&
           provider.retain != nullptr && provider.release != nullptr &&
           provider.register_pair != nullptr && provider.release_holder != nullptr;
  }
  static bool Nonzero(const uint8_t *bytes) noexcept {
    for (size_t index = 0; index < 16; ++index) if (bytes[index] != 0) return true;
    return false;
  }
  static int Install(void *target, const DarwinArtScmPairOfferV1 *offer) noexcept {
    auto &self = *static_cast<PairInstallReceipt *>(target);
    if (offer == nullptr || self.armed_ || self.first_.installed_ || self.second_.installed_ ||
        offer->carrier == 0 || !Nonzero(offer->authority) || !Nonzero(offer->holder_a) ||
        !Nonzero(offer->holder_b) || std::memcmp(offer->holder_a, offer->holder_b, 16) == 0) return EINVAL;
    // All storage preallocated. Actual Rust retain is allocation-free Arc.
    void *first = self.provider_.retain(self.provider_.context);
    if (first == nullptr) return ENOMEM;
    void *second = self.provider_.retain(self.provider_.context);
    if (second == nullptr) { self.provider_.release(first); return ENOMEM; }
    self.first_.Set(self.provider_, first, *offer, 0);
    self.second_.Set(self.provider_, second, *offer, 1);
    self.armed_ = true;
    return 0;
  }
  static void Rollback(void *target) noexcept { static_cast<PairInstallReceipt *>(target)->Clear(); }
  void Clear() noexcept {
    if (!armed_) return;
    // Disarm before callbacks: final grant release can reenter native code.
    // A nested rollback must not clean up either exact object a second time.
    armed_ = false;
    first_.Reset();
    second_.Reset();
  }
  EndpointLease &first_;
  EndpointLease &second_;
  DarwinArtScmEndpointProviderV1 provider_;
  bool armed_ = false;
  bool confirmed_ = false;
};
} // namespace darwin_art::bionic::scm
#endif
