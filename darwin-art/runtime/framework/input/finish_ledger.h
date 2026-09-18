#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <mutex>
#include <memory>
#include "input_event_origin.h"

namespace darwin_art::input {
struct InputRoutingRecipient;
struct FinishAck final {
  InputEventOrigin origin;
  std::shared_ptr<const InputRoutingRecipient> recipient;
  bool handled = false;
};

struct RemoteFinishState final {
  bool sealed = false;
  size_t outstanding_remote = 0;
};

// Android finishInputEvent bookkeeping, independent of endpoint descriptor
// ownership. Reservations are bounded and lossless: a full ledger rejects a
// new sequence instead of evicting an outstanding obligation.
class FinishLedger final {
 public:
  static constexpr size_t kCapacity = 256;

  FinishLedger() = default;
  FinishLedger(const FinishLedger&) = delete;
  FinishLedger& operator=(const FinishLedger&) = delete;

  // Permanently stop admitting imported-channel reservations and return a
  // coherent sealed/outstanding snapshot. Idempotent; local reservations
  // remain admissible.
  RemoteFinishState SealRemoteAdmission();
  RemoteFinishState RemoteState() const;
  // Reports imported reservations still requiring remote ACK submission,
  // including unrecorded and currently claimed entries.
  bool HasOutstandingRemote() const;

  // Admit one sequence. Duplicate registration and full-ledger admission are
  // rejected; only the latter increments OverflowCount(). Imported-channel
  // admission is rejected after SealRemoteAdmission() without overflow.
  bool TryRegister(uint32_t sequence, InputEventOrigin origin = {},
                   std::shared_ptr<const InputRoutingRecipient> recipient = {});
  // Atomically allocate the first free sequence at or after *next_sequence,
  // register it, and advance the caller's cursor. Zero is a valid sequence;
  // failure leaves both output values unchanged.
  bool TryRegisterNext(
      uint32_t* next_sequence, uint32_t* allocated,
      InputEventOrigin origin = {},
      std::shared_ptr<const InputRoutingRecipient> recipient = {});
  // Remove an unacknowledged reservation, for pre-invocation rollback.
  bool Cancel(uint32_t sequence);
  // Record exactly one ACK. Duplicate ACKs are rejected and do not overwrite
  // the first handled value.
  bool Record(uint32_t sequence, bool handled, FinishAck* ack = nullptr);
  // Reserve the earliest recorded ACK for one outbound submission. The
  // reservation is identified by a unique claim ticket rather than framework
  // sequence, which may be reused after a deferred attempt.
  bool ClaimPendingAck(FinishAck* snapshot, uint64_t* ticket);
  // Complete one exact submission claim. A deferred submission releases the
  // claim but retains the recorded obligation for a later retry.
  bool CompleteAckSubmission(uint64_t ticket, bool accepted);
  // Complete one exact claim when its destination is terminal. Terminal ACKs
  // are not accepted, but are no longer retryable; observation closure still
  // owns their final removal if it has not happened yet.
  bool CompleteAckTerminal(uint64_t ticket);
  // True while any recorded ACK, including a claimed submission, remains
  // neither accepted nor terminal.
  bool HasPendingAck() const;
  // Close the Java/event observation scope. Returns whether an ACK was
  // already observed and copies its first handled value. Closing consumes
  // only after the outbound ACK is accepted or terminal.
  bool CloseObservation(uint32_t sequence, bool* handled);
  // Mark the outbound ACK accepted. If observation is already closed this
  // consumes the reservation; otherwise it remains for CloseObservation.
  bool MarkAckAccepted(uint32_t sequence);
  // Consume an acknowledged reservation.
  bool Take(uint32_t sequence, bool* handled);
  uint64_t OverflowCount() const;

 private:
  struct Entry {
    uint32_t sequence = 0;
    uint64_t claim_ticket = 0;
    bool observer_open = true;
    bool ack_recorded = false;
    bool ack_accepted = false;
    bool ack_terminal = false;
    bool claimed = false;
    bool handled = false;
    InputEventOrigin origin{};
    std::shared_ptr<const InputRoutingRecipient> recipient;
  };

  mutable std::mutex mutex_;
  std::array<Entry, kCapacity> entries_{};
  size_t count_ = 0;
  uint64_t overflow_count_ = 0;
  uint64_t next_ticket_ = 1;
  bool remote_admission_sealed_ = false;

  RemoteFinishState RemoteStateLocked() const;
  uint64_t AllocateTicketLocked();
};

}  // namespace darwin_art::input
