#pragma once

#include "surface_transaction_lifetime.h"
#include "surface_transaction_submission_lifecycle.h"

#include <cstdint>

namespace darwin_art::window {

// The port owns only the ready compositor operation.  It must not wait on an
// acquire fence, invoke an Android callback, or enqueue work.  Submission
// owns those policy and lifetime boundaries and calls CompleteSurfaceTransaction
// after a successful port operation.
// Production calls are serialized by GlobalSubmission's active admission gate
// across its synchronous and worker paths. Registry commit and retained capture
// are separate locked operations; do not call this concurrently outside that
// owner or introduce another ready-transaction worker for the same registry.
bool ApplyReadySurfaceTransaction(SurfaceTransaction* transaction,
                                  SurfaceTransactionStats* stats);

enum class SurfaceTransactionSubmissionResult : uint8_t {
  kAccepted,
  kClosed,
  kFailed,
  Accepted = kAccepted,
  Closed = kClosed,
  Failed = kFailed,
};

// Android's transaction apply/latch owner.  At most one worker is created for
// an admission epoch.  Accepted payload is detached from the caller before
// returning, while rejected payload remains entirely caller-owned.
class SurfaceTransactionSubmission final {
 public:
  using Result = SurfaceTransactionSubmissionResult;

  SurfaceTransactionSubmission();
  SurfaceTransactionSubmission(const SurfaceTransactionSubmission&) = delete;
  SurfaceTransactionSubmission& operator=(const SurfaceTransactionSubmission&) = delete;
  ~SurfaceTransactionSubmission();

  // Nonblocking with respect to acquire fences and callbacks.  kClosed and
  // kFailed leave transaction untouched.  kAccepted clears the public
  // transaction and takes ownership of its detached payload.
  [[nodiscard]] Result SubmitSurfaceTransaction(
      SurfaceTransaction* transaction) noexcept;

  // Closes new admissions and wakes the worker.  Queued, unlatched payload is
  // discarded by the worker using the real transaction lifetime owner, which
  // forwards each original acquire fence to its discard callback.
  void CloseAdmission() noexcept;

  // A snapshot.  Once closed, this joins an already-exited worker outside the
  // queue mutex and reports true only after queue, active callbacks/stats,
  // resource cleanup, worker exit, and worker join have all completed.
  bool PollQuiesced() noexcept;

  // Reopens a closed, fully drained and joined admission epoch.  Reset is an
  // intentionally equivalent spelling used by registration/shutdown code.
  // A never-started empty owner may also be reopened during initial setup.
  bool Reopen() noexcept;
  bool Reset() noexcept;

 private:
  struct State;
  State* state_;
};

using SurfaceTransactionLatchOwner = SurfaceTransactionSubmission;
using SurfaceTransactionSubmissionOwner = SurfaceTransactionSubmission;

}  // namespace darwin_art::window

// Narrow C ABI used by libandroid callers and by registration/shutdown.
extern "C" void ASurfaceTransaction_apply(ASurfaceTransaction* transaction);
