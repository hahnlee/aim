#include "runtime/framework/input/finish_ledger.h"

#include <cassert>
#include <cstdio>
#include <limits>
#include <atomic>
#include <thread>

using darwin_art::input::FinishLedger;
using darwin_art::input::FinishAck;
using darwin_art::input::InputEventOrigin;
using darwin_art::input::ReceiverPacketOrigin;

int main() {
  // EOF admission is independent of Java finish. Future Java completions and
  // claimed/deferred ACKs remain remote obligations, not just recorded ACKs.
  FinishLedger eof;
  const InputEventOrigin imported{ReceiverPacketOrigin::kImportedChannel, 99};
  assert(eof.TryRegister(10, imported));
  assert(eof.TryRegister(11, imported));
  assert(eof.TryRegister(12));
  assert(eof.HasOutstandingRemote() && !eof.HasPendingAck());
  eof.SealRemoteAdmission();
  eof.SealRemoteAdmission();
  uint32_t eof_cursor = 20, eof_output = 77;
  assert(!eof.TryRegister(20, imported));
  assert(!eof.TryRegisterNext(&eof_cursor, &eof_output, imported));
  assert(eof_cursor == 20 && eof_output == 77 && eof.OverflowCount() == 0);
  assert(eof.TryRegisterNext(&eof_cursor, &eof_output));
  assert(eof_output == 20 && eof_cursor == 21);
  assert(eof.Cancel(11) && eof.HasOutstandingRemote());
  assert(!eof.CloseObservation(10, nullptr));
  assert(eof.HasOutstandingRemote() && !eof.HasPendingAck());
  assert(eof.Record(10, true));
  FinishAck eof_snapshot;
  uint64_t eof_ticket = 0;
  assert(eof.ClaimPendingAck(&eof_snapshot, &eof_ticket));
  assert(eof.HasOutstandingRemote());
  assert(eof.CompleteAckSubmission(eof_ticket, false));
  assert(eof.HasOutstandingRemote());
  assert(eof.ClaimPendingAck(&eof_snapshot, &eof_ticket));
  assert(eof.CompleteAckSubmission(eof_ticket, true));
  assert(!eof.HasOutstandingRemote());
  // Acceptance/terminal settle remote transport obligation even while the
  // observation still owns a live Java sequence. Local work does not count.
  FinishLedger observed;
  assert(observed.TryRegister(1, imported));
  assert(observed.Record(1, false));
  assert(observed.MarkAckAccepted(1));
  assert(!observed.HasOutstandingRemote());
  assert(observed.TryRegister(2, imported));
  assert(observed.Record(2, true));
  assert(observed.ClaimPendingAck(&eof_snapshot, &eof_ticket));
  assert(observed.CompleteAckTerminal(eof_ticket));
  assert(!observed.HasOutstandingRemote());
  assert(observed.CloseObservation(1, nullptr));
  assert(observed.CloseObservation(2, nullptr));
  assert(!observed.RemoteState().sealed);  // Empty is not EOF settlement.
  const auto observed_eof = observed.SealRemoteAdmission();
  assert(observed_eof.sealed && observed_eof.outstanding_remote == 0);
  // Admission racing EOF is linearized by the same ledger mutex. Whatever
  // wins, the seal snapshot must include every successful prior admission.
  for (unsigned iteration = 0; iteration < 100; ++iteration) {
    FinishLedger racing;
    std::atomic<bool> start{false};
    bool admitted = false;
    std::thread registration([&] {
      while (!start.load(std::memory_order_acquire)) {}
      admitted = racing.TryRegister(1, imported);
    });
    start.store(true, std::memory_order_release);
    const auto snapshot = racing.SealRemoteAdmission();
    registration.join();
    assert(snapshot.sealed && snapshot.outstanding_remote == (admitted ? 1u : 0u));
    assert(!racing.TryRegister(2, imported));
  }

  // Sequence allocation is one ledger transaction. Zero is a valid Java
  // integer bit pattern, and wrap skips every live collision.
  FinishLedger allocator;
  assert(allocator.TryRegister(std::numeric_limits<uint32_t>::max()));
  assert(allocator.TryRegister(0));
  assert(allocator.TryRegister(1));
  uint32_t cursor = std::numeric_limits<uint32_t>::max();
  uint32_t allocated = 0xabcdef01;
  assert(allocator.TryRegisterNext(&cursor, &allocated));
  assert(allocated == 2 && cursor == 3);
  uint32_t aliased = 7;
  assert(!allocator.TryRegisterNext(&aliased, &aliased));
  assert(aliased == 7);

  FinishLedger full;
  for (uint32_t sequence = 0; sequence < FinishLedger::kCapacity; ++sequence)
    assert(full.TryRegister(sequence));
  cursor = 0x12345678;
  allocated = 0x87654321;
  assert(!full.TryRegisterNext(&cursor, &allocated));
  assert(cursor == 0x12345678 && allocated == 0x87654321);

  // An observed-but-unaccepted ACK remains a live sequence identity. Only an
  // exact accepted claim after observation closure retires it for reuse.
  FinishLedger delayed;
  assert(delayed.TryRegister(10));
  assert(delayed.Record(10, true));
  bool delayed_handled = false;
  assert(delayed.CloseObservation(10, &delayed_handled) && delayed_handled);
  cursor = 10;
  allocated = 0;
  assert(delayed.TryRegisterNext(&cursor, &allocated));
  assert(allocated == 11);
  assert(delayed.Cancel(11));
  FinishAck delayed_snapshot;
  uint64_t delayed_ticket = 0;
  assert(delayed.ClaimPendingAck(&delayed_snapshot, &delayed_ticket));
  assert(delayed.CompleteAckSubmission(delayed_ticket, true));
  cursor = 10;
  allocated = 0;
  assert(delayed.TryRegisterNext(&cursor, &allocated));
  assert(allocated == 10);

  FinishLedger ledger;
  for (uint32_t sequence = 1; sequence <= FinishLedger::kCapacity; ++sequence)
    assert(ledger.TryRegister(sequence));
  assert(!ledger.TryRegister(1));  // duplicate does not consume capacity
  assert(ledger.OverflowCount() == 0);
  assert(!ledger.TryRegister(257));
  assert(ledger.OverflowCount() == 1);

  assert(ledger.Record(1, true));
  assert(!ledger.Record(1, false));  // first ACK/handled value wins
  bool handled = false;
  assert(ledger.Take(1, &handled) && handled);
  assert(!ledger.Take(1, &handled));

  assert(ledger.Cancel(2));
  assert(!ledger.Cancel(2));  // already cancelled
  assert(ledger.TryRegister(257));
  assert(ledger.Record(257, false));
  assert(!ledger.Cancel(257));  // acknowledged entries cannot be cancelled
  assert(ledger.Take(257, nullptr));

  // The remaining original reservations were never evicted by a full
  // admission failure.
  for (uint32_t sequence = 3; sequence <= FinishLedger::kCapacity;
       ++sequence) {
    assert(ledger.Record(sequence, false));
    assert(ledger.Take(sequence, &handled) && !handled);
  }

  // Synchronous finish: ACK is accepted before the observation scope closes;
  // the reservation remains until CloseObservation.
  assert(ledger.TryRegister(1000));
  assert(ledger.Record(1000, true));
  assert(ledger.MarkAckAccepted(1000));
  assert(!ledger.TryRegister(1000));
  assert(ledger.CloseObservation(1000, &handled) && handled);
  assert(ledger.TryRegister(1000));
  assert(ledger.Cancel(1000));

  // Asynchronous finish: observation closes first, then the accepted ACK
  // settles the record and returns capacity.
  assert(ledger.TryRegister(1001));
  assert(!ledger.CloseObservation(1001, &handled));
  assert(ledger.Record(1001, false));
  assert(ledger.MarkAckAccepted(1001));
  assert(ledger.TryRegister(1001));
  assert(ledger.Cancel(1001));

  // An ACK send failure leaves the observed reservation for an explicit retry.
  assert(ledger.TryRegister(1002));
  assert(ledger.Record(1002, true));
  assert(ledger.CloseObservation(1002, &handled) && handled);
  assert(!ledger.TryRegister(1002));
  assert(ledger.MarkAckAccepted(1002));

  // More than one full ledger of asynchronous finishes must recycle slots,
  // rather than evicting unrelated outstanding obligations.
  for (uint32_t sequence = 2000; sequence < 2300; ++sequence) {
    assert(ledger.TryRegister(sequence));
    assert(!ledger.CloseObservation(sequence, &handled));
    assert(ledger.Record(sequence, (sequence & 1) != 0));
    assert(ledger.MarkAckAccepted(sequence));
  }

  // ClaimPendingAck preserves caller outputs when there is no pending ACK,
  // then claims the earliest recorded value without allowing legacy helpers
  // to steal the in-flight reservation.
  FinishAck untouched;
  untouched.origin.sequence = 4242;
  uint64_t untouched_ticket = 4242;
  assert(!ledger.ClaimPendingAck(&untouched, &untouched_ticket));
  assert(untouched.origin.sequence == 4242 && untouched_ticket == 4242);

  InputEventOrigin first_origin{ReceiverPacketOrigin::kImportedChannel, 4000};
  assert(ledger.TryRegister(4000, first_origin));
  assert(ledger.Record(4000, true));
  FinishAck first_snapshot;
  uint64_t first_ticket = 0;
  assert(ledger.ClaimPendingAck(&first_snapshot, &first_ticket));
  assert(first_snapshot.origin.sequence == 4000 && first_snapshot.handled);
  assert(ledger.HasPendingAck());
  assert(!ledger.Take(4000, &handled));
  assert(!ledger.MarkAckAccepted(4000));
  // A deferred send releases only the claim; the recorded value is offered
  // again and receives a distinct single-use claim ticket.
  assert(ledger.CompleteAckSubmission(first_ticket, false));
  FinishAck retry_snapshot;
  uint64_t retry_ticket = 0;
  assert(ledger.ClaimPendingAck(&retry_snapshot, &retry_ticket));
  assert(retry_ticket != first_ticket && retry_snapshot.handled);
  assert(!ledger.CompleteAckSubmission(first_ticket, true));
  assert(ledger.CloseObservation(4000, &handled) && handled);
  assert(ledger.CompleteAckSubmission(retry_ticket, true));
  assert(!ledger.HasPendingAck());

  // A claimed head does not block an independent later entry from being
  // claimed, while legacy sequence APIs still reject both claimed entries.
  assert(ledger.TryRegister(4001, {}, nullptr));
  assert(ledger.TryRegister(4002, {}, nullptr));
  assert(ledger.Record(4001, false));
  assert(ledger.Record(4002, true));
  FinishAck head_snapshot;
  FinishAck next_snapshot;
  uint64_t head_ticket = 0;
  uint64_t next_ticket = 0;
  assert(ledger.ClaimPendingAck(&head_snapshot, &head_ticket));
  assert(head_snapshot.origin.sequence == 0 && !head_snapshot.handled);
  assert(ledger.ClaimPendingAck(&next_snapshot, &next_ticket));
  assert(next_ticket != head_ticket && next_snapshot.handled);
  assert(!ledger.Take(4001, nullptr) && !ledger.MarkAckAccepted(4001));
  assert(!ledger.Take(4002, nullptr) && !ledger.MarkAckAccepted(4002));
  assert(ledger.CompleteAckSubmission(next_ticket, true));
  assert(ledger.CompleteAckSubmission(head_ticket, true));
  assert(ledger.CloseObservation(4001, nullptr));
  assert(ledger.CloseObservation(4002, nullptr));
  assert(!ledger.HasPendingAck());

  // Terminal completion is final but is intentionally not acceptance. After
  // the terminal entry retires, reusing its framework sequence gets a fresh
  // ticket; the stale terminal ticket cannot complete the new claim.
  assert(ledger.TryRegister(4003));
  assert(ledger.Record(4003, true));
  assert(ledger.CloseObservation(4003, &handled) && handled);
  FinishAck terminal_snapshot;
  uint64_t terminal_ticket = 0;
  assert(ledger.ClaimPendingAck(&terminal_snapshot, &terminal_ticket));
  assert(ledger.CompleteAckTerminal(terminal_ticket));
  assert(!ledger.MarkAckAccepted(4003));
  assert(!ledger.HasPendingAck());
  assert(ledger.TryRegister(4003));
  assert(ledger.Record(4003, false));
  FinishAck reused_snapshot;
  uint64_t reused_ticket = 0;
  assert(ledger.ClaimPendingAck(&reused_snapshot, &reused_ticket));
  assert(reused_ticket != terminal_ticket);
  assert(!ledger.CompleteAckSubmission(terminal_ticket, true));
  assert(ledger.HasPendingAck());
  assert(ledger.CompleteAckSubmission(reused_ticket, true));
  assert(ledger.CloseObservation(4003, &handled) && !handled);
  assert(!ledger.HasPendingAck());

  // Accepted before observation closes the entry only when the observation
  // later closes, covering the other side of the completion ordering.
  assert(ledger.TryRegister(4004));
  assert(ledger.Record(4004, true));
  FinishAck before_close_snapshot;
  uint64_t before_close_ticket = 0;
  assert(ledger.ClaimPendingAck(&before_close_snapshot, &before_close_ticket));
  assert(ledger.CompleteAckSubmission(before_close_ticket, true));
  assert(!ledger.HasPendingAck());
  assert(ledger.CloseObservation(4004, &handled) && handled);

  // Releasing a retained sink may synchronously reenter this owner. It must
  // run after the ledger lock is released, including pre-invocation cancel.
  bool released = false;
  auto sink = std::shared_ptr<const darwin_art::input::InputRoutingRecipient>(
      static_cast<const darwin_art::input::InputRoutingRecipient*>(nullptr),
      [&](const auto*) {
        released = true;
        assert(ledger.TryRegister(3001));
      });
  assert(ledger.TryRegister(3000, {}, sink));
  sink.reset();
  assert(!released && ledger.Cancel(3000) && released);
  assert(ledger.Cancel(3001));
  std::puts("finish ledger: bounded admission/no eviction/duplicate ACK PASS");
}
