#pragma once

#include "finish_ledger.h"
#include <atomic>
#include <cstddef>

namespace darwin_art::input {

struct ReceiverFinishProgress final {
  size_t accepted = 0;
  size_t terminal = 0;
  bool backpressured = false;
  bool recovery_needed = false;
  bool retry_needed = false;
  bool coalesced = false;
  explicit operator bool() const { return accepted != 0; }
};

// One receiver epoch. Retains original publication/transport per reservation;
// no registry lookup, JNI globals, routing selection or descriptor ownership.
class ReceiverFinishOwner final {
 public:
  bool Reserve(uint32_t framework_sequence, InputEventOrigin,
               std::shared_ptr<const InputRoutingRecipient> original);
  bool ReserveNext(uint32_t* next_sequence, uint32_t* allocated,
                   InputEventOrigin,
                   std::shared_ptr<const InputRoutingRecipient> original);
  // Stops future imported-channel reservations for this receiver epoch and
  // returns one coherent sealed/outstanding snapshot. Idempotent.
  RemoteFinishState SealRemoteAdmission();
  RemoteFinishState RemoteState() const;
  bool HasOutstandingRemote() const;
  bool Cancel(uint32_t framework_sequence);
  bool CloseObservation(uint32_t framework_sequence, bool* handled);
  // Record Java finish once, then service retained submission obligations.
  // Progress is not proof that this particular packet reached its publisher.
  ReceiverFinishProgress Finish(uint32_t framework_sequence, bool handled);
  ReceiverFinishProgress RetryPending(size_t budget = 64);
  bool HasPendingAck() const;

 private:
  FinishLedger ledger_;
  std::atomic_flag retrying_ = ATOMIC_FLAG_INIT;
};

}  // namespace darwin_art::input
