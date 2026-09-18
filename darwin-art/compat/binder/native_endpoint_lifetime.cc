#include "native_endpoint_lifetime.h"

#include "android_util_Binder.h"
#include <binder/BpBinder.h>
#include <binder/IBinder.h>
#include <binder/ProcessState.h>
#include <utils/Errors.h>

#include <atomic>
#include <cstdint>
#include <limits>
#include <map>
#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::binder {
namespace {
class NativeEndpoint;
struct ConnectionAuthority;
struct Catalog final {
  std::mutex mutex;
  bool installed = false;
  bool closed = false;
  std::atomic<size_t> factories{0}, listeners{0}, connections{0}, retirements{0};
  std::atomic<uint64_t> next_generation{1};
  // Ledger and lock outlive the owned resources during static destruction.
  std::shared_ptr<ConnectionAuthority> connection;
  std::map<android::IBinder*, std::shared_ptr<NativeEndpoint>> nodes;
};
Catalog& State() { static Catalog state; return state; }

class FactoryAdmission final {
 public:
  explicit FactoryAdmission(bool install = false) noexcept {
    auto& state = State();
    std::lock_guard lock(state.mutex);
    if (state.closed || (install && state.installed)) return;
    if (install) state.installed = true;
    state.factories.fetch_add(1, std::memory_order_relaxed);
    admitted_ = true;
  }
  ~FactoryAdmission() {
    if (admitted_) State().factories.fetch_sub(1, std::memory_order_release);
  }
  explicit operator bool() const noexcept { return admitted_; }
 private:
  bool admitted_ = false;
};

struct ConnectionAuthority final {
  explicit ConnectionAuthority(const darwin_art_binder_authority_hooks_t& hooks) noexcept
      : retained(hooks.retain(hooks.context)), live(hooks.live), release(hooks.release) {
    State().connections.fetch_add(1, std::memory_order_relaxed);
  }
  ~ConnectionAuthority() {
    Close();
    Retire();
  }
  bool Live() const noexcept {
    auto value = gate.load(std::memory_order_acquire);
    while ((value & kClosed) == 0 && value != kClosed - 1) {
      if (gate.compare_exchange_weak(value, value + 1, std::memory_order_acq_rel)) {
        const bool result = retained != nullptr && live(retained) == 1;
        if (!result) gate.fetch_or(kClosed, std::memory_order_acq_rel);
        const auto before = gate.fetch_sub(1, std::memory_order_acq_rel);
        return result && (before & kClosed) == 0;
      }
    }
    return false;
  }
  void Close() noexcept { gate.fetch_or(kClosed, std::memory_order_acq_rel); }
  void Retire() noexcept {
    void* retired = nullptr;
    {
      std::lock_guard lock(cleanup_mutex);
      if (released || gate.load(std::memory_order_acquire) != kClosed) return;
      released = true;
      retired = std::exchange(retained, nullptr);
    }
    if (retired != nullptr) release(retired);
    // Count the owned callback resource, not inert ticket metadata. Include
    // the release callback's entire tail in native image quiescence.
    State().connections.fetch_sub(1, std::memory_order_release);
  }
  static constexpr uint64_t kClosed = uint64_t{1} << 63;
  mutable std::atomic<uint64_t> gate{0};
  std::mutex cleanup_mutex;
  bool released = false;
  void* retained;
  const decltype(darwin_art_binder_authority_hooks_t::live) live;
  const decltype(darwin_art_binder_authority_hooks_t::release) release;
};

enum class Phase : uint8_t { kInert, kLive, kClosed };
struct NodeAuthority final {
  explicit NodeAuthority(std::shared_ptr<ConnectionAuthority> owner) noexcept
      : connection(std::move(owner)) {}
  bool Live() const noexcept {
    return phase.load(std::memory_order_acquire) == Phase::kLive && connection->Live();
  }
  void Seal() noexcept { phase.store(Phase::kClosed, std::memory_order_release); }
  const std::shared_ptr<ConnectionAuthority> connection;
  std::atomic<Phase> phase{Phase::kInert};
};

class NodeDeath final : public android::IBinder::DeathRecipient {
 public:
  explicit NodeDeath(const std::shared_ptr<NodeAuthority>& authority) noexcept
      : authority_(authority) {
    State().listeners.fetch_add(1, std::memory_order_relaxed);
  }
  ~NodeDeath() override {
    State().listeners.fetch_sub(1, std::memory_order_release);
  }
  void binderDied(const android::wp<android::IBinder>&) override {
    // No JNI, root policy, transport calls or resource cleanup in an obituary.
    // A strong obituary pin keeps the listener ledger nonzero through return.
    if (const auto owner = authority_.lock()) owner->Seal();
  }
 private:
  const std::weak_ptr<NodeAuthority> authority_;
};

uint64_t ReserveGeneration() noexcept {
  auto& next = State().next_generation;
  auto value = next.load(std::memory_order_relaxed);
  while (value != std::numeric_limits<uint64_t>::max()) {
    if (next.compare_exchange_weak(value, value + 1, std::memory_order_relaxed)) return value;
  }
  return 0;
}

class NativeEndpoint final : public EndpointLifetime {
 public:
  NativeEndpoint(android::sp<android::IBinder> binder,
                 std::shared_ptr<ConnectionAuthority> connection, uint64_t generation)
      : binder_(std::move(binder)), authority_(std::make_shared<NodeAuthority>(std::move(connection))),
        generation_(generation), listener_(android::sp<NodeDeath>::make(authority_)) {}
  ~NativeEndpoint() override { Close(); }
  uint64_t Generation() const noexcept override { return generation_; }
  bool Live() const noexcept override { return authority_->Live(); }
  bool Matches(uint64_t generation) const noexcept override {
    return generation == generation_ && Live();
  }
  bool Start() noexcept {
    if (binder_->linkToDeath(listener_) != android::OK) return false;
    linked_ = true;
    auto expected = Phase::kInert;
    if (!authority_->connection->Live() || !binder_->isBinderAlive() ||
        !authority_->phase.compare_exchange_strong(expected, Phase::kLive,
                                                    std::memory_order_acq_rel)) return false;
    // A node obituary or connection terminal racing this publication wins.
    return Live();
  }
  void Close() noexcept {
    authority_->Seal();
    android::sp<NodeDeath> listener;
    android::sp<android::IBinder> binder;
    bool linked = false;
    {
      std::lock_guard lock(cleanup_mutex_);
      listener = std::move(listener_);
      binder = std::move(binder_);
      linked = std::exchange(linked_, false);
    }
    if (linked && binder != nullptr && listener != nullptr) binder->unlinkToDeath(listener);
    // Binder sp and obituary listener release outside every provider/input lock.
  }
 private:
  std::mutex cleanup_mutex_;
  android::sp<android::IBinder> binder_;
  const std::shared_ptr<NodeAuthority> authority_;
  const uint64_t generation_;
  android::sp<NodeDeath> listener_;
  bool linked_ = false;
};
}  // namespace

