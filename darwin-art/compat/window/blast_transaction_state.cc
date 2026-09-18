#include "blast_transaction_state.h"

#include <algorithm>

namespace darwin_art::window {

BlastTransactionState::~BlastTransactionState() { CloseAndDiscard(); }

void BlastTransactionState::CloseLocked() {
  destroyed = true;
  outstanding_sync = false;
  continuous_sync = false;
  sync_phase = SyncPhase::kIdle;
  ++sync_generation;
}

std::vector<ASurfaceTransaction*>
BlastTransactionState::TakeAllOwnedLocked() {
  std::vector<ASurfaceTransaction*> result;
  result.reserve(held_transactions.size() + future_transactions.size() +
                 (continuous_transaction == nullptr ? 0 : 1));
  for (ASurfaceTransaction* transaction : held_transactions) {
    if (transaction != nullptr) result.push_back(transaction);
  }
  held_transactions.clear();
  for (const PendingTransaction& pending : future_transactions) {
    if (pending.transaction != nullptr) result.push_back(pending.transaction);
  }
  future_transactions.clear();
  if (continuous_transaction != nullptr) {
    result.push_back(continuous_transaction);
    continuous_transaction = nullptr;
  }
  continuous_frame = 0;
  return result;
}

void BlastTransactionState::CloseAndDiscard() {
  std::vector<ASurfaceTransaction*> owned;
  {
    std::lock_guard<std::mutex> lock(mutex);
    CloseLocked();
    owned = TakeAllOwnedLocked();
  }
  for (ASurfaceTransaction* transaction : owned) {
    ASurfaceTransaction_delete(transaction);
  }
}

std::vector<ASurfaceTransaction*> BlastTransactionState::TakeDueLocked(
    uint64_t frame, bool all) {
  std::vector<ASurfaceTransaction*> result;
  for (auto it = future_transactions.begin(); it != future_transactions.end();) {
    if (all || it->frame == 0 || (frame != 0 && it->frame <= frame)) {
      if (it->transaction != nullptr) result.push_back(it->transaction);
      it = future_transactions.erase(it);
    } else {
      ++it;
    }
  }
  return result;
}

bool BlastTransactionState::HoldTransactionLocked(
    ASurfaceTransaction* transaction) {
  if (transaction == nullptr ||
      held_transactions.size() >= kMaxHeldTransactions) {
    return false;
  }
  held_transactions.push_back(transaction);
  return true;
}

void BlastTransactionState::QueueFutureLocked(ASurfaceTransaction* transaction,
                                              uint64_t frame) {
  if (transaction != nullptr) {
    future_transactions.push_back({transaction, frame});
  }
}

void BlastTransactionState::NoteAcquiredFrameLocked(uint64_t frame) {
  last_acquired_frame = std::max(last_acquired_frame, frame);
}

uint64_t BlastTransactionState::BeginSyncGenerationLocked() {
  ++sync_generation;
  return sync_generation;
}

}  // namespace darwin_art::window
