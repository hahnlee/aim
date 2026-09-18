#include "../../compat/window/surface_transaction_submission.h"

#include <android/hardware_buffer.h>

#include <cassert>
#include <cstdint>
#include <atomic>
#include <mutex>
#include <thread>
#include <vector>

using darwin_art::window::SurfaceTransaction;
using darwin_art::window::SurfaceTransactionStats;
using darwin_art::window::SurfaceTransactionSubmission;
using darwin_art::window::SurfaceTransactionSubmissionResult;

namespace {

std::atomic<int> g_ready{1};
std::atomic<int> g_apply_count{0};
std::atomic<int> g_port_ready{1};
std::atomic<int> g_buffer_release_count{0};
std::atomic<int> g_control_release_count{0};
std::atomic<int> g_fence_close_count{0};
std::atomic<int> g_fence_dup_count{0};
std::atomic<int> g_fence_wait_count{0};
std::atomic<int> g_blocking_wait_count{0};
std::mutex g_order_mutex;
std::vector<int> g_order;

template <typename T>
T* Fake(uintptr_t value) {
  return reinterpret_cast<T*>(value);
}

void Record(int value) {
  std::lock_guard<std::mutex> lock(g_order_mutex);
  g_order.push_back(value);
}

void Complete(void* context, ASurfaceTransactionStats*) {
  auto* owner = static_cast<SurfaceTransactionSubmission*>(context);
  owner->CloseAdmission();
  // The callback itself is part of the active count.  Quiescence cannot be
  // reported until this callback and the enclosing stats/resource cleanup
  // have returned.
  assert(!owner->PollQuiesced());
}

void ReentrantComplete(void* context, ASurfaceTransactionStats*) {
  auto* owner = static_cast<SurfaceTransactionSubmission*>(context);
  SurfaceTransaction nested;
  nested.updates.push_back({.acquire_fence = -1, .position_x = 1000});
  assert(owner->SubmitSurfaceTransaction(&nested) ==
         SurfaceTransactionSubmissionResult::kAccepted);
  assert(nested.updates.empty());
}

struct SynchronousReentrantContext {
  SurfaceTransactionSubmission* owner = nullptr;
  std::atomic<bool> entered{false};
  std::atomic<bool> release{false};
};

void SynchronousReentrantComplete(void* context, ASurfaceTransactionStats*) {
  auto* state = static_cast<SynchronousReentrantContext*>(context);
  state->entered.store(true, std::memory_order_release);
  SurfaceTransaction nested;
  nested.updates.push_back({.acquire_fence = -1, .position_x = 501});
  assert(state->owner->SubmitSurfaceTransaction(&nested) ==
         SurfaceTransactionSubmissionResult::kAccepted);
  assert(nested.updates.empty());
  while (!state->release.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
}

void Discard(void*, int fence) {
  Record(fence);
  assert(fence == 407);
}

void DiscardQuarantine(void*, int fence) {
  Record(fence);
  assert(fence == -2);
}

}  // namespace

extern "C" void AHardwareBuffer_release(AHardwareBuffer*) {
  g_buffer_release_count.fetch_add(1, std::memory_order_release);
}

extern "C" void ASurfaceControl_release(ASurfaceControl*) {
  g_control_release_count.fetch_add(1, std::memory_order_release);
}

extern "C" int darwin_art_bionic_socket_broker_close(int) {
  g_fence_close_count.fetch_add(1, std::memory_order_release);
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_dup(int fence) {
  g_fence_dup_count.fetch_add(1, std::memory_order_release);
  if (fence == 8) return -1;
  return fence + 400;
}

extern "C" int sync_wait(int, int timeout_ms) {
  g_fence_wait_count.fetch_add(1, std::memory_order_release);
  if (timeout_ms < 0) {
    g_blocking_wait_count.fetch_add(1, std::memory_order_release);
  }
  return g_ready.load(std::memory_order_acquire) == 0 ? -1 : 0;
}

extern "C" void darwin_art_android_mark_hardware_buffer_released(void*) {}

bool darwin_art::window::ApplyReadySurfaceTransaction(
    SurfaceTransaction* transaction, SurfaceTransactionStats* stats) {
  g_apply_count.fetch_add(1, std::memory_order_release);
  if (transaction == nullptr || g_ready.load(std::memory_order_acquire) == 0 ||
      g_port_ready.load(std::memory_order_acquire) == 0) {
    return false;
  }
  if (stats != nullptr) stats->present_fence = 91;
  Record(transaction->updates.empty() ? -1
                                      : transaction->updates.front().position_x);
  return true;
}

int main() {
  // A ready transaction applies synchronously, while callback-owned admission
  // remains active until callback and stats/resource cleanup have returned.
  SurfaceTransactionSubmission synchronous;
  SurfaceTransaction ready;
  ready.updates.push_back({.acquire_fence = -1});
  ready.completes.push_back({&Complete, &synchronous});
  assert(synchronous.SubmitSurfaceTransaction(&ready) ==
         SurfaceTransactionSubmissionResult::kAccepted);
  assert(ready.updates.empty());
  assert(synchronous.PollQuiesced());
  assert(synchronous.Reopen());

  // Close is idempotent.  There is no worker in this epoch, so it is already
  // joined once the synchronous callback has returned.
  synchronous.CloseAdmission();
  assert(synchronous.PollQuiesced());

  // A synchronous callback can re-enter submission.  The nested ready batch
  // must be queued behind the active parent, and the single worker cannot
  // apply it while the callback is still holding the parent's cleanup open.
  g_apply_count.store(0, std::memory_order_release);
  g_buffer_release_count.store(0, std::memory_order_release);
  {
    std::lock_guard<std::mutex> lock(g_order_mutex);
    g_order.clear();
  }
  SurfaceTransactionSubmission synchronous_reentrant;
  SynchronousReentrantContext reentrant_context{.owner = &synchronous_reentrant};
  SurfaceTransaction parent;
  parent.updates.push_back({.buffer = Fake<AHardwareBuffer>(0x200),
                            .has_buffer = true,
                            .position_x = 500});
  parent.completes.push_back({&SynchronousReentrantComplete,
                              &reentrant_context});
  SurfaceTransactionSubmissionResult parent_result =
      SurfaceTransactionSubmissionResult::kFailed;
  std::thread synchronous_caller([&] {
    parent_result = synchronous_reentrant.SubmitSurfaceTransaction(&parent);
  });
  while (!reentrant_context.entered.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  assert(g_apply_count.load(std::memory_order_acquire) == 1);
  assert(g_buffer_release_count.load(std::memory_order_acquire) == 0);
  assert(!synchronous_reentrant.PollQuiesced());
  reentrant_context.release.store(true, std::memory_order_release);
  synchronous_caller.join();
  assert(parent_result == SurfaceTransactionSubmissionResult::kAccepted);
  // CompleteSurfaceTransaction releases the parent buffer and stats before
  // the worker is allowed to run the nested ready operation.
  assert(g_buffer_release_count.load(std::memory_order_acquire) == 1);
  while (g_apply_count.load(std::memory_order_acquire) < 2) {
    std::this_thread::yield();
  }
  {
    std::lock_guard<std::mutex> lock(g_order_mutex);
    assert(g_order.size() == 2);
    assert(g_order[0] == 500 && g_order[1] == 501);
  }
  synchronous_reentrant.CloseAdmission();
  while (!synchronous_reentrant.PollQuiesced()) std::this_thread::yield();

  // An unsignaled acquire fence goes to exactly one FIFO worker.  Closing the
  // owner wakes it, and lifetime discard forwards a duplicate of that fence.
  g_ready.store(0, std::memory_order_release);
  SurfaceTransactionSubmission async;
  SurfaceTransaction blocked;
  blocked.updates.push_back({.opaque = Fake<ASurfaceControl>(1),
                             .acquire_fence = 7,
                             .has_buffer = true});
  blocked.buffer_callbacks.push_back(
      {.control = Fake<ASurfaceControl>(1), .discard = &Discard});
  assert(async.SubmitSurfaceTransaction(&blocked) ==
         SurfaceTransactionSubmissionResult::kAccepted);
  assert(blocked.updates.empty());
  async.CloseAdmission();
  assert(!async.PollQuiesced());
  // Two callers may poll concurrently, but only the caller that successfully
  // owns the join may report completion.  Reopen remains false until that
  // joined snapshot is visible.
  std::atomic<bool> poll_go{false};
  std::atomic<bool> poll_one{false};
  std::atomic<bool> poll_two{false};
  std::thread poller_one([&] {
    while (!poll_go.load(std::memory_order_acquire)) std::this_thread::yield();
    poll_one.store(async.PollQuiesced(), std::memory_order_release);
  });
  std::thread poller_two([&] {
    while (!poll_go.load(std::memory_order_acquire)) std::this_thread::yield();
    poll_two.store(async.PollQuiesced(), std::memory_order_release);
  });
  poll_go.store(true, std::memory_order_release);
  poller_one.join();
  poller_two.join();
  while (!async.PollQuiesced()) std::this_thread::yield();
  assert(poll_one.load(std::memory_order_acquire) ||
         poll_two.load(std::memory_order_acquire));
  assert(g_fence_dup_count.load(std::memory_order_acquire) == 1);
  assert(async.Reset());

  // A dup failure uses the lifetime owner's immediate private quarantine
  // sentinel; it must not fall back to sync_wait(..., -1) during close.
  const int blocking_waits_before_dup_failure =
      g_blocking_wait_count.load(std::memory_order_acquire);
  SurfaceTransaction dup_failure;
  dup_failure.updates.push_back({.opaque = Fake<ASurfaceControl>(2),
                                 .acquire_fence = 8,
                                 .has_buffer = true});
  dup_failure.buffer_callbacks.push_back(
      {.control = Fake<ASurfaceControl>(2), .discard = &DiscardQuarantine});
  assert(async.SubmitSurfaceTransaction(&dup_failure) ==
         SurfaceTransactionSubmissionResult::kAccepted);
  assert(dup_failure.updates.empty());
  async.CloseAdmission();
  while (!async.PollQuiesced()) std::this_thread::yield();
  assert(g_fence_dup_count.load(std::memory_order_acquire) == 2);
  assert(g_blocking_wait_count.load(std::memory_order_acquire) ==
         blocking_waits_before_dup_failure);

  // FIFO saturation beyond the old fixed held-transaction limit does not
  // reject admitted payload.  The first callback may submit reentrantly.
  g_ready.store(0, std::memory_order_release);
  g_apply_count.store(0, std::memory_order_release);
  {
    std::lock_guard<std::mutex> lock(g_order_mutex);
    g_order.clear();
  }
  SurfaceTransactionSubmission fifo;
  for (int i = 0; i < 32; ++i) {
    SurfaceTransaction transaction;
    transaction.updates.push_back({.acquire_fence = i == 0 ? 7 : -1,
                                   .position_x = i});
    if (i == 0) transaction.completes.push_back({&ReentrantComplete, &fifo});
    assert(fifo.SubmitSurfaceTransaction(&transaction) ==
           SurfaceTransactionSubmissionResult::kAccepted);
  }
  g_ready.store(1, std::memory_order_release);
  while (g_apply_count.load(std::memory_order_acquire) < 33) {
    std::this_thread::yield();
  }
  fifo.CloseAdmission();
  while (!fifo.PollQuiesced()) std::this_thread::yield();
  assert(g_apply_count.load(std::memory_order_acquire) >= 33);
  {
    std::lock_guard<std::mutex> lock(g_order_mutex);
    assert(g_order.size() == 33);
    for (int i = 0; i < 32; ++i) assert(g_order[static_cast<size_t>(i)] == i);
    assert(g_order[32] == 1000);
  }

  // A rejected closed admission leaves the internal public payload untouched;
  // C ABI callers separately clear/discard it through the lifetime owner.
  SurfaceTransaction rejected;
  rejected.updates.push_back({.acquire_fence = -1});
  assert(fifo.SubmitSurfaceTransaction(&rejected) ==
         SurfaceTransactionSubmissionResult::kClosed);
  assert(rejected.updates.size() == 1);
  assert(g_fence_wait_count.load(std::memory_order_acquire) > 0);
  return 0;
}
