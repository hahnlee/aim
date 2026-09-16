#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Pointer-free transaction state shared by Darwin's NDK SurfaceControl
// producer and the exact AOSP SurfaceFlinger frontend.  `what` uses
// android::layer_state_t change bits; the remaining fields are the matching
// layer_state_t payload.
typedef struct DarwinArtSurfaceFlingerLayerUpdate {
  uint32_t layer_id;
  uint32_t parent_id;
  uint32_t relative_parent_id;
  uint64_t what;
  uint32_t flags;
  uint32_t mask;
  uint32_t transform;
  float x;
  float y;
  float scale_x;
  float scale_y;
  int32_t z;
  float alpha;
  int32_t destination_left;
  int32_t destination_top;
  int32_t destination_right;
  int32_t destination_bottom;
  int32_t crop_left;
  int32_t crop_top;
  int32_t crop_right;
  int32_t crop_bottom;
} DarwinArtSurfaceFlingerLayerUpdate;

typedef enum DarwinArtSurfaceFlingerLayerChange {
  DARWIN_ART_SF_POSITION_CHANGED = UINT64_C(0x00000001),
  DARWIN_ART_SF_LAYER_CHANGED = UINT64_C(0x00000002),
  DARWIN_ART_SF_ALPHA_CHANGED = UINT64_C(0x00000008),
  DARWIN_ART_SF_MATRIX_CHANGED = UINT64_C(0x00000010),
  DARWIN_ART_SF_FLAGS_CHANGED = UINT64_C(0x00000040),
  DARWIN_ART_SF_RELATIVE_LAYER_CHANGED = UINT64_C(0x00004000),
  DARWIN_ART_SF_REPARENT = UINT64_C(0x00008000),
  DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED = UINT64_C(0x00040000),
  DARWIN_ART_SF_CROP_CHANGED = UINT64_C(0x00100000),
  DARWIN_ART_SF_BUFFER_CHANGED = UINT64_C(0x00200000),
  DARWIN_ART_SF_DATASPACE_CHANGED = UINT64_C(0x00800000),
  DARWIN_ART_SF_DAMAGE_CHANGED = UINT64_C(0x02000000),
  DARWIN_ART_SF_DESTINATION_FRAME_CHANGED = UINT64_C(0x100000000),
  // The layer's transparent-region hint changed.  The AOSP frontend does not
  // need to interpret this payload; Darwin's retained GPU composer consumes
  // the bounded rectangles after it has applied the frontend's layer order.
  DARWIN_ART_SF_TRANSPARENT_REGION_CHANGED = UINT64_C(0x200000000),
} DarwinArtSurfaceFlingerLayerChange;

typedef struct DarwinArtSurfaceFlingerCommitResult {
  uint64_t transaction_id;
  size_t transaction_count;
  size_t layer_state_count;
} DarwinArtSurfaceFlingerCommitResult;

// Queue, collect and flush one transaction through AOSP TransactionHandler.
// Metal composition must only consume the transaction after this returns
// true. This is the first production ownership boundary; later frontend
// stages consume the same ResolvedComposerState vector without changing this
// producer ABI.
bool darwin_art_surfaceflinger_commit_transaction(
    uint64_t transaction_id,
    const DarwinArtSurfaceFlingerLayerUpdate* updates,
    size_t update_count,
    DarwinArtSurfaceFlingerCommitResult* out_result);

// Copies the bottom-to-top layer order maintained by AOSP's
// LayerLifecycleManager and LayerHierarchyBuilder. The order is global to the
// central SurfaceFlinger process and is valid after a successful commit.
// Passing a null output with zero capacity queries the required size.
bool darwin_art_surfaceflinger_copy_layer_order(uint32_t* out_layer_ids,
                                                size_t capacity,
                                                size_t* out_count);

// Delivers SurfaceControl handle destruction to AOSP's lifecycle manager.
// Root destruction follows Android ownership rules and recursively destroys
// child layers whose handles are no longer alive.
bool darwin_art_surfaceflinger_destroy_layer_handles(
    const uint32_t* layer_ids, size_t layer_count);

#ifdef __cplusplus
}
#endif
