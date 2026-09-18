#include "compat/window/surface_control_state.h"

#include <cassert>
#include <cstdint>
#include <cstdio>

struct AHardwareBuffer {};

namespace {
int g_acquires = 0;
int g_releases = 0;
}

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  ++g_acquires;
}

extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  ++g_releases;
}

extern "C" void AHardwareBuffer_describe(
    const AHardwareBuffer* buffer, AHardwareBuffer_Desc* description) {
  assert(buffer != nullptr && description != nullptr);
  *description = {};
  description->width = 64;
  description->height = 32;
  description->layers = 1;
  description->usage = AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY;
}

int main() {
  auto* root = reinterpret_cast<ASurfaceControl*>(static_cast<uintptr_t>(0x100));
  auto* child = reinterpret_cast<ASurfaceControl*>(static_cast<uintptr_t>(0x200));
  auto* buffer = reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(0x300));
  std::vector<darwin_art::window::SurfaceControlStateView> controls;
  controls.push_back({
      .opaque = root,
      .owner_process_id = 77,
      .layer_id = 1,
      .composition_root = true,
      .attached_to_root = true,
      .visible = true,
  });
  controls.push_back({
      .opaque = child,
      .owner_process_id = 77,
      .layer_id = 2,
      .parent_owner_process_id = 77,
      .parent_id = 1,
      .buffer = buffer,
      .name = "child",
      .attached_to_root = true,
      .visible = true,
      .position_x = 4,
      .position_y = 5,
      .scale_x = 2.0f,
      .scale_y = 3.0f,
  });
  std::vector<darwin_art::window::SurfaceControlUpdateView> updates;
  updates.push_back({
      .opaque = child,
      .buffer = buffer,
      .has_buffer = true,
      .has_parent = true,
      .parent_owner_process_id = 77,
      .parent_id = 1,
      .has_damage = true,
      .damage = {ARect{1, 2, 3, 4}, ARect{0, 1, 8, 6}},
  });

  // The rejection preflight consumes only copied values. In particular, the
  // frontend buffer-present bit survives the later transaction ownership
  // transfer, while this pass performs no AHardwareBuffer retain/release.
  std::vector<DarwinArtSurfaceFlingerLayerUpdate> frontend;
  assert(darwin_art::window::BuildSurfaceControlFrontendUpdates(
      controls, updates, &frontend));
  assert(frontend.size() == 1);
  assert((frontend[0].what & DARWIN_ART_SF_BUFFER_CHANGED) != 0);
  assert(updates[0].buffer == buffer);
  assert(controls[1].buffer == buffer);
  assert(g_acquires == 0);
  assert(g_releases == 0);

  // Model a rejected callback: no provider mutation or transfer is allowed
  // after the no-retain preflight has produced its payload.
  assert(frontend[0].what & DARWIN_ART_SF_BUFFER_CHANGED);
  assert(updates[0].buffer == buffer);
  assert(controls[1].buffer == buffer);
  assert(g_acquires == 0);
  assert(g_releases == 0);

  {
    darwin_art::window::SurfaceControlSnapshot snapshot;
    assert(darwin_art::window::BuildSurfaceControlSnapshot(
        controls, updates, 77, true, &snapshot));
    assert(snapshot.presentations.size() == 1);
    assert(snapshot.presentations[0].opaque == child);
    assert(snapshot.presentations[0].buffer == buffer);
    assert(snapshot.presentations[0].parent_id == 1);
    assert(snapshot.presentations[0].has_damage);
    assert(snapshot.presentations[0].damage.left == 0);
    assert(snapshot.presentations[0].damage.right == 8);
    assert(g_acquires == 1);

    // The producer may reparent/detach and replace its state immediately
    // after snapshot publication. The retained immutable view stays stable.
    controls[1].parent_id = 0;
    controls[1].attached_to_root = false;
    controls[1].visible = false;
    controls[1].buffer = nullptr;
    assert(snapshot.presentations[0].parent_id == 1);
    assert(snapshot.presentations[0].buffer == buffer);
  }
  assert(g_releases == 1);

  darwin_art::window::SurfaceControlSnapshot detached;
  assert(darwin_art::window::BuildSurfaceControlSnapshot(
      controls, updates, 77, false, &detached));
  assert(detached.presentations.empty());
  assert(detached.control_states.size() >= 1);
  auto* replacement = reinterpret_cast<AHardwareBuffer*>(
      static_cast<uintptr_t>(0x400));
  controls[1].parent_id = 1;
  controls[1].attached_to_root = true;
  controls[1].visible = true;
  controls[1].buffer = replacement;
  {
    darwin_art::window::SurfaceControlSnapshot replaced;
    assert(darwin_art::window::BuildSurfaceControlSnapshot(
        controls, updates, 77, true, &replaced));
    assert(replaced.presentations.size() == 1);
    assert(replaced.presentations[0].buffer == replacement);
    assert(g_acquires == 2);
  }
  assert(g_releases == 2);
  std::puts("surface-control state: retained AHB snapshot survives detach/reparent and releases once PASS");
}
