#pragma once

#include "composition_fence_monitor.h"

#include <cstdint>
#include <memory>
#include <mutex>

namespace darwin_art::window {

enum class SurfaceScanoutRequestResult : uint8_t {
  kGated,
  kNoWork,
  kPresentNow,
  kWakeMain,
  kAlreadyPending,
  kClosed,
};

struct SurfaceScanoutCounters final {
  uint64_t requests = 0;
  uint64_t fence_gated = 0;
  uint64_t coalesced = 0;
  uint64_t dirty_skipped = 0;
  uint64_t present_calls = 0;
  int32_t last_status = 0;
};

// Owns presentation scheduling, notify/fence readiness and scanout dirty
// generations. The callback context is opaque to this owner; bridge code
// performs all AppKit, Metal, surface and window work after callbacks return.
class SurfaceScanoutOwner final {
 public:
  static std::unique_ptr<SurfaceScanoutOwner> Create(
      CompositionFenceReadyCallback ready_callback,
      void* ready_context) noexcept;
  ~SurfaceScanoutOwner();

  SurfaceScanoutOwner(const SurfaceScanoutOwner&) = delete;
  SurfaceScanoutOwner& operator=(const SurfaceScanoutOwner&) = delete;

  // Rebind the composition notification to the exact current IOSurface. A
  // zero id leaves notification dirty state cleared without registering.
  bool RebindBacking(uint32_t iosurface_id) noexcept;
  bool TrackCompositionFence(int fence_fd) noexcept;
  bool ScanoutReady() const noexcept;
  uint64_t SubmittedGeneration() const noexcept;

  // Request consumes the embedded-frame dirty source independently from fence
  // generations. on_main selects direct presentation; worker requests return
  // whether the AppKit actor must be woken or already has one pending.
  SurfaceScanoutRequestResult Request(uint64_t embedded_frame,
                                       bool on_main) noexcept;
  bool BeginDrain(uint64_t* request_generation) noexcept;
  bool CompleteDrain(uint64_t request_generation, int32_t status) noexcept;
  bool TakeReimportRequest() noexcept;

  // BeginClose is the first teardown admission. StopAndJoin then prevents
  // fence callbacks, joins the monitor, and releases all remaining resources.
  bool BeginClose() noexcept;
  void StopAndJoin() noexcept;

  SurfaceScanoutCounters SnapshotCounters() const noexcept;

 private:
  SurfaceScanoutOwner(CompositionFenceReadyCallback ready_callback,
                      void* ready_context);
  bool ClaimDisplayNotificationLocked() noexcept;
  bool ClaimScanoutDirtyLocked(uint64_t embedded_frame) noexcept;
  void CancelNotification(int token) noexcept;

  mutable std::mutex mutex_;
  std::unique_ptr<CompositionFenceMonitor> fence_monitor_;
  int notification_token_ = -1;
  bool reimport_backing_ = false;
  bool closing_ = false;
  bool scheduled_ = false;
  uint64_t requested_ = 0;
  uint64_t last_requested_generation_ = 0;
  uint64_t last_requested_embedded_frame_ = 0;
  uint64_t requests_ = 0;
  uint64_t fence_gated_ = 0;
  uint64_t coalesced_ = 0;
  uint64_t dirty_skipped_ = 0;
  uint64_t present_calls_ = 0;
  int32_t last_status_ = 0;
};

}  // namespace darwin_art::window
