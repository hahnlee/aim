#include "finish_ledger.h"

#include <utility>

namespace darwin_art::input {

RemoteFinishState FinishLedger::RemoteStateLocked() const {
  RemoteFinishState state;
  state.sealed = remote_admission_sealed_;
  for (size_t i = 0; i < count_; ++i) {
    const auto& entry = entries_[i];
    if (entry.origin.kind == ReceiverPacketOrigin::kImportedChannel &&
        !entry.ack_accepted && !entry.ack_terminal)
      ++state.outstanding_remote;
  }
  return state;
}

RemoteFinishState FinishLedger::SealRemoteAdmission() {
  std::lock_guard<std::mutex> lock(mutex_);
  remote_admission_sealed_ = true;
  return RemoteStateLocked();
}

RemoteFinishState FinishLedger::RemoteState() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return RemoteStateLocked();
}

bool FinishLedger::HasOutstandingRemote() const {
  return RemoteState().outstanding_remote != 0;
}

uint64_t FinishLedger::AllocateTicketLocked() {
  // At most kCapacity tickets are live, so a free nonzero value must exist.
  // The scan makes wraparound deterministic without aliasing another active
  // submission attempt.
  uint64_t candidate = next_ticket_;
  for (;;) {
    if (candidate == 0) candidate = 1;
    bool live = false;
    for (size_t i = 0; i < count_; ++i) {
      if (entries_[i].claim_ticket == candidate) {
        live = true;
        break;
      }
    }
    if (!live) {
      next_ticket_ = candidate + 1;
      return candidate;
    }
    ++candidate;
  }
}

bool FinishLedger::TryRegister(
    uint32_t sequence, InputEventOrigin origin,
    std::shared_ptr<const InputRoutingRecipient> recipient) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (remote_admission_sealed_ &&
      origin.kind == ReceiverPacketOrigin::kImportedChannel)
    return false;
  for (size_t i = 0; i < count_; ++i) {
    if (entries_[i].sequence == sequence) return false;
  }
  if (count_ == entries_.size()) {
    ++overflow_count_;
    return false;
  }
  Entry entry;
  entry.sequence = sequence;
  entry.origin = origin;
  entry.recipient = std::move(recipient);
  entries_[count_++] = std::move(entry);
  return true;
}

bool FinishLedger::TryRegisterNext(
    uint32_t* next_sequence, uint32_t* allocated, InputEventOrigin origin,
    std::shared_ptr<const InputRoutingRecipient> recipient) {
  if (next_sequence == nullptr || allocated == nullptr ||
      next_sequence == allocated)
    return false;
  uint32_t chosen = 0;
  bool chosen_found = false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (remote_admission_sealed_ &&
        origin.kind == ReceiverPacketOrigin::kImportedChannel)
      return false;
    if (count_ == entries_.size()) {
      ++overflow_count_;
      return false;
    }

    const uint32_t start = *next_sequence;
    for (size_t offset = 0; offset <= count_; ++offset) {
      const uint32_t candidate = start + static_cast<uint32_t>(offset);
      bool live = false;
      for (size_t i = 0; i < count_; ++i) {
        if (entries_[i].sequence == candidate) {
          live = true;
          break;
        }
      }
      if (!live) {
        chosen = candidate;
        chosen_found = true;
        break;
      }
    }
    // A non-full ledger has a free sequence in the first count_ + 1
    // candidates. Keep the found state separate because zero is valid.
    if (!chosen_found) return false;

    Entry entry;
    entry.sequence = chosen;
    entry.origin = origin;
    entry.recipient = std::move(recipient);
    entries_[count_++] = std::move(entry);
    // Publish outputs only after the entry is in the ledger. The caller sees
    // no cursor/sequence reservation if the ledger cannot accept it.
    *allocated = chosen;
    *next_sequence = chosen + 1;
  }
  return true;
}

bool FinishLedger::Cancel(uint32_t sequence) {
  Entry retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t index = 0;
    for (; index < count_; ++index) {
      if (entries_[index].sequence == sequence) break;
    }
    if (index == count_ || entries_[index].ack_recorded ||
        entries_[index].claimed)
      return false;
    retired = std::move(entries_[index]);
    // Shift through the empty slot with swaps. Move assignment into a live
    // Entry could release its recipient while mutex_ is held.
    for (size_t i = index; i + 1 < count_; ++i)
      std::swap(entries_[i], entries_[i + 1]);
    --count_;
  }
  return true;
}

bool FinishLedger::Record(uint32_t sequence, bool handled, FinishAck* ack) {
  FinishAck snapshot;
  bool recorded = false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    for (size_t i = 0; i < count_; ++i) {
      auto& entry = entries_[i];
      if (entry.sequence != sequence) continue;
      if (entry.ack_recorded) return false;
      entry.ack_recorded = true;
      entry.handled = handled;
      if (ack != nullptr)
        snapshot = {entry.origin, entry.recipient, entry.handled};
      recorded = true;
      break;
    }
  }
  // Replacing a caller-owned recipient can release the previous resource;
  // perform that assignment after leaving the ledger mutex.
  if (recorded && ack != nullptr) *ack = std::move(snapshot);
  return recorded;
}

