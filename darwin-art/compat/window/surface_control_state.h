#pragma once

#include "../surfaceflinger/metal_composer.h"
#include "../surfaceflinger/transaction_bridge.h"

#include <android/hardware_buffer.h>
#include <android/surface_control.h>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace darwin_art::window {

// A state view is assembled while the producer's SurfaceControl registry is
// locked.  It contains identities and values only; no parent/relative object
// pointer is followed by the snapshot owner after the lock is released.
struct SurfaceControlStateView final {
  ASurfaceControl* opaque = nullptr;
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  uint32_t parent_owner_process_id = 0;
  uint32_t parent_id = 0;
  uint32_t relative_parent_owner_process_id = 0;
  uint32_t relative_parent_id = 0;
  AHardwareBuffer* buffer = nullptr;  // borrowed until BuildSnapshot returns
  std::string name;
  bool composition_root = false;
  bool attached_to_root = false;
  bool has_geometry = false;
  ARect source{};
  ARect destination{};
  ARect crop{};
  bool has_crop = false;
  bool visible = true;
  int32_t position_x = 0;
  int32_t position_y = 0;
  int32_t transform = 0;
  int32_t z_order = 0;
  float scale_x = 1.0f;
  float scale_y = 1.0f;
  float alpha = 1.0f;
  std::vector<ARect> transparent_region;
};

struct SurfaceControlUpdateView final {
  ASurfaceControl* opaque = nullptr;
  AHardwareBuffer* buffer = nullptr;
  bool has_buffer = false;
  bool has_position = false;
  int32_t position_x = 0;
  int32_t position_y = 0;
  bool has_z_order = false;
  int32_t z_order = 0;
  bool has_alpha = false;
  float alpha = 1.0f;
  bool has_scale = false;
  float scale_x = 1.0f;
  float scale_y = 1.0f;
  bool has_visibility = false;
  bool visible = true;
  bool has_parent = false;
  uint32_t parent_owner_process_id = 0;
  uint32_t parent_id = 0;
  bool has_relative_layer = false;
  uint32_t relative_parent_owner_process_id = 0;
  uint32_t relative_parent_id = 0;
  bool has_transform = false;
  int32_t transform = 0;
  bool has_crop = false;
  ARect crop{};
  bool has_geometry = false;
  ARect source{};
  ARect destination{};
  bool has_damage = false;
  std::vector<ARect> damage;
  bool has_transparent_region = false;
  std::vector<ARect> transparent_region;
};

struct SurfaceControlPresentation final {
  ASurfaceControl* opaque = nullptr;  // opaque callback token only
  AHardwareBuffer* buffer = nullptr;  // one retained reference
  uint32_t owner_process_id = 0;
  uint32_t layer_id = 0;
  uint32_t parent_owner_process_id = 0;
  uint32_t parent_id = 0;
  uint32_t relative_parent_owner_process_id = 0;
  uint32_t relative_parent_id = 0;
  ARect source{};
  ARect destination{};
  // False when no destination frame was ever set: SurfaceFlinger then takes
  // the layer bounds from each new buffer, as AOSP does for an unset
  // layer_state_t::destinationFrame.
  bool explicit_geometry = false;
  float alpha = 1.0f;
  int32_t z_order = 0;
  std::string name;
  int32_t transform = 0;
  bool reparented = false;
  bool has_damage = false;
  ARect damage{};
};

// Owns the immutable values handed to either the local Metal compositor or
// the central SurfaceFlinger transport.  The AHardwareBuffer references are
// retained by BuildSurfaceControlSnapshot and released here, after submit.
struct SurfaceControlSnapshot final {
  std::vector<DarwinArtSurfaceFlingerLayerUpdate> frontend_updates;
  std::vector<DarwinArtMetalComposerLayer> control_states;
  std::vector<SurfaceControlPresentation> presentations;

  SurfaceControlSnapshot() = default;
  SurfaceControlSnapshot(const SurfaceControlSnapshot&) = delete;
  SurfaceControlSnapshot& operator=(const SurfaceControlSnapshot&) = delete;
  SurfaceControlSnapshot(SurfaceControlSnapshot&& other) noexcept;
  SurfaceControlSnapshot& operator=(SurfaceControlSnapshot&& other) noexcept;
  ~SurfaceControlSnapshot();
};

// Move-only ownership token for a registry-prepared transaction. The plan is
// opaque here so callers can only inspect the immutable projected snapshot;
// registry finalization owns all state mutation and lifetime transfer.
class PreparedSurfaceCommit final {
 public:
  PreparedSurfaceCommit() noexcept;
  ~PreparedSurfaceCommit();
  PreparedSurfaceCommit(const PreparedSurfaceCommit&) = delete;
  PreparedSurfaceCommit& operator=(const PreparedSurfaceCommit&) = delete;
  PreparedSurfaceCommit(PreparedSurfaceCommit&& other) noexcept;
  PreparedSurfaceCommit& operator=(PreparedSurfaceCommit&& other) noexcept;

  explicit operator bool() const noexcept { return impl_ != nullptr; }
  const SurfaceControlSnapshot* Snapshot() const noexcept;
  void Cancel() noexcept;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
  friend class SurfaceControlRegistry;
};

// Applies one transaction update to a borrowed candidate state view. Pointer
// relationship ownership and attachment reachability stay with the registry;
// this function only projects Android's scalar/payload values.
bool ApplySurfaceControlUpdateOverlay(
    SurfaceControlStateView* control,
    const SurfaceControlUpdateView& update);

// Builds only the pointer-free AOSP frontend payload. This pass deliberately
// retains no AHardwareBuffer and is safe before the producer registry is
// mutated, so a rejected frontend transaction leaves provider state and
// transaction-buffer ownership unchanged.
bool BuildSurfaceControlFrontendUpdates(
    const std::vector<SurfaceControlStateView>& controls,
    const std::vector<SurfaceControlUpdateView>& updates,
    std::vector<DarwinArtSurfaceFlingerLayerUpdate>* out);

// Converts already-resolved producer state into an immutable submission
// snapshot.  This module does not mutate controls, walk hierarchy pointers,
// or choose a hierarchy policy; the AOSP transaction bridge remains the only
// hierarchy authority.
bool BuildSurfaceControlSnapshot(
    const std::vector<SurfaceControlStateView>& controls,
    const std::vector<SurfaceControlUpdateView>& updates,
    uint32_t local_process_id,
    bool central_surfaceflinger,
    SurfaceControlSnapshot* out);

}  // namespace darwin_art::window
