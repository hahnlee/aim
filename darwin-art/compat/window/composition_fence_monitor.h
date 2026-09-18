#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>

struct DarwinArtBionicPollFd;

namespace darwin_art::window {

// The monitor owns every descriptor admitted through Track().  The broker is
// injected so this owner can be tested with real pipes without linking the
// runtime's broker implementation into the test executable.
struct CompositionFenceBroker {
  // Syscall adapters report failure through their integer result/errno;
  // unwinding across monitor descriptor settlement is not supported.
  using Poll = int (*)(DarwinArtBionicPollFd*, size_t, int) noexcept;
  using Close = int (*)(int) noexcept;

  Poll poll = nullptr;
  Close close = nullptr;
};

using CompositionFenceReadyCallback = void (*)(void* context,
                                                uint64_t generation) noexcept;

class CompositionFenceMonitor final {
 public:
  static CompositionFenceBroker DefaultBroker() noexcept;

  explicit CompositionFenceMonitor(
      CompositionFenceReadyCallback callback = nullptr,
      void* callback_context = nullptr,
      CompositionFenceBroker broker = DefaultBroker());
  ~CompositionFenceMonitor();

  CompositionFenceMonitor(const CompositionFenceMonitor&) = delete;
  CompositionFenceMonitor& operator=(const CompositionFenceMonitor&) = delete;

  // Consumes fence_fd on both success and failure.  A successful admission
  // assigns the next FIFO generation and publishes it for ScanoutReady().
  bool Track(int fence_fd) noexcept;

  bool ScanoutReady() const noexcept;
  uint64_t SubmittedGeneration() const noexcept;
  uint64_t ReadyGeneration() const noexcept;

  // Stop is idempotent. It first prevents admission and lets the polling
  // worker and any ready callback finish, then closes all still-pending
  // descriptors. The owner must call this before its callback context dies.
  // Calling Stop (or destroying the monitor) from its callback is a contract
  // violation and fails fast because a worker cannot join itself.
  void Stop() noexcept;

  // Used only when the enclosing surface cannot provide an owner (for
  // example, a failed construction path); it preserves the consumed-FD
  // contract without exposing broker details to the surface bridge.
  static void CloseRejected(int fence_fd) noexcept;

 private:
  struct State;
  static void Run(State* state) noexcept;
  std::unique_ptr<State> state_;
};

}  // namespace darwin_art::window
