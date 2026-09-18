#pragma once

#include <android/surface_control.h>

#include <cstddef>
#include <cstdint>
#include <deque>
#include <mutex>
#include <vector>

namespace darwin_art::window {

// Owns the non-JNI part of a BLAST queue's transaction protocol.  The JNI
// adapter deliberately keeps JavaVM/global-reference state outside this
// object; it only observes these fields while holding mutex and performs all
// callbacks after releasing it.
class BlastTransactionState final {
 public:
  struct PendingTransaction {
    ASurfaceTransaction* transaction = nullptr;
    uint64_t frame = 0;
  };

  enum class SyncPhase : uint8_t { kIdle, kReserved, kGated, kDraining };

  static constexpr size_t kMaxHeldTransactions = 8;

  BlastTransactionState() = default;
  BlastTransactionState(const BlastTransactionState&) = delete;
  BlastTransactionState& operator=(const BlastTransactionState&) = delete;
  ~BlastTransactionState();

  // The adapter may compose its opaque-consumer decision with the transaction
  // decision under this mutex. No JNI or platform callback is made while it
  // is held.
  std::mutex mutex;

  bool destroyed = false;
  bool continuous_sync = false;
  ASurfaceTransaction* continuous_transaction = nullptr;
  uint64_t continuous_frame = 0;
  std::deque<PendingTransaction> future_transactions;
  bool outstanding_sync = false;
  SyncPhase sync_phase = SyncPhase::kIdle;
  uint64_t sync_generation = 0;
  std::deque<ASurfaceTransaction*> held_transactions;
  uint64_t last_acquired_frame = 0;

  // Close invalidates all generations before the platform callback observer
  // is detached. It intentionally does not call platform code under mutex.
  void CloseLocked();

  // Transfers every transaction still owned by this state. The caller must
  // destroy the returned transactions after releasing mutex. This is the
  // common close/clear primitive and prevents double apply/delete ownership.
  std::vector<ASurfaceTransaction*> TakeAllOwnedLocked();

  // Deletes any transactions currently owned by the state, outside its lock.
  void CloseAndDiscard();

  std::vector<ASurfaceTransaction*> TakeDueLocked(uint64_t frame, bool all);

  // Admits a transaction behind an outstanding Java commit gate. The caller
  // retains ownership when this returns false (bounded overflow or null).
  bool HoldTransactionLocked(ASurfaceTransaction* transaction);

  // Takes ownership of a frame-indexed transaction. The owner is the sole
  // place that appends to the pending FIFO.
  void QueueFutureLocked(ASurfaceTransaction* transaction, uint64_t frame);

  void NoteAcquiredFrameLocked(uint64_t frame);
  uint64_t BeginSyncGenerationLocked();
};

}  // namespace darwin_art::window
