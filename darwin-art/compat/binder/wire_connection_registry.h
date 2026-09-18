#pragma once

#include "wire_channel_lifetime.h"

#include <array>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <map>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <utility>
#include <vector>

namespace darwin_art::binder {

// Process-local identity registry for native wire connections.  The registry
// deliberately exposes handles and transactions, rather than its map or its
// synchronization primitives, so a caller cannot manufacture an identity or
// replace an identity behind an existing handle.
template <typename Payload>
class WireConnectionRegistry final {
 private:
  class PinBatch;

 public:
  struct Connection final {
    const int fd;
    const std::shared_ptr<WireChannelLifetime> lifetime;
    const std::shared_ptr<Payload> payload;

    uint64_t Generation() const noexcept { return lifetime->Generation(); }

   private:
    Connection(int fd, std::shared_ptr<WireChannelLifetime> lifetime,
               std::shared_ptr<Payload> payload) noexcept
        : fd(fd),
          lifetime(std::move(lifetime)),
          payload(std::move(payload)) {}

    template <typename>
    friend class WireConnectionRegistry;
  };

  using Handle = std::shared_ptr<const Connection>;

  class Transaction final {
   public:
    Transaction(const Transaction&) = delete;
    Transaction& operator=(const Transaction&) = delete;
    Transaction(Transaction&&) = delete;
    Transaction& operator=(Transaction&&) = delete;

    ~Transaction() noexcept {
      // The recursive mutex is held at exactly this transaction's level.  A
      // nested transaction contributes its complete pin chain to its parent;
      // consequently no shared_ptr is released while an outer level remains
      // locked.  The outermost level moves the chain away before unlocking.
      if (parent_ != nullptr) {
        owner_->top_ = parent_;
        parent_->AppendPins(std::move(pins_), pins_tail_);
        lock_.unlock();
        return;
      }

      owner_->top_ = nullptr;
      auto pins = std::move(pins_);
      lock_.unlock();
      DestroyPins(std::move(pins));
    }

    // Returns the current identity for fd, if one exists.  A sealed identity
    // is intentionally still returned; cleanup callers decide whether Live()
    // is required for their operation.
    Handle FindEstablished(int fd) {
      const auto entry = owner_->connections_.find(fd);
      if (entry == owner_->connections_.end()) return {};
      Pin(entry->second);
      return entry->second;
    }

    // Returns only an existing entry whose immutable generation is exact.
    // This comparison does not require Live(), allowing a cleanup path to
    // inspect a sealed identity without confusing it with a successor.
    Handle FindExact(int fd, uint64_t generation) {
      const auto entry = owner_->connections_.find(fd);
      if (entry == owner_->connections_.end() || !entry->second ||
          entry->second->Generation() != generation) {
        return {};
      }
      Pin(entry->second);
      return entry->second;
    }

    // Publishes one new identity, but only into an absent, non-negative fd.
    // No existing entry is replaced and no alternate fd is selected.
    Handle CreateOwned(int fd, const std::shared_ptr<Payload>& payload) {
      if (fd < 0 || !payload) return {};

      auto entry = owner_->connections_.find(fd);
      if (entry != owner_->connections_.end()) return {};

      auto lifetime = WireChannelLifetime::Create();
      if (!lifetime) return {};

      // The map insertion is intentionally the only publication point.  If
      // construction fails, the caller's payload reference remains untouched.
      Handle connection(new Connection(fd, std::move(lifetime), payload));
      // Pin before publication so an insertion/allocation failure cannot drop
      // the newly constructed payload while this transaction owns the lock.
      Pin(connection);
      auto inserted = owner_->connections_.emplace(fd, connection);
      if (!inserted.second) return {};
      return inserted.first->second;
    }

    // Retires only the exact object currently stored at handle->fd.  Pinning
    // precedes Seal and erase so the payload remains alive through cleanup and
    // through any wait that still owns this transaction.
    bool RetireExact(const Handle& handle) {
      if (!handle) return false;
      auto entry = owner_->connections_.find(handle->fd);
      if (entry == owner_->connections_.end() ||
          entry->second.get() != handle.get()) {
        return false;
      }

      Pin(entry->second);
      entry->second->lifetime->Seal();
      owner_->connections_.erase(entry);
      return true;
    }

    void NotifyAll() noexcept { owner_->condition_.notify_all(); }

