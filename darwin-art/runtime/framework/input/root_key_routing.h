#pragma once
#include "root_key_authority.h"
#include <optional>

namespace darwin_art::input {
// Construct/pin outside input locks and retain until domain/authority/channel
// guards have unlocked. Expired authority is terminal rejection, not fallback.
struct RootKeyRoutingPin final {
  explicit RootKeyRoutingPin(const RootKeyRoutingFence& value) noexcept;
  RootKeyRoutingFence fence;
  RootKeyAuthorityHandle authority;
  InputRoutingHandle route;
  bool required = false;
};
class RootKeyRoutingGuard final {
 public:
  RootKeyRoutingGuard(InputRoutingDomainTransaction&, const RootKeyRoutingPin&) noexcept;
  bool ValidateRoot(const InputRoutingHandle&) const noexcept;
  // Requires the routing channel mutex, acquired AFTER this root guard.
  bool ValidateReadiness(const InputFocusEpochEvaluator::ChannelCache&,
                        const InputRoutingHandle&,
                        const InputRoutingRecipientHandle&) const noexcept;
 private:
  InputRoutingDomainTransaction& domain_;
  const RootKeyRoutingPin& pin_;
  std::optional<RootKeyAuthorityGuard> authority_guard_;
};
// Explicit exact-root proposal. No process/global/current-ready-channel lookup.
darwin_art::DarwinArtInputEnqueueResult RouteRootFrameworkKeyPacket(
    const RootKeyAuthorityHandle&, const DarwinArtKeyEventV1&, InputRoutingAdmission*,
    const DecisionRecordHandle& expected_record = {});
// Classify a retained admission before a capacity retry. Invalid immutable
// fences are terminal; this never constructs a replacement admission.
bool ValidateRootKeyAdmissionForRetry(const InputRoutingAdmission&) noexcept;
}  // namespace darwin_art::input
