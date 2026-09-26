#pragma once

#include "../input/channel_resources.h"
#include "../input/input_transport.h"

#include <cstddef>
#include <cstdint>
#include <memory>

namespace darwin_art::framework::wm {

// WMS-owned opaque identity for the exact server InputChannel resource that
// was acquired by the caller. Releasing this identity is not TX settlement.
using WindowInputEndpointLeaseToken = std::uint64_t;

enum class WindowInputEndpointLeaseAcquireStatus : std::uint8_t {
  kAcquired,
  kInvalidArgument,
  kClosed,
  kOutOfMemory,
  kExhausted,
};

struct WindowInputEndpointLeaseAcquireResult final {
  WindowInputEndpointLeaseAcquireStatus status =
      WindowInputEndpointLeaseAcquireStatus::kInvalidArgument;
  WindowInputEndpointLeaseToken token = 0;
};

// Retains the original JNI-acquired InputChannelResources behind an opaque
// token. It owns no jobject, descriptor, window lookup, or publication policy.
class WindowInputEndpointLease final {
 public:
  static std::shared_ptr<WindowInputEndpointLease> Create() noexcept;
  ~WindowInputEndpointLease();

  WindowInputEndpointLease(const WindowInputEndpointLease&) = delete;
  WindowInputEndpointLease& operator=(const WindowInputEndpointLease&) = delete;

  WindowInputEndpointLeaseAcquireResult Acquire(
      const std::shared_ptr<darwin_art::input::InputChannelResources>& resources);

  // Invalid/stale tokens fail closed as kTerminal. A resource's status is
  // returned unchanged; no terminal result implies that buffered TX settled.
  darwin_art::input::InputTransportStatus PublishWindow(
      WindowInputEndpointLeaseToken token, std::int32_t left, std::int32_t top,
      std::int32_t right, std::int32_t bottom, bool visible,
      std::uint32_t input_flags = 0);
  darwin_art::input::InputTransportStatus PublishFocus(
      WindowInputEndpointLeaseToken token, std::uint64_t epoch, bool focused);

  // Flushes retained bytes on the exact original endpoint. The resource is
  // pinned before leaving the lease-map lock so provider IO cannot race
  // release or resolve a successor channel.
  darwin_art::input::InputTransportStatus Flush(
      WindowInputEndpointLeaseToken token);

  // Captures the current accepted prefix on the exact original transport and
  // queries that same pinned transport. This does not infer an empty queue or
  // inspect a successor endpoint.
  darwin_art::input::InputTransportTxFenceStatus QueryAcceptedTx(
      WindowInputEndpointLeaseToken token);

  // Retains only the original endpoint. False for stale tokens or in-flight TX;
  // a live false result has already denied future TX admission. Retry later.
  bool TerminateAndQuiesce(WindowInputEndpointLeaseToken token);

  // Release detaches the exact token. Any in-flight publication retains the
  // original shared resource until its provider call returns.
  bool Release(WindowInputEndpointLeaseToken token) noexcept;
  bool Close() noexcept;
  bool closed() const noexcept;
  std::size_t size() const noexcept;

 private:
  WindowInputEndpointLease();
  struct State;
  std::shared_ptr<State> state_;
};

}  // namespace darwin_art::framework::wm
