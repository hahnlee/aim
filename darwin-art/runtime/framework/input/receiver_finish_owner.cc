#include "receiver_finish_owner.h"

#include "input_routing.h"
#include "input_transport.h"
#include <stdexcept>

namespace darwin_art::input {
namespace {
bool CanReserve(InputEventOrigin origin,
                const std::shared_ptr<const InputRoutingRecipient>& original) {
  if (original == nullptr) return false;
  switch (origin.kind) {
    case ReceiverPacketOrigin::kLocalQueue:
      return true;
    case ReceiverPacketOrigin::kImportedChannel:
      return original->originalendpoint != nullptr &&
             original->originalendpoint->transport != nullptr &&
             original->originalendpoint->transport->RemoteEndpointFd() >= 0;
  }
  return false;
}
}  // namespace

bool ReceiverFinishOwner::Reserve(
    uint32_t sequence, InputEventOrigin origin,
    std::shared_ptr<const InputRoutingRecipient> original) {
  if (!CanReserve(origin, original)) return false;
  return ledger_.TryRegister(sequence, origin, std::move(original));
}

bool ReceiverFinishOwner::ReserveNext(
    uint32_t* next_sequence, uint32_t* allocated, InputEventOrigin origin,
    std::shared_ptr<const InputRoutingRecipient> original) {
  if (!CanReserve(origin, original)) return false;
  return ledger_.TryRegisterNext(next_sequence, allocated, origin,
                                 std::move(original));
}

RemoteFinishState ReceiverFinishOwner::SealRemoteAdmission() {
  return ledger_.SealRemoteAdmission();
}

RemoteFinishState ReceiverFinishOwner::RemoteState() const {
  return ledger_.RemoteState();
}

bool ReceiverFinishOwner::HasOutstandingRemote() const {
  return ledger_.HasOutstandingRemote();
}

bool ReceiverFinishOwner::Cancel(uint32_t sequence) {
  return ledger_.Cancel(sequence);
}

bool ReceiverFinishOwner::CloseObservation(uint32_t sequence, bool* handled) {
  return ledger_.CloseObservation(sequence, handled);
}

ReceiverFinishProgress ReceiverFinishOwner::Finish(uint32_t sequence, bool handled) {
  if (!ledger_.Record(sequence, handled)) return {};
  return RetryPending();
}

bool ReceiverFinishOwner::HasPendingAck() const {
  return ledger_.HasPendingAck();
}

ReceiverFinishProgress ReceiverFinishOwner::RetryPending(size_t budget) {
  ReceiverFinishProgress progress;
  if (retrying_.test_and_set(std::memory_order_acquire)) {
    progress.coalesced = progress.retry_needed = true;
    return progress;
  }
  struct Admission {
    std::atomic_flag& flag;
    bool active = true;
    void Release() { flag.clear(std::memory_order_release); active = false; }
    ~Admission() { if (active) Release(); }
  } admission{retrying_};
  for (size_t count = 0; count < budget; ++count) {
    FinishAck ack;
    uint64_t ticket = 0;
    if (!ledger_.ClaimPendingAck(&ack, &ticket)) break;
    struct Claim {
      FinishLedger& ledger;
      uint64_t ticket;
      bool settled = false;
      ~Claim() { if (!settled) (void)ledger.CompleteAckSubmission(ticket, false); }
    } claim{ledger_, ticket};
    InputTransportStatus status = InputTransportStatus::kAccepted;
    std::shared_ptr<InputTransport> transport;
    if (ack.origin.kind == ReceiverPacketOrigin::kImportedChannel) {
      transport = ack.recipient->originalendpoint->transport;
      status = transport->IsTxTerminal() ? InputTransportStatus::kTerminal
          : SendInputTransportAck64(transport.get(), ack.origin.sequence, ack.handled);
    }
    if (status == InputTransportStatus::kBackpressured) {
      progress.backpressured = true;
      // Empty-TX allocation/counter rejection has no writable wake source.
      // Retain it for explicit owner recovery, never spin an empty FD.
      progress.recovery_needed = !transport->HasPendingTx();
      break;
    }
    const bool terminal = status == InputTransportStatus::kTerminal;
    claim.settled = terminal ? ledger_.CompleteAckTerminal(ticket)
                            : ledger_.CompleteAckSubmission(ticket, true);
    if (!claim.settled) throw std::logic_error("Lost exact receiver ACK claim");
    if (terminal) ++progress.terminal;
    else ++progress.accepted;
  }
  // Release admission BEFORE the final pending scan. A concurrent/reentrant
  // finish either appears in this scan, or acquires admission itself; there
  // is no final-empty-scan/busy-clear lost-wakeup interval.
  admission.Release();
  progress.retry_needed = ledger_.HasPendingAck();
  return progress;
}

}  // namespace darwin_art::input
