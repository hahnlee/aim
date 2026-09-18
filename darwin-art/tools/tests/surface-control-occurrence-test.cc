#include "compat/window/surface_control_registry.h"
#include "compat/window/surface_transaction_lifetime.h"

#include <android/hardware_buffer.h>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <new>

static int allocation_countdown = -1;
void* operator new(std::size_t size) {
  if (allocation_countdown == 0) {
    allocation_countdown = -1;
    throw std::bad_alloc();
  }
  if (allocation_countdown > 0) --allocation_countdown;
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t) noexcept { std::free(p); }

struct AHardwareBuffer { int references = 1; };
extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer && buffer->references > 0);
  ++buffer->references;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer && buffer->references > 1);
  --buffer->references;
}
extern "C" void AHardwareBuffer_describe(
    const AHardwareBuffer*, AHardwareBuffer_Desc* description) {
  *description = {};
  description->width = description->height = 16;
  description->layers = 1;
}
extern "C" int darwin_art_bionic_socket_broker_close(int) { return 0; }
extern "C" int darwin_art_bionic_socket_broker_dup(int fd) { return fd + 1000; }
extern "C" void darwin_art_android_mark_hardware_buffer_released(void*) {}
extern "C" void ASurfaceControl_release(ASurfaceControl* control) {
  darwin_art::window::SurfaceControlRegistry::Instance().Release(control);
}
extern "C" void ASurfaceControl_acquire(ASurfaceControl* control) {
  darwin_art::window::SurfaceControlRegistry::Instance().Acquire(control);
}
extern "C" bool ASurfaceTransactionStats_getPreviousBufferMetadata(
    ASurfaceTransactionStats*, ASurfaceControl*, AHardwareBuffer**, uint64_t*);

using namespace darwin_art::window;

// These are prepared transaction payloads. Test the actual registry latch and
// callback-stat lifetime, separately from builder publication/reentrancy tests.
static void Latch(SurfaceControlRegistry& registry, ASurfaceControl* control,
                  AHardwareBuffer* buffer, uint64_t cookie,
                  SurfaceTransactionStats* stats) {
  SurfaceTransaction transaction;
  AHardwareBuffer_acquire(buffer);
  transaction.updates.push_back({.opaque = control, .buffer = buffer,
      .submission_cookie = cookie, .has_buffer = true});
  assert(registry.ApplyAcceptedTransaction(&transaction, stats));
  assert(transaction.updates.front().buffer == nullptr);
}

int main() {
  auto& registry = SurfaceControlRegistry::Instance();
  // Every fallible creation step must leave registry and parent ownership
  // unchanged. These are actual registry allocations, not synthetic handles.
  auto* parent = registry.Create(nullptr, "parent", true, 111, 200);
  assert(parent);
  int failures = 0;
  SurfaceTransaction empty_transaction;
  for (int index = 0; index < 12; ++index) {
    allocation_countdown = index;
    ASurfaceControl* child = nullptr;
    bool escaped = false;
    try {
      child = registry.Create(parent,
          "allocation-failure-child-with-a-name-beyond-small-string-storage",
          false, 111, 201 + index);
    } catch (const std::bad_alloc&) {
      escaped = true;
    }
    allocation_countdown = -1;
    assert(!escaped);
    if (child) registry.Release(child);
    else ++failures;
    std::vector<SurfaceControlStateView> controls;
    std::vector<SurfaceControlUpdateView> updates;
    assert(registry.CopyViews(&empty_transaction, &controls, &updates));
    assert(controls.size() == 1);
    if (child) break;
  }
  assert(failures >= 3);
  registry.Release(parent);
  {
    std::vector<SurfaceControlStateView> controls;
    std::vector<SurfaceControlUpdateView> updates;
    assert(registry.CopyViews(&empty_transaction, &controls, &updates));
    assert(controls.empty());
  }
  AHardwareBuffer a, b;
  auto* first = registry.Create(nullptr, "occurrence", true, 111, 222);
  auto* alias = registry.Create(nullptr, "alias", true, 111, 222);
  assert(first && alias && first != alias);
  uint32_t first_owner = 0, first_layer = 0, alias_owner = 0, alias_layer = 0;
  assert(registry.GetIdentity(first, &first_owner, &first_layer));
  assert(registry.GetIdentity(alias, &alias_owner, &alias_layer));
  assert(first_owner == alias_owner && first_layer == alias_layer);
  {
    SurfaceTransactionStats initial;
    Latch(registry, first, &a, 101, &initial);
    assert(initial.previous_buffers.empty());
  }
  {
    SurfaceTransactionStats alias_replacement;
    Latch(registry, alias, &b, 102, &alias_replacement);
    assert(alias_replacement.previous_buffers.contains(alias));
    assert(alias_replacement.previous_submission_cookies.contains(alias));
    assert(alias_replacement.previous_buffers.at(alias) == &a);
    assert(alias_replacement.previous_submission_cookies.at(alias) == 101);
  }
  {
    SurfaceTransactionStats delayed;
    // One allocation, a different submission occurrence: equality must not
    // suppress predecessor capture. Hold stats across yet another latch.
    Latch(registry, first, &b, 103, &delayed);
    assert(delayed.previous_buffers.contains(first));
    assert(delayed.previous_submission_cookies.contains(first));
    assert(delayed.previous_buffers.at(first) == &b);
    assert(delayed.previous_submission_cookies.at(first) == 102);
    AHardwareBuffer* borrowed = nullptr;
    uint64_t cookie = 0;
    const int references = b.references;
    assert(ASurfaceTransactionStats_getPreviousBufferMetadata(
        reinterpret_cast<ASurfaceTransactionStats*>(&delayed), first,
        &borrowed, &cookie));
    assert(borrowed == &b && cookie == 102 && b.references == references);
    {
      SurfaceTransactionStats newer;
      Latch(registry, alias, &b, 104, &newer);
      assert(newer.previous_buffers.contains(alias));
      assert(newer.previous_submission_cookies.contains(alias));
      assert(newer.previous_buffers.at(alias) == &b);
      assert(newer.previous_submission_cookies.at(alias) == 103);
    }
    assert(delayed.previous_buffers.at(first) == &b);
    assert(delayed.previous_submission_cookies.at(first) == 102);
    // Untracked occurrences must never accidentally retire a tracked token.
    delayed.previous_submission_cookies.at(first) = 0;
    assert(!ASurfaceTransactionStats_getPreviousBufferMetadata(
        reinterpret_cast<ASurfaceTransactionStats*>(&delayed), first,
        &borrowed, &cookie));
    assert(borrowed == nullptr && cookie == 0 && b.references == references);
  }
  registry.Release(first);
  registry.Release(alias);
  assert(a.references == 1 && b.references == 1);
  std::puts("surface-control occurrence: canonical aliases/same-AHB/delayed stats/ref balance PASS");
}
