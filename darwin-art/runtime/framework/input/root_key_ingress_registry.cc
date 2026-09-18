#include "root_key_ingress_registry.h"

#include "root_key_ingress_lifetime.h"

#include <algorithm>
#include <mutex>
#include <new>
#include <unordered_map>
#include <utility>
#include <vector>

namespace darwin_art::input {
namespace {

template <typename T, typename U>
bool SameWeakOwner(const std::weak_ptr<T>& left,
                   const std::weak_ptr<U>& right) noexcept {
  return !left.owner_before(right) && !right.owner_before(left);
}

template <typename T, typename U>
bool SameWeakOwner(const std::weak_ptr<T>& left,
                   const std::shared_ptr<U>& right) noexcept {
  const std::weak_ptr<U> candidate = right;
  return SameWeakOwner(left, candidate);
}

struct CanonicalEntry final {
  std::weak_ptr<RootKeyAuthority> authority;
  void* owner_looper = nullptr;
  RootKeyIngress::SubmitPort submit_port = nullptr;
  std::weak_ptr<RootKeyIngress> facade;
  std::weak_ptr<RootKeyIngressLifetime> lifetime;
  bool building = false;
};

struct Registry final {
  std::mutex mutex;
  bool closed = false;
  size_t factories = 0;
  std::unordered_map<RootKeyAuthority*, CanonicalEntry> canonical;
  // Strong pins retain failed starts and retired timer/task ledgers until
  // their actual provider quiescence is observed by Poll.
  std::vector<RootKeyIngressLifetimeHandle> lifetimes;
};

Registry& GlobalRegistry() {
  static Registry registry;
  return registry;
}

void PruneQuietLifetimes() noexcept {
  auto& registry = GlobalRegistry();
  std::vector<RootKeyIngressLifetimeHandle> pins;
  try {
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      pins = registry.lifetimes;
    }
    std::vector<RootKeyIngressLifetimeHandle> quiet;
    quiet.reserve(pins.size());
    for (const auto& lifetime : pins)
      if (IsRootKeyIngressLifetimeQuiescent(lifetime)) quiet.push_back(lifetime);

    std::vector<RootKeyIngressLifetimeHandle> retired;
    retired.reserve(pins.size());
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      // Remove quiet entries in place, moving their strong pins to a
      // pre-reserved outer vector. Moved-from handles are destroyed under the
      // lock, but never retain provider/task resources.
      for (size_t i = registry.lifetimes.size(); i != 0; --i) {
        auto& lifetime = registry.lifetimes[i - 1];
        if (std::find(quiet.begin(), quiet.end(), lifetime) != quiet.end()) {
          retired.push_back(std::move(lifetime));
          registry.lifetimes.erase(registry.lifetimes.begin() + (i - 1));
        }
      }
      for (auto it = registry.canonical.begin(); it != registry.canonical.end();) {
        const bool quiet_lifetime = std::any_of(
            quiet.begin(), quiet.end(), [&](const auto& lifetime) {
              return SameWeakOwner(it->second.lifetime, lifetime);
            });
        if (!it->second.building && it->second.facade.expired() &&
            (it->second.lifetime.expired() || quiet_lifetime))
          it = registry.canonical.erase(it);
        else
          ++it;
      }
    }
    // Release provider/context-bearing pins after the registry mutex is gone.
    retired.clear();
  } catch (...) {
    // Pruning is opportunistic. A later process poll retries it.
  }
}

struct FactoryAdmission final {
  Registry& registry;
  ~FactoryAdmission() {
    std::lock_guard<std::mutex> lock(registry.mutex);
    if (registry.factories != 0) --registry.factories;
  }
};

}  // namespace

