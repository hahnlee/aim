#pragma once

#include "root_key_ingress.h"

namespace darwin_art::input {

// Canonical per-authority factory. Repeated installation reuses one ingress;
// a different owner looper or submit port is rejected.
RootKeyIngressHandle AcquireCanonicalRootKeyIngress(
    const RootKeyAuthorityHandle& authority, void* owner_looper,
    RootKeyIngress::SubmitPort submit_port) noexcept;

// Process teardown ports. Close admits shutdown and retains inert lifetime
// ledgers until Poll observes real task/timer/context quiescence.
bool CloseRootKeyIngressAdmission() noexcept;
bool PollRootKeyIngressQuiesced() noexcept;

}  // namespace darwin_art::input