bool InstallNativeBinderAuthority(const darwin_art_binder_authority_hooks_t* hooks) noexcept {
  FactoryAdmission admission(true);
  if (!admission) return false;
  // A legacy prefix can omit the capability; actual kernel-node capture then
  // rejects missing authority rather than manufacturing an owner.
  if (hooks == nullptr) return true;
  if (hooks->struct_size < sizeof(*hooks) || hooks->abi_version != 1 ||
      hooks->context == nullptr || hooks->retain == nullptr || hooks->live == nullptr ||
      hooks->release == nullptr) return false;
  try {
    auto connection = std::make_shared<ConnectionAuthority>(*hooks);
    if (!connection->Live()) return false;
    auto& state = State();
    std::lock_guard lock(state.mutex);
    if (state.closed) return false;
    state.connection = connection;
    return true;
  } catch (const std::bad_alloc&) { return false; }
}

std::shared_ptr<EndpointLifetime> CaptureNativeBinderEndpoint(JNIEnv* env,
                                                           jobject capability) noexcept {
  FactoryAdmission admission;
  if (!admission || env == nullptr || capability == nullptr || env->ExceptionCheck()) return {};
  // AOSP performs trusted BinderProxy/native-object decoding, not Java field shape.
  auto binder = android::ibinderForJavaObject(env, capability);
  if (env->ExceptionCheck() || binder == nullptr || binder->remoteBinder() == nullptr ||
      binder->remoteBinder()->isRpcBinder()) return {};
  std::shared_ptr<ConnectionAuthority> connection;
  {
    auto& state = State();
    std::lock_guard lock(state.mutex);
    if (state.closed || state.connection == nullptr) return {};
    const auto found = state.nodes.find(binder.get());
    if (found != state.nodes.end()) return found->second->Live() ? found->second : nullptr;
    connection = state.connection;
  }
  if (!connection->Live()) return {};
  const auto generation = ReserveGeneration();
  if (generation == 0) return {};
  try {
    auto candidate = std::make_shared<NativeEndpoint>(binder, connection, generation);
    if (!candidate->Start()) return {};
    std::shared_ptr<NativeEndpoint> selected;
    {
      auto& state = State();
      std::lock_guard lock(state.mutex);
      if (!state.closed && candidate->Live()) {
        const auto [position, inserted] = state.nodes.emplace(binder.get(), candidate);
        (void)inserted;
        selected = position->second;
      }
    }
    // A concurrent capture uses the existing canonical generation; losing
    // candidates unlink/retire outside catalog locks before factory admission ends.
    return selected != nullptr && selected->Live() ? selected : nullptr;
  } catch (const std::bad_alloc&) { return {}; }
}

bool CloseNativeBinderEndpointAdmission() noexcept {
  struct RetirementTail final {
    RetirementTail() { State().retirements.fetch_add(1, std::memory_order_acq_rel); }
    ~RetirementTail() { State().retirements.fetch_sub(1, std::memory_order_release); }
  } retirement_tail;
  std::map<android::IBinder*, std::shared_ptr<NativeEndpoint>> retired;
  std::shared_ptr<ConnectionAuthority> connection;
  {
    auto& state = State();
    std::lock_guard lock(state.mutex);
    state.closed = true;
    retired.swap(state.nodes);
    connection = state.connection;
  }
  if (connection != nullptr) connection->Close();
  for (const auto& [identity, endpoint] : retired) {
    (void)identity;
    endpoint->Close();
  }
  if (connection != nullptr) connection->Retire();
  return PollNativeBinderEndpointsQuiesced();
}

bool PollNativeBinderEndpointsQuiesced() noexcept {
  auto& state = State();
  std::shared_ptr<ConnectionAuthority> connection;
  {
    std::lock_guard lock(state.mutex);
    if (!state.closed) return false;
    connection = state.connection;
  }
  if (connection != nullptr) connection->Retire();
  std::lock_guard lock(state.mutex);
  return state.closed && state.factories.load(std::memory_order_acquire) == 0 &&
         state.retirements.load(std::memory_order_acquire) == 0 &&
         state.listeners.load(std::memory_order_acquire) == 0 &&
         state.connections.load(std::memory_order_acquire) == 0;
}

bool NativeBinderWorkersQuiesced() noexcept {
  const auto process = android::ProcessState::selfOrNull();
  return process == nullptr || !process->isThreadPoolStarted();
}
}  // namespace darwin_art::binder
