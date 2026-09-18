#pragma once

#include "surface_transaction_state.h"

#include <unordered_map>

namespace darwin_art::window {

// Callback-visible transaction statistics. The C ABI presents this object as
// ASurfaceTransactionStats while the callback is running; its owner retains
// previous buffers and the present fence until callback cleanup completes.
struct SurfaceTransactionStats {
  SurfaceTransactionStats() = default;
  SurfaceTransactionStats(const SurfaceTransactionStats&) = delete;
  SurfaceTransactionStats& operator=(const SurfaceTransactionStats&) = delete;
  SurfaceTransactionStats(SurfaceTransactionStats&&) = delete;
  SurfaceTransactionStats& operator=(SurfaceTransactionStats&&) = delete;
  std::vector<ASurfaceControl*> controls;
  std::unordered_map<ASurfaceControl*, AHardwareBuffer*> previous_buffers;
  std::unordered_map<ASurfaceControl*, uint64_t> previous_submission_cookies;
  int present_fence = -1;

  ~SurfaceTransactionStats();
};

// These operations consume ownership only after the caller has left any
// structural-state mutex. They preserve the existing discard fence forwarding
// and callback ordering contract. Callback groups and every duplicated fence
// are detached before dispatch; callbacks may delete/clear/reuse the source.
// A buffer discard callback consumes its fence descriptor (or -1/-2 sentinel).
void DiscardBufferCallbacks(SurfaceTransaction* transaction,
                            ASurfaceControl* control = nullptr,
                            bool quarantine = false);
void DiscardTransactionCallbacks(SurfaceTransaction* transaction);
void ReleaseTransactionControls(SurfaceTransaction* transaction);
void ReleaseTransactionBuffers(SurfaceTransaction* transaction);

// On failure, no callback/context ownership is transferred and the existing
// callback list is unchanged. No callback runs during registration.
bool RegisterSurfaceTransactionBufferCallbacks(
    SurfaceTransaction* transaction, ASurfaceControl* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int)) noexcept;

// Detaches the entire payload before dispatch. A callback may synchronously
// clear/delete/reuse the public transaction; cleanup accesses only the detached
// payload and leaves any newly added public state intact.
void CompleteSurfaceTransaction(SurfaceTransaction* transaction,
                                SurfaceTransactionStats* stats);

}  // namespace darwin_art::window
