#pragma once

#include <atomic>
#include <cstdint>
#include <memory>
#include <utility>

namespace darwin_art::window {

// A nonblocking admission boundary for callbacks which may outlive the
// observer's owner. Close prevents new admissions but deliberately does not
// wait for callbacks already in flight. The admitted Lease keeps the small
// control block alive, so it remains safe when the containing State is
// released before callback cleanup has completed.
class BlastCallbackGate final {
 private:
  struct Control {
    // The high bit is closed; the remaining bits are the admitted lease count.
    // Keeping both values in one word makes Close and TryEnter a single atomic
    // state transition, so Close cannot return while a racing TryEnter can
    // still claim a lease.
    static constexpr uint64_t kClosed = UINT64_C(1) << 63;
    static constexpr uint64_t kCountMask = ~kClosed;

    std::atomic<uint64_t> state{0};
  };

 public:
  class Lease final {
   public:
    Lease() noexcept = default;
    ~Lease() { Release(); }

    Lease(const Lease&) = delete;
    Lease& operator=(const Lease&) = delete;

    Lease(Lease&& other) noexcept : control_(std::move(other.control_)) {}

    Lease& operator=(Lease&& other) noexcept {
      if (this != &other) {
        Release();
        control_ = std::move(other.control_);
      }
      return *this;
    }

    // An explicit conversion prevents accidentally treating a lease as an
    // integer while retaining the natural `if (gate.TryEnter())` spelling.
    explicit operator bool() const noexcept { return control_ != nullptr; }

   private:
    explicit Lease(std::shared_ptr<Control>&& control) noexcept
        : control_(std::move(control)) {}

    void Release() noexcept {
      if (control_ == nullptr) return;
      control_->state.fetch_sub(1, std::memory_order_acq_rel);
      control_.reset();
    }

    std::shared_ptr<Control> control_;
    friend class BlastCallbackGate;
  };

  BlastCallbackGate() : control_(std::make_shared<Control>()) {}

  BlastCallbackGate(const BlastCallbackGate&) = delete;
  BlastCallbackGate& operator=(const BlastCallbackGate&) = delete;
  BlastCallbackGate(BlastCallbackGate&&) noexcept = default;
  BlastCallbackGate& operator=(BlastCallbackGate&&) noexcept = default;
  ~BlastCallbackGate() = default;

  // Attempts one callback admission without waiting. A false Lease means the
  // gate was closed (or its count reached the representable limit).
  [[nodiscard]] Lease TryEnter() const noexcept {
    if (control_ == nullptr) return Lease{};

    // Copying an existing shared_ptr only increments its control-block
    // reference count; it does not allocate. Keeping this owner in the Lease
    // also closes the State/gate destruction race for an already-admitted
    // callback.
    std::shared_ptr<Control> owner = control_;
    uint64_t state = owner->state.load(std::memory_order_acquire);
    for (;;) {
      if ((state & Control::kClosed) != 0 ||
          (state & Control::kCountMask) == Control::kCountMask) {
        return Lease{};
      }
      if (owner->state.compare_exchange_weak(
              state, state + 1, std::memory_order_acquire,
              std::memory_order_relaxed)) {
        return Lease(std::move(owner));
      }
      // A failed CAS updates state. Recheck the closed bit and count before
      // making another lock-free, nonblocking retry.
    }
  }

  // Idempotently closes admission. Existing leases are not revoked and are
  // never waited on here; their destruction makes Drained() become true.
  void Close() const noexcept {
    if (control_ != nullptr) {
      control_->state.fetch_or(Control::kClosed, std::memory_order_acq_rel);
    }
  }

  // Reports closure plus the absence of all admitted callbacks. This is a
  // snapshot and may become false only if called on a moved-from gate (which
  // has no control block); it never waits for cleanup.
  bool Drained() const noexcept {
    if (control_ == nullptr) return false;
    const uint64_t state = control_->state.load(std::memory_order_acquire);
    return (state & Control::kClosed) != 0 &&
           (state & Control::kCountMask) == 0;
  }

 private:
  std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::window
