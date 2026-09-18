#include "surface_transaction_submission.h"
#include "surface_control_registry.h"
#include "surface_control_submit_darwin.h"
#include "../surfaceflinger/transaction_bridge.h"

#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <unistd.h>

namespace darwin_art::window {
namespace {
std::atomic<uint64_t> next_transaction_id{1};
[[noreturn]] void FailCommittedTransaction() {
  // Once AOSP acceptance or buffer transfer has occurred, discard would return
  // an unlatched producer slot while committed state still retains its buffer.
  // Preserve the existing fatal policy until an upstream recovery contract is
  // available; never manufacture successful completion or rollback here.
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: failure after commit; terminating\n");
  std::abort();
}
}

bool ApplyReadySurfaceTransaction(SurfaceTransaction* transaction,
                                  SurfaceTransactionStats* output_stats) {
  if (transaction == nullptr || output_stats == nullptr) return false;
  const auto environment = QuerySurfaceControlSubmissionEnvironment();
  if (environment.application && !environment.central_service) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: application requires central SurfaceFlinger\n");
    return false;
  }
  auto& stats = *output_stats;
  auto& registry = SurfaceControlRegistry::Instance();
  const uint64_t id = next_transaction_id.fetch_add(1, std::memory_order_relaxed);
  bool committed = false;
  bool submission_attempted = false;
  try {
    PreparedSurfaceCommit prepared;
    if (!registry.PrepareSurfaceCommit(transaction,
            static_cast<uint32_t>(getpid()), environment.central_service,
            &stats, &prepared)) return false;
    const auto& snapshot = *prepared.Snapshot();
    if (!environment.central_service) {
      DarwinArtSurfaceFlingerCommitResult result{};
      if (!darwin_art_surfaceflinger_commit_transaction(
              id, snapshot.frontend_updates.data(),
              snapshot.frontend_updates.size(), &result)) return false;
      committed = true;
    }
    if (std::getenv("DARWIN_ART_DEBUG_SURFACE_TRANSACTIONS") != nullptr) {
      for (const auto& layer : snapshot.presentations) {
        std::fprintf(stderr,
                     "ART Android SurfaceTransaction: ready pid=%d transaction=%llu "
                     "owner=%u local=%u parent=%u:%u buffer=%p name=%s\n",
                     getpid(), static_cast<unsigned long long>(id),
                     layer.owner_process_id, layer.layer_id,
                     layer.parent_owner_process_id, layer.parent_id,
                     static_cast<void*>(layer.buffer), layer.name.c_str());
      }
    }
    submission_attempted = true;
    const auto submitted = SubmitSurfaceControlDarwin(
        snapshot, id, environment.central_service);
    if (submitted.receipt.disposition == DARWIN_ART_SF_COMMIT_COMMITTED)
      committed = true;
    if (!submitted.success || !submitted.context_restored) {
      // Only a proven rejection with a healthy backend can cancel. Missing
      // replies and committed errors cannot return producer ownership safely.
      if (!committed && submitted.context_restored &&
          submitted.receipt.disposition == DARWIN_ART_SF_COMMIT_REJECTED &&
          submitted.receipt.error != 0 && submitted.present_fence < 0 &&
          submitted.receipt.completion_fd < 0) return false;
      FailCommittedTransaction();
    }
    if (!submitted.no_work &&
        (submitted.receipt.disposition != DARWIN_ART_SF_COMMIT_COMMITTED ||
         submitted.receipt.error != 0 || submitted.present_fence < 0 ||
         submitted.present_fence != submitted.receipt.completion_fd))
      FailCommittedTransaction();
    // No allocation or transport remains at this publication boundary.
    if (!registry.FinalizeSurfaceCommit(&prepared)) FailCommittedTransaction();
    stats.present_fence = submitted.present_fence;
    return true;
  } catch (...) {
    if (committed || submission_attempted) FailCommittedTransaction();
    return false;
  }
}
}
