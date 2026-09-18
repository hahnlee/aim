#include "../../compat/window/surface_transaction_merge.h"

#include <cassert>
#include <cstdint>
#include <cstdio>

using darwin_art::window::MergeSurfaceTransactions;
using darwin_art::window::SurfaceTransaction;

namespace {

int callback_calls = 0;

void CommitCallback(void*, ASurfaceTransactionStats*) { ++callback_calls; }
void DiscardCallback(void*) { ++callback_calls; }
void BufferCompleteCallback(void*, ASurfaceTransactionStats*) {
  ++callback_calls;
}
void BufferDiscardCallback(void*, int) { ++callback_calls; }

template <typename T>
T* Fake(uintptr_t value) {
  return reinterpret_cast<T*>(value);
}

SurfaceTransaction::Update& UpdateFor(SurfaceTransaction& transaction,
                                      ASurfaceControl* control) {
  transaction.updates.push_back({.opaque = control});
  return transaction.updates.back();
}

}  // namespace

int main() {
  ASurfaceControl* first_control = Fake<ASurfaceControl>(0x1001);
  ASurfaceControl* second_control = Fake<ASurfaceControl>(0x1002);
  ASurfaceControl* third_control = Fake<ASurfaceControl>(0x1003);
  AHardwareBuffer* old_buffer = Fake<AHardwareBuffer>(0x2001);
  AHardwareBuffer* new_buffer = Fake<AHardwareBuffer>(0x2002);
  AHardwareBuffer* third_buffer = Fake<AHardwareBuffer>(0x2003);

  SurfaceTransaction destination;
  SurfaceTransaction source;
  SurfaceTransaction disposal;

  destination.controls = {first_control, second_control};
  source.controls = {first_control, third_control};

  auto& old = UpdateFor(destination, first_control);
  old.buffer = old_buffer;
  old.acquire_fence = 41;
  old.submission_cookie = 101;
  old.has_buffer = true;
  old.has_alpha = true;
  old.alpha = 0.25f;
  destination.buffer_callbacks.push_back(
      {first_control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});

  auto& incoming = UpdateFor(source, first_control);
  incoming.buffer = new_buffer;
  incoming.acquire_fence = 42;
  incoming.submission_cookie = 102;
  incoming.has_buffer = true;
  incoming.has_visibility = true;
  incoming.visible = false;
  incoming.has_position = true;
  incoming.position_x = 7;
  incoming.position_y = 8;
  incoming.has_damage = true;
  incoming.damage.push_back({1, 2, 3, 4});

  auto& third = UpdateFor(source, third_control);
  third.buffer = third_buffer;
  third.acquire_fence = 43;
  third.submission_cookie = 103;
  third.has_buffer = true;

  source.commits.push_back({&CommitCallback, nullptr});
  source.completes.push_back({&CommitCallback, nullptr});
  source.discards.push_back({&DiscardCallback, nullptr});
  source.buffer_callbacks.push_back(
      {first_control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});
  source.buffer_callbacks.push_back(
      {third_control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});

  assert(MergeSurfaceTransactions(&destination, &source, &disposal));
  assert(callback_calls == 0);

  // Source ownership is completely transferred/emptied, including callbacks.
  assert(source.controls.empty());
  assert(source.updates.empty());
  assert(source.commits.empty());
  assert(source.completes.empty());
  assert(source.discards.empty());
  assert(source.buffer_callbacks.empty());

  // The duplicate source control reference is retained by disposal, while a
  // new source control is transferred into destination exactly once.
  assert(destination.controls.size() == 3);
  assert(destination.controls[0] == first_control);
  assert(destination.controls[1] == second_control);
  assert(destination.controls[2] == third_control);
  assert(disposal.controls.size() == 1);
  assert(disposal.controls[0] == first_control);

  // New buffer/fence and every source field win in destination.
  assert(destination.updates.size() == 2);
  const auto& merged = destination.updates[0];
  assert(merged.opaque == first_control);
  assert(merged.buffer == new_buffer && merged.acquire_fence == 42);
  assert(merged.submission_cookie == 102);
  assert(merged.has_buffer && merged.has_visibility && !merged.visible);
  assert(merged.has_position && merged.position_x == 7 && merged.position_y == 8);
  assert(merged.has_damage && merged.damage.size() == 1);
  assert(merged.has_alpha && merged.alpha == 0.25f);
  assert(destination.updates[1].opaque == third_control);
  assert(destination.updates[1].buffer == third_buffer);
  assert(destination.updates[1].acquire_fence == 43);
  assert(destination.updates[1].submission_cookie == 103);
  assert(destination.commits.size() == 1 && destination.completes.size() == 1);
  assert(destination.discards.size() == 1);
  assert(destination.buffer_callbacks.size() == 2);

  // The displaced destination owner is fully represented in disposal. Its
  // old fence remains associated with the old buffer callback for deletion's
  // existing DiscardBufferCallbacks fence-forwarding path.
  assert(disposal.updates.size() == 1);
  assert(disposal.updates[0].opaque == first_control);
  assert(disposal.updates[0].buffer == old_buffer);
  assert(disposal.updates[0].acquire_fence == 41);
  assert(disposal.updates[0].submission_cookie == 101);
  assert(disposal.updates[0].has_buffer);
  assert(disposal.buffer_callbacks.size() == 1);
  assert(disposal.buffer_callbacks[0].control == first_control);
  assert(disposal.buffer_callbacks[0].discard == &BufferDiscardCallback);

  // Self/null are successful no-ops, and a non-empty disposal is rejected
  // before any source or destination mutation.
  SurfaceTransaction unchanged_destination = destination;
  SurfaceTransaction unchanged_source;
  unchanged_source.controls.push_back(second_control);
  SurfaceTransaction nonempty_disposal;
  nonempty_disposal.controls.push_back(third_control);
  assert(MergeSurfaceTransactions(&unchanged_destination, &unchanged_source,
                                  &nonempty_disposal) == false);
  assert(unchanged_source.controls.size() == 1);
  assert(unchanged_destination.controls.size() == destination.controls.size());
  assert(nonempty_disposal.controls.size() == 1);
  assert(MergeSurfaceTransactions(nullptr, &unchanged_source, &disposal));
  assert(MergeSurfaceTransactions(&unchanged_destination,
                                  &unchanged_destination, &disposal));

  std::puts("SurfaceTransaction structural merge: ownership transfer, deferred "
            "disposal/fence association, callback safety, and no-op guards PASS");
  return 0;
}
