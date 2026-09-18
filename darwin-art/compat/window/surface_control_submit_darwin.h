#pragma once
#include "surface_control_state.h"
#include "../surfaceflinger/commit_receipt.h"
#include <cstdint>
namespace darwin_art::window {
// Runtime configuration facts; the common transaction owner decides whether
// this configuration satisfies Android application admission requirements.
struct SurfaceControlSubmissionEnvironment {
  bool central_service = false;
  bool application = false;
};
SurfaceControlSubmissionEnvironment QuerySurfaceControlSubmissionEnvironment();
// Returned FD is transferred to the transaction statistics owner. This port
// consumes immutable retained values only; it never mutates controls or invokes
// Android callbacks. Failure classification belongs to the transaction owner.
struct SurfaceControlSubmitResult {
  bool success = false;
  int present_fence = -1;
  DarwinArtSurfaceFlingerReceipt receipt{DARWIN_ART_SF_COMMIT_UNKNOWN, 0, -1};
  bool context_restored = true;
  // Explicit local empty-payload settlement, never a remote Committed receipt.
  bool no_work = false;
};
SurfaceControlSubmitResult SubmitSurfaceControlDarwin(
    const SurfaceControlSnapshot& snapshot, uint64_t transaction_id,
    bool central_surfaceflinger);
}
