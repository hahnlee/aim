#include "compat/window/surface_control_registry.h"
#include "compat/window/surface_transaction_lifetime.h"

#include <android/hardware_buffer.h>

#include <algorithm>
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <new>

namespace {

bool forbid_allocations = false;
int fail_after = -1;

}  // namespace

void* operator new(std::size_t size) {
  if (forbid_allocations || fail_after == 0) {
    fail_after = -1;
    throw std::bad_alloc();
  }
  if (fail_after > 0) --fail_after;
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { std::free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { std::free(pointer); }

struct AHardwareBuffer {
  int references = 1;
};

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  ++buffer->references;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer != nullptr && buffer->references > 0);
  --buffer->references;
}
extern "C" void AHardwareBuffer_describe(
    const AHardwareBuffer* buffer, AHardwareBuffer_Desc* description) {
  assert(buffer != nullptr && description != nullptr);
  *description = {};
  description->width = 32;
  description->height = 16;
  description->layers = 1;
}

namespace darwin_art::window {
SurfaceTransactionStats::~SurfaceTransactionStats() {
  for (const auto& entry : previous_buffers) AHardwareBuffer_release(entry.second);
}
}  // namespace darwin_art::window

namespace {

using darwin_art::window::PreparedSurfaceCommit;
using darwin_art::window::SurfaceControlRegistry;
using darwin_art::window::SurfaceControlStateView;
using darwin_art::window::SurfaceControlUpdateView;
using darwin_art::window::SurfaceTransaction;
using darwin_art::window::SurfaceTransactionStats;

SurfaceTransaction BufferTransaction(ASurfaceControl* first,
                                     ASurfaceControl* alias,
                                     AHardwareBuffer* buffer,
                                     uint64_t first_cookie,
                                     uint64_t alias_cookie) {
  SurfaceTransaction transaction;
  transaction.controls = {first, alias};
  transaction.updates.push_back({
      .opaque = first,
      .buffer = buffer,
      .submission_cookie = first_cookie,
      .has_buffer = true,
      .has_visibility = true,
      .visible = true,
  });
  transaction.updates.push_back({
      .opaque = alias,
      .buffer = buffer,
      .submission_cookie = alias_cookie,
      .has_buffer = true,
      .has_visibility = true,
      .visible = false,
  });
  // Mirror SurfaceTransactionBuilder::SetBuffer: every update owns the
  // transaction-side reference until Finalize transfers it to the wrapper.
  AHardwareBuffer_acquire(buffer);
  AHardwareBuffer_acquire(buffer);
  return transaction;
}

void ReleaseTestTransactionBuffers(SurfaceTransaction* transaction) {
  for (auto& update : transaction->updates) {
    if (update.buffer != nullptr) {
      AHardwareBuffer_release(update.buffer);
      update.buffer = nullptr;
    }
  }
}

void TestPrepareOverlayAliasesAndFinalize() {
  auto& registry = SurfaceControlRegistry::Instance();
  auto* root = registry.Create(nullptr, "prepared-root", true, 700, 1);
  auto* relative = registry.Create(nullptr, "prepared-relative", true, 700, 2);
  auto* first = registry.Create(nullptr, "prepared-first", false, 700, 3);
  auto* alias = registry.Create(nullptr, "prepared-alias", false, 700, 3);
  assert(root != nullptr && relative != nullptr && first != nullptr &&
         alias != nullptr);
  AHardwareBuffer first_buffer;

  auto transaction = BufferTransaction(first, alias, &first_buffer, 10, 11);
  transaction.updates[0].has_parent = true;
  transaction.updates[0].parent = root;
  transaction.updates[0].has_relative_layer = true;
  transaction.updates[0].relative_to = relative;
  transaction.updates[0].has_position = true;
  transaction.updates[0].position_x = 4;
  transaction.updates[0].position_y = 5;
  transaction.updates[0].has_z_order = true;
  transaction.updates[0].z_order = 9;

  SurfaceTransactionStats stats;
  PreparedSurfaceCommit prepared;
  assert(registry.PrepareSurfaceCommit(&transaction, 0, true, &stats,
                                       &prepared));
  assert(prepared);
  assert(transaction.updates[0].buffer == &first_buffer);
  assert(transaction.updates[1].buffer == &first_buffer);
  const auto* projected = prepared.Snapshot();
  assert(projected != nullptr && projected->presentations.size() == 1);
  assert(projected->presentations[0].parent_id == 1);
  assert(projected->presentations[0].relative_parent_id == 2);

  forbid_allocations = true;
  bool finalized = false;
  try {
    finalized = registry.FinalizeSurfaceCommit(&prepared);
  } catch (...) {
    assert(false && "finalization allocated or threw");
  }
  forbid_allocations = false;
  assert(finalized);
  assert(transaction.updates[0].buffer == nullptr);
  assert(transaction.updates[1].buffer == nullptr);
  assert(stats.controls.size() == 2);
  assert(stats.previous_buffers.size() == 1);
  assert(stats.previous_buffers.at(alias) == &first_buffer);
  assert(stats.previous_submission_cookies.at(alias) == 10);
  assert(first_buffer.references > 1);
  prepared.Cancel();

  // Absolute z-order clears a previously relative layer in the projected
  // state and in the committed registry, rather than leaving a stale
  // relative pointer behind.
  SurfaceTransaction absolute;
  absolute.controls = {first};
  absolute.updates.push_back({.opaque = first,
                              .has_z_order = true,
                              .z_order = 14});
  SurfaceTransactionStats absolute_stats;
  PreparedSurfaceCommit absolute_plan;
  assert(registry.PrepareSurfaceCommit(&absolute, 0, true, &absolute_stats,
                                       &absolute_plan));
  assert(absolute_plan.Snapshot()->presentations.size() == 1);
  assert(absolute_plan.Snapshot()->presentations[0].relative_parent_id == 0);
  assert(registry.FinalizeSurfaceCommit(&absolute_plan));
  std::vector<SurfaceControlStateView> copied_controls;
  std::vector<SurfaceControlUpdateView> copied_updates;
  assert(registry.CopyViews(&absolute, &copied_controls, &copied_updates));
  auto first_view = std::find_if(
      copied_controls.begin(), copied_controls.end(),
      [first](const SurfaceControlStateView& view) {
        return view.opaque == first;
      });
  assert(first_view != copied_controls.end());
  assert(first_view->relative_parent_id == 0);
  absolute_plan.Cancel();

  auto replacement = new AHardwareBuffer();
  auto replacement_transaction =
      BufferTransaction(first, alias, replacement, 12, 13);
  SurfaceTransactionStats replacement_stats;
  PreparedSurfaceCommit replacement_plan;
  assert(registry.PrepareSurfaceCommit(&replacement_transaction, 0, true,
                                       &replacement_stats, &replacement_plan));
  // The canonical occurrence is simulated in update order: first sees the
  // old buffer/cookie, alias sees the first update's replacement.
  assert(replacement_stats.previous_buffers.empty());
  assert(replacement_transaction.updates[0].buffer == replacement);
  replacement_plan.Cancel();
  assert(replacement_transaction.updates[0].buffer == replacement);
  ReleaseTestTransactionBuffers(&replacement_transaction);
  delete replacement;

  registry.Release(alias);
  registry.Release(first);
  registry.Release(relative);
  registry.Release(root);
}

void TestCanonicalPredecessorAndSameBuffer() {
  auto& registry = SurfaceControlRegistry::Instance();
  auto* first = registry.Create(nullptr, "canonical-first", true, 701, 7);
  auto* alias = registry.Create(nullptr, "canonical-alias", true, 701, 7);
  assert(first != nullptr && alias != nullptr);
  auto* first_buffer = new AHardwareBuffer();
  auto* second_buffer = new AHardwareBuffer();
  {
    auto initial = BufferTransaction(first, alias, first_buffer, 20, 21);
    SurfaceTransactionStats initial_stats;
    PreparedSurfaceCommit initial_plan;
    assert(registry.PrepareSurfaceCommit(&initial, 0, true, &initial_stats,
                                         &initial_plan));
    assert(registry.FinalizeSurfaceCommit(&initial_plan));
    initial_plan.Cancel();

    auto replacement = BufferTransaction(first, alias, second_buffer, 22, 23);
    SurfaceTransactionStats replacement_stats;
    PreparedSurfaceCommit replacement_plan;
    assert(registry.PrepareSurfaceCommit(
        &replacement, 0, true, &replacement_stats, &replacement_plan));
    assert(registry.FinalizeSurfaceCommit(&replacement_plan));
    assert(replacement_stats.previous_buffers.at(first) == first_buffer);
    assert(replacement_stats.previous_submission_cookies.at(first) == 21);
    assert(replacement_stats.previous_buffers.at(alias) == second_buffer);
    assert(replacement_stats.previous_submission_cookies.at(alias) == 22);
    replacement_plan.Cancel();

    // Re-submit the same AHardwareBuffer as a new occurrence. Equality must
    // not suppress predecessor capture or cookie advancement.
    auto same = BufferTransaction(first, alias, second_buffer, 24, 25);
    SurfaceTransactionStats same_stats;
    PreparedSurfaceCommit same_plan;
    assert(registry.PrepareSurfaceCommit(&same, 0, true, &same_stats,
                                         &same_plan));
    assert(registry.FinalizeSurfaceCommit(&same_plan));
    assert(same_stats.previous_buffers.at(first) == second_buffer);
    assert(same_stats.previous_submission_cookies.at(first) == 23);
    assert(same_stats.previous_buffers.at(alias) == second_buffer);
    assert(same_stats.previous_submission_cookies.at(alias) == 24);
    same_plan.Cancel();
  }

  registry.Release(alias);
  registry.Release(first);
  delete second_buffer;
  delete first_buffer;
}

void TestPrepareAllocationFailureAndMissingOccurrence() {
  auto& registry = SurfaceControlRegistry::Instance();
  auto* control = registry.Create(nullptr, "prepare-failure", true, 702, 9);
  assert(control != nullptr);
  auto* buffer = new AHardwareBuffer();
  SurfaceTransaction transaction;
  transaction.controls.push_back(control);
  transaction.updates.push_back({.opaque = control, .buffer = buffer,
                                 .has_buffer = true});
  SurfaceTransactionStats stats;
  PreparedSurfaceCommit prepared;
  fail_after = 0;
  assert(!registry.PrepareSurfaceCommit(&transaction, 0, true, &stats,
                                        &prepared));
  assert(transaction.updates[0].buffer == buffer);
  assert(stats.previous_buffers.empty() && stats.controls.empty());
  ReleaseTestTransactionBuffers(&transaction);
  fail_after = -1;
  prepared.Cancel();
  registry.Release(control);
  delete buffer;

  // Exercise failures at each allocation point after the plan has copied raw
  // touched identities.  A failed Prepare must never release a reference it
  // did not acquire, and must leave both the transaction and stats untouched.
  for (int budget = 0; budget < 24; ++budget) {
    auto* retry_control =
        registry.Create(nullptr, "prepare-failure-retry", true, 702,
                        static_cast<uint32_t>(100 + budget));
    assert(retry_control != nullptr);
    auto* retry_buffer = new AHardwareBuffer();
    SurfaceTransaction retry;
    retry.controls.push_back(retry_control);
    retry.updates.push_back({.opaque = retry_control,
                             .buffer = retry_buffer,
                             .has_buffer = true});
    SurfaceTransactionStats retry_stats;
    PreparedSurfaceCommit retry_plan;
    fail_after = budget;
    const bool accepted = registry.PrepareSurfaceCommit(
        &retry, 0, true, &retry_stats, &retry_plan);
    fail_after = -1;
    assert(retry.updates[0].buffer == retry_buffer);
    assert(retry_stats.controls.empty() && retry_stats.previous_buffers.empty());
    if (accepted) retry_plan.Cancel();
    ReleaseTestTransactionBuffers(&retry);
    registry.Release(retry_control);
    delete retry_buffer;
  }
}

}  // namespace

int main() {
  TestPrepareOverlayAliasesAndFinalize();
  TestCanonicalPredecessorAndSameBuffer();
  TestPrepareAllocationFailureAndMissingOccurrence();
  std::puts("prepared surface commit: candidate projection, canonical predecessor simulation, cancel safety, and allocation-free finalize PASS");
}
