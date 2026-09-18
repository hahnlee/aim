#include "compat/window/surface_transaction_builder.h"

#include <cassert>
#include <cstdint>
#include <cstdlib>
#include <new>
#include <vector>

using darwin_art::window::SurfaceTransaction;
using darwin_art::window::SurfaceTransactionBuilder;

extern "C" int darwin_art_bionic_socket_broker_close(int fd);

int g_allocations_before_failure = -1;

void* operator new(std::size_t size) {
  if (g_allocations_before_failure == 0) {
    g_allocations_before_failure = -1;
    throw std::bad_alloc();
  }
  if (g_allocations_before_failure > 0) --g_allocations_before_failure;
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete[](void* pointer, std::size_t) noexcept {
  std::free(pointer);
}

namespace {

int g_buffer_acquires = 0;
int g_buffer_releases = 0;
int g_control_acquires = 0;
int g_control_releases = 0;
int g_fence_dups = 0;
int g_fence_closes = 0;

template <typename T>
T* Fake(uintptr_t value) {
  return reinterpret_cast<T*>(value);
}

void ResetCounts() {
  g_buffer_acquires = 0;
  g_buffer_releases = 0;
  g_control_acquires = 0;
  g_control_releases = 0;
  g_fence_dups = 0;
  g_fence_closes = 0;
}

void ReleaseContents(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  for (auto& update : transaction->updates) {
    if (update.buffer != nullptr) AHardwareBuffer_release(update.buffer);
    if (update.acquire_fence >= 0)
      (void)darwin_art_bionic_socket_broker_close(update.acquire_fence);
    update.buffer = nullptr;
    update.acquire_fence = -1;
  }
  for (ASurfaceControl* control : transaction->controls) {
    if (control != nullptr) ASurfaceControl_release(control);
  }
  transaction->updates.clear();
  transaction->controls.clear();
  transaction->buffer_callbacks.clear();
}

struct ReentrantDiscard final {
  SurfaceTransaction* transaction = nullptr;
  ASurfaceControl* replacement_control = nullptr;
  AHardwareBuffer* replacement_buffer = nullptr;
  int replacement_fence = -1;
  int calls = 0;
  int received_fence = -1;
};

void ConsumeAndDeleteReuse(void* opaque, int fence) {
  auto* context = static_cast<ReentrantDiscard*>(opaque);
  ++context->calls;
  context->received_fence = fence;
  // The discard callback consumes the transferred duplicate. It then models
  // ASurfaceTransaction_delete followed by allocator reuse: the public
  // object is emptied and the replacement state is rebuilt synchronously.
  if (fence >= 0)
    (void)darwin_art_bionic_socket_broker_close(fence);
  ReleaseContents(context->transaction);
  context->transaction->~SurfaceTransaction();
  new (context->transaction) SurfaceTransaction();
  SurfaceTransactionBuilder nested(context->transaction);
  assert(nested.SetBuffer(context->replacement_control,
                          context->replacement_buffer,
                          context->replacement_fence));
}

void TestRepeatedReplacementAndRetention() {
  ResetCounts();
  SurfaceTransaction transaction;
  auto* control = Fake<ASurfaceControl>(0x100);
  auto* second_control = Fake<ASurfaceControl>(0x200);
  auto* first = Fake<AHardwareBuffer>(0x300);
  auto* second = Fake<AHardwareBuffer>(0x400);
  auto* third = Fake<AHardwareBuffer>(0x500);

  SurfaceTransactionBuilder builder(&transaction);
  assert(builder.SetPosition(control, 4, 5));
  assert(builder.SetScale(control, 2.0f, 3.0f));
  assert(builder.Remember(control));
  assert(transaction.controls.size() == 1);
  assert(g_control_acquires == 1);
  assert(builder.Remember(second_control));
  assert(transaction.controls.size() == 2);
  assert(g_control_acquires == 2);
  assert(builder.SetRelativeLayer(control, second_control, 9));
  assert(transaction.updates[0].has_relative_layer);
  assert(transaction.updates[0].relative_to == second_control);
  assert(transaction.updates[0].z_order == 9);

  assert(builder.SetBuffer(control, first, 10));
  transaction.buffer_callbacks.push_back(
      {.control = control, .discard = [](void*, int fence) {
         assert(fence == 1010);
         (void)darwin_art_bionic_socket_broker_close(fence);
       }});
  assert(builder.SetBuffer(control, second, 11));
  assert(g_buffer_acquires == 2);
  assert(g_buffer_releases == 1);
  assert(g_fence_dups == 1);
  assert(g_fence_closes == 2);  // callback duplicate and displaced fence
  assert(transaction.buffer_callbacks.empty());
  assert(transaction.updates.size() == 2);

  // A callback without a discard function still gets an operation-local
  // duplicate, which the batch must close itself rather than leak.
  transaction.buffer_callbacks.push_back({.control = control});
  assert(builder.SetBuffer(control, third, 12));
  assert(g_buffer_acquires == 3);
  assert(g_buffer_releases == 2);
  assert(g_fence_dups == 2);
  assert(g_fence_closes == 4);  // null callback duplicate and displaced fence
  ReleaseContents(&transaction);
  assert(g_buffer_releases == 3);
  assert(g_fence_closes == 5);  // final fence
  assert(g_control_releases == 2);
}

void TestFailedAdmissionClosesIncomingFence() {
  ResetCounts();
  SurfaceTransaction transaction;
  auto* control = Fake<ASurfaceControl>(0x110);
  auto* buffer = Fake<AHardwareBuffer>(0x310);
  assert(!SurfaceTransactionBuilder(nullptr).SetBuffer(control, buffer, 30));
  assert(g_buffer_acquires == 0 && g_fence_closes == 1);
  assert(!SurfaceTransactionBuilder(&transaction).SetBuffer(nullptr, buffer, 31));
  assert(g_buffer_acquires == 0 && g_fence_closes == 2);
  assert(transaction.updates.empty() && transaction.controls.empty());
}

void TestAllocationFailureLeavesDisplacedStateUntouched() {
  ResetCounts();
  SurfaceTransaction transaction;
  auto* control = Fake<ASurfaceControl>(0x115);
  auto* old_buffer = Fake<AHardwareBuffer>(0x315);
  auto* incoming = Fake<AHardwareBuffer>(0x415);
  SurfaceTransactionBuilder builder(&transaction);
  assert(builder.SetBuffer(control, old_buffer, 35));
  transaction.buffer_callbacks.reserve(1);
  transaction.buffer_callbacks.push_back(
      {.control = control, .discard = [](void*, int) { assert(false); }});
  const size_t original_updates = transaction.updates.size();
  g_allocations_before_failure = 0;
  assert(!builder.SetBuffer(control, incoming, 36));
  assert(transaction.updates.size() == original_updates);
  assert(transaction.updates[0].buffer == old_buffer);
  assert(transaction.updates[0].acquire_fence == 35);
  assert(transaction.buffer_callbacks.size() == 1);
  assert(g_buffer_acquires == 1 && g_buffer_releases == 0);
  assert(g_fence_dups == 0 && g_fence_closes == 1);
  ReleaseContents(&transaction);
  assert(g_buffer_releases == 1 && g_fence_closes == 2);
}

void TestReferencePreparationRejectsInvalidOrPartialTargets() {
  ResetCounts();
  SurfaceTransaction transaction;
  auto* parent = Fake<ASurfaceControl>(0x125);
  auto* control = Fake<ASurfaceControl>(0x225);
  assert(!SurfaceTransactionBuilder(&transaction).SetReparent(nullptr, parent));
  assert(!SurfaceTransactionBuilder(&transaction).SetRelativeLayer(
      nullptr, parent, 1));
  assert(transaction.controls.empty() && transaction.updates.empty());

  // Let update-vector reservation succeed, then fail controls-vector
  // reservation. Neither control may be retained by a rejected operation.
  g_allocations_before_failure = 1;
  assert(!SurfaceTransactionBuilder(&transaction).SetReparent(control, parent));
  assert(transaction.controls.empty() && transaction.updates.empty());
  assert(g_control_acquires == 0 && g_control_releases == 0);
}

void TestCallbackDeleteReuseDoesNotLoseNewState() {
  ResetCounts();
  auto* transaction = new SurfaceTransaction();
  auto* control = Fake<ASurfaceControl>(0x120);
  auto* replacement_control = Fake<ASurfaceControl>(0x220);
  auto* old_buffer = Fake<AHardwareBuffer>(0x320);
  auto* new_buffer = Fake<AHardwareBuffer>(0x420);
  auto* callback_buffer = Fake<AHardwareBuffer>(0x520);
  SurfaceTransactionBuilder builder(transaction);
  assert(builder.SetBuffer(control, old_buffer, 40));
  ReentrantDiscard context{transaction, replacement_control, callback_buffer,
                           50};
  transaction->buffer_callbacks.push_back(
      {.control = control, .context = &context, .discard = &ConsumeAndDeleteReuse});

  assert(builder.SetBuffer(control, new_buffer, 41));
  // The callback consumed the duplicate, and the delete/reuse path released
  // the incoming replacement exactly once before installing its own state.
  assert(context.calls == 1 && context.received_fence == 1040);
  assert(g_fence_dups == 1);
  assert(g_fence_closes == 3);  // callback duplicate, replacement fence, old fence
  assert(g_buffer_acquires == 3);  // old, outer replacement, nested replacement
  assert(g_buffer_releases == 2);  // delete/reuse released outer + detached old
  assert(transaction->updates.size() == 1);
  assert(transaction->updates[0].opaque == replacement_control);
  assert(transaction->updates[0].buffer == callback_buffer);
  assert(transaction->updates[0].acquire_fence == 50);
  assert(transaction->controls.size() == 1);
  assert(transaction->controls[0] == replacement_control);

  ReleaseContents(transaction);
  assert(g_buffer_releases == 3);
  assert(g_fence_closes == 4);  // nested final fence
  assert(g_control_releases == 2);  // original delete/reuse plus final cleanup
  delete transaction;
}

}  // namespace

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer*) {
  ++g_buffer_acquires;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer*) {
  ++g_buffer_releases;
}
extern "C" void ASurfaceControl_acquire(ASurfaceControl*) {
  ++g_control_acquires;
}
extern "C" void ASurfaceControl_release(ASurfaceControl*) {
  ++g_control_releases;
}
extern "C" int darwin_art_bionic_socket_broker_dup(int fd) {
  ++g_fence_dups;
  return fd + 1000;
}
extern "C" int darwin_art_bionic_socket_broker_close(int) {
  ++g_fence_closes;
  return 0;
}

int main() {
  TestRepeatedReplacementAndRetention();
  TestFailedAdmissionClosesIncomingFence();
  TestAllocationFailureLeavesDisplacedStateUntouched();
  TestReferencePreparationRejectsInvalidOrPartialTargets();
  TestCallbackDeleteReuseDoesNotLoseNewState();
  return 0;
}
