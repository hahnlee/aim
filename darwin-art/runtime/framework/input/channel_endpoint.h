#pragma once

#include "finish_ledger.h"
#include "input_transport.h"

#include <cstdint>
#include <memory>

namespace darwin_art::input {

enum class ChannelEndpointCreationError {
  kNone,
  kInvalidDescriptor,
  kTransportUnavailable,
  kOutOfMemory,
};

// Non-JNI InputChannel resource and finish-sequence owner. Routing chooses
// recipients; this owner only retains a transport and records completions.
class ChannelEndpoint final {
 public:
  static std::shared_ptr<ChannelEndpoint> CreateLocal(
      ChannelEndpointCreationError* error = nullptr);
  // Always consumes the descriptor, including allocation/open failure.
  static std::shared_ptr<ChannelEndpoint> AdoptRemote(
      int owned_fd, ChannelEndpointCreationError* error = nullptr);
  explicit ChannelEndpoint(std::shared_ptr<InputTransport> transport);
  ~ChannelEndpoint();
  ChannelEndpoint(const ChannelEndpoint&) = delete;
  ChannelEndpoint& operator=(const ChannelEndpoint&) = delete;

  std::shared_ptr<InputTransport> Transport() const;
  bool TryRegisterFinish(uint32_t sequence);
  bool CancelFinish(uint32_t sequence);
  bool CloseFinishObservation(uint32_t sequence, bool* handled);
  bool MarkFinishAckAccepted(uint32_t sequence);
  // Legacy callers retain a void registration surface; failed admission is
  // intentionally not converted into eviction or success.
  void RegisterFinish(uint32_t sequence);
  bool RecordFinish(uint32_t sequence, bool handled);
  bool TakeFinish(uint32_t sequence, bool* handled);
  uint64_t OverflowCount() const;
  InputTransportStatus SendAck(uint32_t sequence, bool handled);
  bool WakeLocal();
  void DrainLocalWake();
  int BorrowParcelFd(bool server_side) const;
  // input_flags: InputWindowFlags of the WMS window (input_window_state.h).
  InputTransportStatus PublishWindow(int32_t left, int32_t top, int32_t right,
                                     int32_t bottom, bool visible,
                                     uint32_t input_flags = 0);
  InputTransportStatus PublishFocus(uint64_t epoch, bool focused);

 private:
  struct State;
  std::shared_ptr<State> state_;
};
}  // namespace darwin_art::input
