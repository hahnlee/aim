#include "../../compat/window/blast_transaction_state.h"

#include <cassert>
#include <cstddef>
#include <cstdint>
#include <map>
#include <mutex>
#include <utility>
#include <vector>

// The NDK intentionally leaves this platform token opaque. The test gives it
// an owned identity so every production transfer can be checked independently.
struct ASurfaceTransaction {
  uint64_t id = 0;
};

using darwin_art::window::BlastTransactionState;

namespace {

std::map<ASurfaceTransaction*, int> delete_counts;
std::mutex* state_mutex = nullptr;
bool delete_while_mutex_held = false;
BlastTransactionState* reentrant_state = nullptr;
ASurfaceTransaction* reentrant_trigger = nullptr;
bool reentered = false;

ASurfaceTransaction* Token(uint64_t id) {
  return new ASurfaceTransaction{id};
}

int DeleteCount(ASurfaceTransaction* token) {
  return delete_counts[token];
}

void DeleteTransferred(std::vector<ASurfaceTransaction*> tokens) {
  for (ASurfaceTransaction* token : tokens) {
    ASurfaceTransaction_delete(token);
  }
}

}  // namespace

extern "C" void ASurfaceTransaction_delete(ASurfaceTransaction* token) {
  assert(token != nullptr);
  if (state_mutex != nullptr) {
    if (state_mutex->try_lock()) {
      state_mutex->unlock();
    } else {
      delete_while_mutex_held = true;
    }
  }
  ++delete_counts[token];
  if (reentrant_state == nullptr || token != reentrant_trigger || reentered) {
    delete token;
    return;
  }
  reentered = true;
  // CloseAndDiscard must have transferred the entire owned set before its
  // first platform delete. A reentrant close therefore observes no ownership
  // and cannot delete the same token a second time.
  reentrant_state->CloseAndDiscard();
  delete token;
}

int main() {
  {
    BlastTransactionState state;
    state_mutex = &state.mutex;
    ASurfaceTransaction* held = Token(1);
    ASurfaceTransaction* future = Token(2);
    ASurfaceTransaction* continuous = Token(3);
    {
      std::lock_guard<std::mutex> lock(state.mutex);
      assert(state.HoldTransactionLocked(held));
      state.QueueFutureLocked(future, 7);
      state.continuous_transaction = continuous;
    }
    reentrant_state = &state;
    reentrant_trigger = held;
    state.CloseAndDiscard();
    reentrant_state = nullptr;
    reentrant_trigger = nullptr;
    assert(reentered);
    assert(DeleteCount(held) == 1);
    assert(DeleteCount(future) == 1);
    assert(DeleteCount(continuous) == 1);
    assert(!delete_while_mutex_held);
    state.CloseAndDiscard();
    assert(DeleteCount(held) == 1);
    assert(DeleteCount(future) == 1);
    assert(DeleteCount(continuous) == 1);
    state_mutex = nullptr;
  }

  // Destruction is the same transfer boundary and must delete each token
  // once, even though no caller explicitly invokes CloseAndDiscard.
  {
    auto* state = new BlastTransactionState();
    state_mutex = &state->mutex;
    ASurfaceTransaction* token = Token(4);
    {
      std::lock_guard<std::mutex> lock(state->mutex);
      state->QueueFutureLocked(token, 1);
    }
    delete state;
    state_mutex = nullptr;
    assert(DeleteCount(token) == 1);
    assert(!delete_while_mutex_held);
  }

  // TakeDueLocked transfers only due entries in original FIFO order; the
  // caller is responsible for deletion after releasing the state mutex.
  {
    BlastTransactionState state;
    state_mutex = &state.mutex;
    ASurfaceTransaction* frame_nine = Token(9);
    ASurfaceTransaction* frame_four = Token(4);
    ASurfaceTransaction* frame_seven = Token(7);
    std::vector<ASurfaceTransaction*> due;
    {
      std::lock_guard<std::mutex> lock(state.mutex);
      state.QueueFutureLocked(frame_nine, 9);
      state.QueueFutureLocked(frame_four, 4);
      state.QueueFutureLocked(frame_seven, 7);
      due = state.TakeDueLocked(7, false);
      assert(due.size() == 2);
      assert(due[0] == frame_four);
      assert(due[1] == frame_seven);
      assert(DeleteCount(frame_four) == 0);
      assert(DeleteCount(frame_seven) == 0);
      assert(state.future_transactions.size() == 1);
    }
    DeleteTransferred(std::move(due));
    assert(DeleteCount(frame_four) == 1);
    assert(DeleteCount(frame_seven) == 1);
    std::vector<ASurfaceTransaction*> remaining;
    {
      std::lock_guard<std::mutex> lock(state.mutex);
      remaining = state.TakeDueLocked(0, true);
      assert(remaining.size() == 1);
      assert(remaining[0] == frame_nine);
      assert(DeleteCount(frame_nine) == 0);
    }
    DeleteTransferred(std::move(remaining));
    assert(DeleteCount(frame_nine) == 1);
    assert(!delete_while_mutex_held);
    state_mutex = nullptr;
  }

  // The held queue is bounded. The rejected token remains caller-owned and
  // is deleted exactly once outside the mutex; accepted tokens are state-owned.
  {
    BlastTransactionState state;
    state_mutex = &state.mutex;
    std::vector<ASurfaceTransaction*> accepted;
    accepted.reserve(BlastTransactionState::kMaxHeldTransactions);
    ASurfaceTransaction* rejected = nullptr;
    {
      std::lock_guard<std::mutex> lock(state.mutex);
      for (size_t index = 0; index < BlastTransactionState::kMaxHeldTransactions;
           ++index) {
        accepted.push_back(Token(static_cast<uint64_t>(index + 100)));
        assert(state.HoldTransactionLocked(accepted.back()));
      }
      rejected = Token(999);
      assert(!state.HoldTransactionLocked(rejected));
      assert(state.held_transactions.size() ==
             BlastTransactionState::kMaxHeldTransactions);
    }
    state.CloseAndDiscard();
    ASurfaceTransaction_delete(rejected);
    for (ASurfaceTransaction* token : accepted) assert(DeleteCount(token) == 1);
    assert(DeleteCount(rejected) == 1);
    assert(!delete_while_mutex_held);
    state_mutex = nullptr;
  }
  return 0;
}
