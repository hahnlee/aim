#pragma once

#include "root_key_ingress.h"

#include <memory>

namespace darwin_art::input {

// Opaque lifetime/admission pin. Registry code may retain and close this
// object, but cannot inspect or mutate ingress queue state.
class RootKeyIngressLifetime;
using RootKeyIngressLifetimeHandle = std::shared_ptr<RootKeyIngressLifetime>;

struct PreparedRootKeyIngress final {
  RootKeyIngressHandle facade;
  RootKeyIngressLifetimeHandle lifetime;
};

// Preparation is inert: no looper task, subscription, wake, or provider
// publication occurs until StartRootKeyIngress is called by the registry.
PreparedRootKeyIngress PrepareRootKeyIngress(
    const RootKeyAuthorityHandle& authority, void* owner_looper,
    RootKeyIngress::SubmitPort submit_port) noexcept;

// Start owns the complete admitted initialization scope. On failure it closes
// the opaque lifetime; the registry may retain it until provider quiescence.
bool StartRootKeyIngress(const RootKeyIngressLifetimeHandle& lifetime,
                         const RootKeyAuthorityHandle& authority) noexcept;
void CloseRootKeyIngressLifetime(
    const RootKeyIngressLifetimeHandle& lifetime) noexcept;
bool IsRootKeyIngressLifetimeClosed(
    const RootKeyIngressLifetimeHandle& lifetime) noexcept;
bool IsRootKeyIngressLifetimeQuiescent(
    const RootKeyIngressLifetimeHandle& lifetime) noexcept;

}  // namespace darwin_art::input