bool FinishLedger::ClaimPendingAck(FinishAck* snapshot, uint64_t* ticket) {
  if (snapshot == nullptr || ticket == nullptr) return false;
  FinishAck pending;
  uint64_t pending_ticket = 0;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    for (size_t i = 0; i < count_; ++i) {
      auto& entry = entries_[i];
      if (!entry.ack_recorded || entry.ack_accepted || entry.ack_terminal ||
          entry.claimed)
        continue;
      pending = {entry.origin, entry.recipient, entry.handled};
      pending_ticket = AllocateTicketLocked();
      entry.claimed = true;
      entry.claim_ticket = pending_ticket;
      break;
    }
  }
  if (pending_ticket == 0) return false;
  // Do not overwrite either output until a claim was found. In particular,
  // releasing an old snapshot's recipient never occurs under mutex_.
  *snapshot = std::move(pending);
  *ticket = pending_ticket;
  return true;
}

bool FinishLedger::CompleteAckSubmission(uint64_t ticket, bool accepted) {
  if (ticket == 0) return false;
  Entry retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t index = 0;
    for (; index < count_; ++index) {
      if (entries_[index].claim_ticket == ticket) break;
    }
    if (index == count_ || !entries_[index].claimed ||
        !entries_[index].ack_recorded || entries_[index].ack_accepted ||
        entries_[index].ack_terminal)
      return false;
    entries_[index].claimed = false;
    entries_[index].claim_ticket = 0;
    if (!accepted) return true;
    entries_[index].ack_accepted = true;
    if (entries_[index].observer_open) return true;
    retired = std::move(entries_[index]);
    for (size_t i = index; i + 1 < count_; ++i)
      std::swap(entries_[i], entries_[i + 1]);
    --count_;
  }
  return true;
}

bool FinishLedger::CompleteAckTerminal(uint64_t ticket) {
  if (ticket == 0) return false;
  Entry retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t index = 0;
    for (; index < count_; ++index) {
      if (entries_[index].claim_ticket == ticket) break;
    }
    if (index == count_ || !entries_[index].claimed ||
        !entries_[index].ack_recorded || entries_[index].ack_accepted ||
        entries_[index].ack_terminal)
      return false;
    entries_[index].claimed = false;
    entries_[index].claim_ticket = 0;
    entries_[index].ack_terminal = true;
    if (entries_[index].observer_open) return true;
    retired = std::move(entries_[index]);
    for (size_t i = index; i + 1 < count_; ++i)
      std::swap(entries_[i], entries_[i + 1]);
    --count_;
  }
  return true;
}

bool FinishLedger::Take(uint32_t sequence, bool* handled) {
  Entry retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t index = 0;
    for (; index < count_; ++index) {
      if (entries_[index].sequence == sequence) break;
    }
    if (index == count_ || !entries_[index].ack_recorded ||
        entries_[index].claimed)
      return false;
    if (handled != nullptr) *handled = entries_[index].handled;
    retired = std::move(entries_[index]);
    for (size_t i = index; i + 1 < count_; ++i)
      std::swap(entries_[i], entries_[i + 1]);
    --count_;
  }
  return true;
}

bool FinishLedger::CloseObservation(uint32_t sequence, bool* handled) {
  Entry retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t index = 0;
    for (; index < count_; ++index) {
      if (entries_[index].sequence == sequence) break;
    }
    if (index == count_ || !entries_[index].observer_open) return false;
    entries_[index].observer_open = false;
    if (!entries_[index].ack_recorded) return false;
    if (handled != nullptr) *handled = entries_[index].handled;
    if ((!entries_[index].ack_accepted && !entries_[index].ack_terminal) ||
        entries_[index].claimed)
      return true;
    retired = std::move(entries_[index]);
    for (size_t i = index; i + 1 < count_; ++i)
      std::swap(entries_[i], entries_[i + 1]);
    --count_;
  }
  return true;
}

bool FinishLedger::MarkAckAccepted(uint32_t sequence) {
  Entry retired;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    size_t index = 0;
    for (; index < count_; ++index) {
      if (entries_[index].sequence == sequence) break;
    }
    if (index == count_ || !entries_[index].ack_recorded ||
        entries_[index].ack_accepted || entries_[index].ack_terminal ||
        entries_[index].claimed)
      return false;
    entries_[index].ack_accepted = true;
    if (entries_[index].observer_open) return true;
    retired = std::move(entries_[index]);
    for (size_t i = index; i + 1 < count_; ++i)
      std::swap(entries_[i], entries_[i + 1]);
    --count_;
  }
  return true;
}

bool FinishLedger::HasPendingAck() const {
  std::lock_guard<std::mutex> lock(mutex_);
  for (size_t i = 0; i < count_; ++i) {
    if (entries_[i].ack_recorded && !entries_[i].ack_accepted &&
        !entries_[i].ack_terminal)
      return true;
  }
  return false;
}

uint64_t FinishLedger::OverflowCount() const {
  std::lock_guard<std::mutex> lock(mutex_);
  return overflow_count_;
}

}  // namespace darwin_art::input