    template <typename Predicate>
    void Wait(Predicate&& predicate) {
      if (parent_ != nullptr) {
        throw std::logic_error(
            "WireConnectionRegistry::Wait requires the outermost transaction");
      }
      // condition_variable_any temporarily unlocks its BasicLockable.  The
      // transaction stack must follow that unlock: while this transaction is
      // asleep, another thread owns the registry as an outermost transaction,
      // not as a child of this (possibly foreign-thread) stack frame.
      WaitUnlocker wait_unlocker(this);
      std::unique_lock<WaitUnlocker> wait_lock(wait_unlocker,
                                                std::adopt_lock);
      try {
        owner_->condition_.wait(wait_lock,
                                std::forward<Predicate>(predicate));
      } catch (...) {
        wait_lock.release();
        throw;
      }
      wait_lock.release();
    }

   private:
    friend class WireConnectionRegistry;

    class WaitUnlocker final {
     public:
      explicit WaitUnlocker(Transaction* transaction)
          : transaction_(transaction) {}

      void lock() {
        transaction_->lock_.lock();
        transaction_->owner_->top_ = transaction_;
      }

      void unlock() noexcept {
        transaction_->owner_->top_ = nullptr;
        transaction_->lock_.unlock();
      }

     private:
      Transaction* transaction_;
    };

    explicit Transaction(WireConnectionRegistry& owner)
        // Allocate before acquiring this transaction's recursive level. A
        // nested caller can already own an outer level: this is not an
        // allocation-free API. The guarantee concerns deferred pin destruction.
        : owner_(&owner),
          pins_(new PinBatch()),
          pins_tail_(pins_.get()),
          lock_(owner.mutex_) {
      parent_ = owner.top_;
      owner.top_ = this;
    }

    void Pin(const Handle& handle) { pins_->Add(handle); }

    void AppendPins(std::unique_ptr<PinBatch> pins, PinBatch* tail) noexcept {
      if (!pins) return;
      pins_tail_->next = std::move(pins);
      pins_tail_ = tail;
    }

    static void DestroyPins(std::unique_ptr<PinBatch> pins) noexcept {
      // Destruction is iterative to avoid recursive destruction of a long
      // nested transaction chain.  This runs after every recursive lock level
      // has been released.
      while (pins) {
        auto next = std::move(pins->next);
        pins.reset();
        pins = std::move(next);
      }
    }

    WireConnectionRegistry* owner_;
    std::unique_ptr<PinBatch> pins_;
    PinBatch* pins_tail_;
    std::unique_lock<std::recursive_mutex> lock_;
    Transaction* parent_ = nullptr;
  };

  WireConnectionRegistry() = default;
  WireConnectionRegistry(const WireConnectionRegistry&) = delete;
  WireConnectionRegistry& operator=(const WireConnectionRegistry&) = delete;

  [[nodiscard]] Transaction Lock() { return Transaction(*this); }

 private:
  class PinBatch final {
   public:
    PinBatch() { promoted.reserve(kInlinePins); }

    void Add(const Handle& handle) {
      if (!handle || Contains(handle)) return;
      if (inline_count < kInlinePins) {
        // shared_ptr copying is noexcept and this slot is known to be empty.
        inline_pins[inline_count++] = handle;
        return;
      }

      // A vector reallocation must never destroy shared_ptrs while the
      // registry is locked.  It owns heap cells instead, so reallocation only
      // destroys unique_ptrs; each actual Handle is destroyed with the batch
      // after the lock is released.  Reserve precedes the copy/promote step.
      if (promoted.size() == promoted.capacity()) {
        const std::size_t next_capacity =
            promoted.capacity() == 0 ? kInlinePins : promoted.capacity() * 2;
        promoted.reserve(next_capacity);
      }
      promoted.push_back(std::make_unique<Handle>(handle));
    }

    std::unique_ptr<PinBatch> next;

   private:
    static constexpr std::size_t kInlinePins = 8;

    bool Contains(const Handle& handle) const noexcept {
      for (std::size_t i = 0; i < inline_count; ++i) {
        if (inline_pins[i].get() == handle.get()) return true;
      }
      for (const auto& promoted_pin : promoted) {
        if (promoted_pin != nullptr && promoted_pin->get() == handle.get()) {
          return true;
        }
      }
      return false;
    }

    std::array<Handle, kInlinePins> inline_pins{};
    std::size_t inline_count = 0;
    std::vector<std::unique_ptr<Handle>> promoted;
  };

  std::recursive_mutex mutex_;
  std::condition_variable_any condition_;
  std::map<int, Handle> connections_;
  Transaction* top_ = nullptr;
};

}  // namespace darwin_art::binder