RootKeyIngressHandle AcquireCanonicalRootKeyIngress(
    const RootKeyAuthorityHandle& authority, void* owner_looper,
    RootKeyIngress::SubmitPort submit_port) noexcept {
  if (authority == nullptr || owner_looper == nullptr || submit_port == nullptr)
    return {};
  auto& registry = GlobalRegistry();
  try {
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      if (registry.closed) return {};
      ++registry.factories;
    }
    FactoryAdmission factory{registry};
    PruneQuietLifetimes();

    // Resolve an existing canonical facade outside the registry lock, while
    // the factory admission keeps all temporary pins within shutdown scope.
    CanonicalEntry existing_entry;
    bool has_existing = false;
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      if (registry.closed) return {};
      const auto found = registry.canonical.find(authority.get());
      if (found != registry.canonical.end()) {
        if (found->second.building) return {};
        existing_entry = found->second;
        has_existing = true;
      }
    }
    if (has_existing) {
      if (!SameWeakOwner(existing_entry.authority, authority)) {
        // A stale raw-address entry is reusable only after its old weak
        // authority/facade/lifetime metadata has expired and was pruned.
        return {};
      }
      const auto existing = existing_entry.facade.lock();
      if (existing == nullptr) return {};
      if (existing_entry.owner_looper != owner_looper ||
          existing_entry.submit_port != submit_port ||
          IsRootKeyIngressLifetimeClosed(existing_entry.lifetime.lock()))
        return {};
      std::lock_guard<std::mutex> lock(registry.mutex);
      const auto found = registry.canonical.find(authority.get());
      if (registry.closed || found == registry.canonical.end() ||
          found->second.building || found->second.owner_looper != owner_looper ||
          found->second.submit_port != submit_port ||
          !SameWeakOwner(found->second.authority, authority) ||
          !SameWeakOwner(found->second.facade, existing))
        return {};
      return existing;
    }

    // Preparation allocates only inert State/facade/lifetime objects.
    const auto prepared = PrepareRootKeyIngress(authority, owner_looper, submit_port);
    if (prepared.facade == nullptr || prepared.lifetime == nullptr) return {};

    // Reserve the exact canonical identity and strong lifetime before any
    // task/provider publication. A concurrent factory sees building=true.
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      if (registry.closed) return {};
      if (registry.canonical.find(authority.get()) != registry.canonical.end())
        return {};
      registry.lifetimes.push_back(prepared.lifetime);
      registry.canonical.emplace(authority.get(), CanonicalEntry{
          authority, owner_looper, submit_port, {}, prepared.lifetime, true});
    }

    if (!StartRootKeyIngress(prepared.lifetime, authority)) {
      // The failed/closed lifetime remains in the strong ledger. Remove only
      // the building reservation; Poll releases it after real quiescence.
      std::lock_guard<std::mutex> lock(registry.mutex);
      const auto found = registry.canonical.find(authority.get());
      if (found != registry.canonical.end() && found->second.building &&
          SameWeakOwner(found->second.lifetime, prepared.lifetime))
        registry.canonical.erase(found);
      return {};
    }

    // Final publication rechecks global shutdown and exact reservation
    // identity after Start's provider/task work has completed.
    bool publish = false;
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      const auto found = registry.canonical.find(authority.get());
      if (registry.closed || found == registry.canonical.end() ||
          !found->second.building ||
          !SameWeakOwner(found->second.lifetime, prepared.lifetime)) {
        // Close outside registry lock below; leave ledger discoverable.
      } else {
        publish = true;
      }
    }
    if (publish && !IsRootKeyIngressLifetimeClosed(prepared.lifetime)) {
      std::lock_guard<std::mutex> lock(registry.mutex);
      const auto found = registry.canonical.find(authority.get());
      if (!registry.closed && found != registry.canonical.end() &&
          found->second.building &&
          SameWeakOwner(found->second.lifetime, prepared.lifetime)) {
        found->second.facade = prepared.facade;
        found->second.building = false;
        return prepared.facade;
      }
    }
    CloseRootKeyIngressLifetime(prepared.lifetime);
    return {};
  } catch (...) {
    // The prepared facade closes its lifetime while unwinding, before the
    // factory guard releases process admission.
    return {};
  }
}

bool CloseRootKeyIngressAdmission() noexcept {
  auto& registry = GlobalRegistry();
  std::vector<RootKeyIngressLifetimeHandle> pins;
  try {
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      registry.closed = true;
      pins = registry.lifetimes;
    }
    for (const auto& lifetime : pins) CloseRootKeyIngressLifetime(lifetime);
    return true;
  } catch (...) {
    return false;
  }
}

bool PollRootKeyIngressQuiesced() noexcept {
  auto& registry = GlobalRegistry();
  try {
    std::vector<RootKeyIngressLifetimeHandle> pins;
    size_t factories = 0;
    {
      std::lock_guard<std::mutex> lock(registry.mutex);
      if (!registry.closed) return false;
      pins = registry.lifetimes;
      factories = registry.factories;
    }
    bool quiet = factories == 0;
    for (const auto& lifetime : pins)
      quiet = IsRootKeyIngressLifetimeQuiescent(lifetime) && quiet;
    if (!quiet) return false;
    PruneQuietLifetimes();
    return true;
  } catch (...) {
    return false;
  }
}

}  // namespace darwin_art::input
