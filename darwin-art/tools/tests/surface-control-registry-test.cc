#include "compat/window/surface_control_registry.h"
#include "compat/window/surface_transaction_lifetime.h"
#include "compat/window/surface_transaction_state.h"

#include <android/hardware_buffer.h>

#include <atomic>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <thread>
#include <vector>

struct AHardwareBuffer {};

namespace {
std::atomic<int> g_acquires{0};
std::atomic<int> g_releases{0};
}

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  g_acquires.fetch_add(1, std::memory_order_relaxed);
}

extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  g_releases.fetch_add(1, std::memory_order_relaxed);
}

extern "C" void AHardwareBuffer_describe(
    const AHardwareBuffer* buffer, AHardwareBuffer_Desc* description) {
  assert(buffer != nullptr && description != nullptr);
  *description = {};
  description->width = 32;
  description->height = 16;
  description->layers = 1;
  description->usage = AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY;
}

namespace darwin_art::window {
SurfaceTransactionStats::~SurfaceTransactionStats() {}
}

int main() {
  auto& registry = darwin_art::window::SurfaceControlRegistry::Instance();
  // Imported ancestry is part of creation, never a separately published
  // mutation that can race a prepared transaction snapshot.
  assert(registry.Create(nullptr, "invalid-import", false, 0, 0, 77, 0) == nullptr);
  auto* imported = registry.Create(nullptr, "imported-child", false,
                                   0, 0, 77, 88);
  assert(imported != nullptr);
  std::vector<darwin_art::window::SurfaceControlStateView> imported_views;
  std::vector<darwin_art::window::SurfaceControlUpdateView> imported_updates;
  darwin_art::window::SurfaceTransaction imported_transaction;
  assert(registry.CopyViews(&imported_transaction, &imported_views,
                            &imported_updates));
  bool found_imported = false;
  for (const auto& view : imported_views) {
    if (view.opaque != imported) continue;
    assert(view.parent_owner_process_id == 77 && view.parent_id == 88);
    found_imported = true;
  }
  assert(found_imported);
  registry.Release(imported);
  auto* root = registry.Create(nullptr, "root", true);
  auto* sibling = registry.Create(nullptr, "sibling", true);
  auto* child = registry.Create(root, "child", false);
  assert(root != nullptr && sibling != nullptr && child != nullptr);

  auto* buffer = reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(0x101));
  darwin_art::window::SurfaceTransaction transaction;
  registry.Acquire(child);
  transaction.controls.push_back(child);
  transaction.updates.push_back({
      .opaque = child,
      .buffer = buffer,
      .has_buffer = true,
      .z_order = 7,
      .has_parent = true,
      .parent = root,
      .has_relative_layer = true,
      .relative_to = sibling,
  });

  std::vector<darwin_art::window::SurfaceControlStateView> pre_controls;
  std::vector<darwin_art::window::SurfaceControlUpdateView> pre_updates;
  std::vector<DarwinArtSurfaceFlingerLayerUpdate> frontend;
  assert(registry.CopyViews(&transaction, &pre_controls, &pre_updates));
  assert(darwin_art::window::BuildSurfaceControlFrontendUpdates(
      pre_controls, pre_updates, &frontend));
  assert(frontend.size() == 1);
  assert((frontend[0].what & DARWIN_ART_SF_BUFFER_CHANGED) != 0);
  // A rejected frontend callback would stop here: no state or transaction
  // ownership has changed and no AHB retain/release was performed.
  assert(transaction.updates[0].buffer == buffer);
  assert(g_acquires.load() == 0);
  assert(g_releases.load() == 0);

  darwin_art::window::SurfaceTransactionStats stats;
  assert(registry.ApplyAcceptedTransaction(&transaction, &stats));
  assert(transaction.updates[0].buffer == nullptr);
  assert(g_acquires.load() == 1);  // canonical occurrence's independent lease
  assert(g_releases.load() == 0);

  {
    darwin_art::window::SurfaceControlSnapshot snapshot;
    assert(registry.CaptureSnapshot(&transaction, 0, true, &snapshot));
    assert(snapshot.presentations.size() == 1);
    assert(snapshot.presentations[0].buffer == buffer);
    assert(snapshot.presentations[0].parent_id != 0);
    assert(snapshot.presentations[0].relative_parent_id != 0);
    assert(g_acquires.load() == 2);

    // The captured AHB lease survives concurrent handle release and a
    // detached relationship; no post-unlock registry pointer is needed by
    // the snapshot.
    registry.Acquire(child);
    registry.Acquire(child);
    std::vector<std::thread> releasers;
    releasers.emplace_back([&] { registry.Release(child); });
    releasers.emplace_back([&] { registry.Release(child); });
    for (auto& thread : releasers) thread.join();
    assert(snapshot.presentations[0].buffer == buffer);
    assert(snapshot.presentations[0].relative_parent_id != 0);
  }
  assert(g_releases.load() == 1);

  registry.Release(child);  // transaction's remembered control lease
  registry.Release(child);  // caller's original handle
  registry.Release(sibling);
  registry.Release(root);   // child-parent lease is retired with child
  assert(g_releases.load() == 3);  // snapshot, wrapper, canonical occurrence
  std::puts("surface-control registry: accepted transfer, relationship refs, capture lifetime, concurrent release PASS");
}
