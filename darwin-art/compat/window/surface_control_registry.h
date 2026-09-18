#pragma once

#include "surface_control_state.h"

#include <android/surface_control.h>

#include <cstddef>
#include <cstdint>
#include <vector>

namespace darwin_art::window {

struct SurfaceTransaction;
struct SurfaceTransactionStats;

// Owns the process-local SurfaceControl handle registry. The implementation
// keeps the object graph and synchronization private; callers receive only
// opaque handles or copied state views.
class SurfaceControlRegistry final {
 public:
  static SurfaceControlRegistry& Instance();

  ASurfaceControl* Create(ASurfaceControl* parent, const char* name,
                          bool composition_root,
                          uint32_t imported_owner_process_id = 0,
                          uint32_t imported_layer_id = 0,
                          uint32_t imported_parent_owner_process_id = 0,
                          uint32_t imported_parent_layer_id = 0);
  void Acquire(ASurfaceControl* control);
  void Release(ASurfaceControl* control);
  bool GetIdentity(const ASurfaceControl* control, uint32_t* owner_process_id,
                   uint32_t* layer_id) const;
  size_t CopyTransparentRegion(const ASurfaceControl* control,
                               int32_t* rects, size_t capacity) const;

  // Copies all relationships and transaction values while the registry lock is
  // held. Neither output contains a pointer that callers need to dereference.
  bool CopyViews(
      const SurfaceTransaction* transaction,
      std::vector<SurfaceControlStateView>* control_views,
      std::vector<SurfaceControlUpdateView>* update_views) const;

  // Captures and retains all buffer leases while the registry lock is held.
  // The returned snapshot is safe for submit after this call returns; callers
  // must not rebuild it from the borrowed views above after unlocking.
  bool CaptureSnapshot(const SurfaceTransaction* transaction,
                       uint32_t local_process_id,
                       bool central_surfaceflinger,
                       SurfaceControlSnapshot* snapshot) const;

  // Applies a transaction only after the common frontend has accepted it.
  // Buffer ownership transfer and relationship lease retirement are performed
  // here; the caller then obtains a fresh immutable view for Darwin submit.
  bool ApplyAcceptedTransaction(SurfaceTransaction* transaction,
                                SurfaceTransactionStats* stats);

  // Prepares a complete candidate projection and all resource/lifetime tails
  // without mutating registry state. `stats` must be a fresh output object;
  // its previous-buffer maps are populated atomically during finalization.
  bool PrepareSurfaceCommit(SurfaceTransaction* transaction,
                            uint32_t local_process_id,
                            bool central_surfaceflinger,
                            SurfaceTransactionStats* stats,
                            PreparedSurfaceCommit* prepared) const;

  // Commits only scalar swaps, pointer transfers and preallocated map swaps
  // under the registry lock. Every release tail runs after the lock. This is
  // noexcept and performs no allocation or callback dispatch.
  bool FinalizeSurfaceCommit(PreparedSurfaceCommit* prepared) noexcept;
  void CancelSurfaceCommit(PreparedSurfaceCommit* prepared) const noexcept;

 private:
  SurfaceControlRegistry() = default;
};

}  // namespace darwin_art::window
