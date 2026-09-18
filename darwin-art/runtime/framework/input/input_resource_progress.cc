#include "input_resource_progress.h"

#include <atomic>
#include <mutex>
#include <new>
#include <utility>
#include <vector>

namespace darwin_art::input {

struct InputResourceProgressSource::Subscription {
  Subscription(void (*callback)(void*, InputResourceProgress) noexcept,
               std::weak_ptr<void> value)
      : notify(callback), context(std::move(value)) {}

  void (*const notify)(void*, InputResourceProgress) noexcept;
  const std::weak_ptr<void> context;
  std::atomic<bool> active{true};
};

struct InputResourceProgressSource::Core {
  struct Link {
    Link(std::weak_ptr<Subscription> value,
         std::shared_ptr<const Link> next_value)
        : subscription(std::move(value)), next(std::move(next_value)) {}

    const std::weak_ptr<Subscription> subscription;
    const std::shared_ptr<const Link> next;
  };

  mutable std::mutex mutex;
  std::shared_ptr<const Link> head;
  std::uint64_t revision = 0;
};

InputResourceProgressSource::InputResourceProgressSource()
    : core_(std::make_shared<Core>()) {}

InputResourceProgressSource::SubscriptionHandle
InputResourceProgressSource::Subscribe(
    void (*notify)(void*, InputResourceProgress) noexcept,
    std::weak_ptr<void> context) {
  if (notify == nullptr || context.expired()) return {};

  std::shared_ptr<Subscription> subscription;
  try {
    subscription = std::make_shared<Subscription>(notify, std::move(context));
  } catch (const std::bad_alloc&) {
    return {};
  }

  const auto core = core_;
  for (;;) {
    std::shared_ptr<const Core::Link> old_head;
    {
      std::lock_guard lock(core->mutex);
      old_head = core->head;
    }

    std::shared_ptr<const Core::Link> candidate;
    try {
      // Build a fresh immutable chain for Subscribe. Only live subscriptions
      // with live weak contexts are copied, so expired leases are pruned
      // transactionally. The old chain is unchanged if allocation throws.
      std::vector<std::shared_ptr<Subscription>> live;
      for (auto link = old_head; link != nullptr; link = link->next) {
        auto existing = link->subscription.lock();
        if (existing == nullptr ||
            !existing->active.load(std::memory_order_acquire) ||
            existing->context.expired())
          continue;
        live.push_back(std::move(existing));
      }
      live.push_back(subscription);
      for (auto it = live.rbegin(); it != live.rend(); ++it)
        candidate = std::make_shared<Core::Link>(*it, std::move(candidate));
    } catch (const std::bad_alloc&) {
      return {};
    }

    {
      std::lock_guard lock(core->mutex);
      if (core->head != old_head) continue;
      core->head = std::move(candidate);
      return SubscriptionHandle(core, std::move(subscription));
    }
  }
}

std::uint64_t InputResourceProgressSource::Revision() const {
  const auto core = core_;
  std::lock_guard lock(core->mutex);
  return core->revision;
}

void InputResourceProgressSource::Notify(InputResourceProgressKind kind) noexcept {
  const auto core = core_;
  std::shared_ptr<const Core::Link> head;
  std::uint64_t revision = 0;
  {
    std::lock_guard lock(core->mutex);
    revision = ++core->revision;
    head = core->head;
  }

  const InputResourceProgress event{kind, revision};
  // Link and subscription/context weak locks do not allocate. Callback
  // reentry is safe because no source lock is held across user code.
  for (auto link = head; link != nullptr; link = link->next) {
    auto subscription = link->subscription.lock();
    if (subscription == nullptr ||
        !subscription->active.load(std::memory_order_acquire))
      continue;
    auto context = subscription->context.lock();
    if (context == nullptr ||
        !subscription->active.load(std::memory_order_acquire))
      continue;
    subscription->notify(context.get(), event);
  }
}

InputResourceProgressSource::SubscriptionHandle::~SubscriptionHandle() {
  Drop();
}

InputResourceProgressSource::SubscriptionHandle::SubscriptionHandle(
    SubscriptionHandle&& other) noexcept
    : core_(std::move(other.core_)),
      subscription_(std::move(other.subscription_)) {}

InputResourceProgressSource::SubscriptionHandle&
InputResourceProgressSource::SubscriptionHandle::operator=(
    SubscriptionHandle&& other) noexcept {
  if (this != &other) {
    Drop();
    core_ = std::move(other.core_);
    subscription_ = std::move(other.subscription_);
  }
  return *this;
}

void InputResourceProgressSource::SubscriptionHandle::Drop() noexcept {
  if (subscription_ != nullptr)
    subscription_->active.store(false, std::memory_order_release);
  subscription_.reset();
  core_.reset();
}

}  // namespace darwin_art::input
