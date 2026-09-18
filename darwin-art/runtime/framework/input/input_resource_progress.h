#pragma once

#include <cstdint>
#include <memory>
#include <utility>

namespace darwin_art::input {

enum class InputResourceProgressKind : std::uint8_t {
  kTxAccepted,
  kTxAdvanced,
  kTerminal,
  kTxTerminal,
  kRxTerminal,
  kLocalCapacity,
  // Hints only: subscribers must reacquire their reader authority before drain.
  kRxBuffered,
  kRxConsumed,
};

struct InputResourceProgress {
  InputResourceProgressKind kind;
  std::uint64_t revision;
};

// Allocation-free progress publication for one input resource. The source
// owns neither transport nor routing policy; subscribers receive stale or
// overlapping hints and must recheck their owner state before acting.
class InputResourceProgressSource final {
  struct Core;
  struct Subscription;

 public:
  class SubscriptionHandle final {
   public:
    SubscriptionHandle() = default;
    ~SubscriptionHandle();
    SubscriptionHandle(const SubscriptionHandle&) = delete;
    SubscriptionHandle& operator=(const SubscriptionHandle&) = delete;
    SubscriptionHandle(SubscriptionHandle&& other) noexcept;
    SubscriptionHandle& operator=(SubscriptionHandle&& other) noexcept;
    explicit operator bool() const { return subscription_ != nullptr; }

   private:
    friend class InputResourceProgressSource;
    SubscriptionHandle(std::shared_ptr<Core> core,
                       std::shared_ptr<Subscription> subscription)
        : core_(std::move(core)), subscription_(std::move(subscription)) {}
    void Drop() noexcept;
    std::shared_ptr<Core> core_;
    std::shared_ptr<Subscription> subscription_;
  };

  InputResourceProgressSource();
  InputResourceProgressSource(const InputResourceProgressSource&) = delete;
  InputResourceProgressSource& operator=(const InputResourceProgressSource&) = delete;

  SubscriptionHandle Subscribe(
      void (*notify)(void*, InputResourceProgress) noexcept,
      std::weak_ptr<void> context);
  std::uint64_t Revision() const;
  void Notify(InputResourceProgressKind kind) noexcept;

 private:
  std::shared_ptr<Core> core_;
};

}  // namespace darwin_art::input
