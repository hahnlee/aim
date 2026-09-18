#include "surface_scanout_owner.h"

#include <notify.h>

#include <limits>
#include <new>
#include <string>

namespace darwin_art::window {
namespace {

std::string CompositionNotificationName(uint32_t surface_id) {
  return "dev.darwinart.surface." + std::to_string(surface_id) +
         ".composition";
}

}  // namespace

SurfaceScanoutOwner::SurfaceScanoutOwner(
    CompositionFenceReadyCallback ready_callback, void* ready_context)
    : fence_monitor_(new CompositionFenceMonitor(ready_callback, ready_context)) {}

SurfaceScanoutOwner::~SurfaceScanoutOwner() { StopAndJoin(); }

std::unique_ptr<SurfaceScanoutOwner> SurfaceScanoutOwner::Create(
    CompositionFenceReadyCallback ready_callback, void* ready_context) noexcept {
  try {
    return std::unique_ptr<SurfaceScanoutOwner>(
        new SurfaceScanoutOwner(ready_callback, ready_context));
  } catch (...) {
    return nullptr;
  }
}

void SurfaceScanoutOwner::CancelNotification(int token) noexcept {
  if (token >= 0) (void)notify_cancel(token);
}

bool SurfaceScanoutOwner::RebindBacking(uint32_t iosurface_id) noexcept {
  int previous = -1;
  int token = -1;
  try {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closing_) return false;
    previous = notification_token_;
    notification_token_ = -1;
    reimport_backing_ = false;
    last_requested_generation_ = 0;
    last_requested_embedded_frame_ = 0;
    if (iosurface_id != 0) {
      const std::string name = CompositionNotificationName(iosurface_id);
      if (notify_register_check(name.c_str(), &token) == NOTIFY_STATUS_OK)
        notification_token_ = token;
    }
  } catch (...) {
    CancelNotification(previous);
    return false;
  }
  CancelNotification(previous);
  return iosurface_id == 0 || token >= 0;
}

bool SurfaceScanoutOwner::TrackCompositionFence(int fence_fd) noexcept {
  bool rejected;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    rejected = closing_;
  }
  // Provider FD release may re-enter host resource ownership. Never call it
  // under the scanout scheduler mutex; the external surface lease pins this
  // owner/monitor through the complete Track call.
  if (rejected || fence_monitor_ == nullptr) {
    if (fence_fd >= 0) CompositionFenceMonitor::CloseRejected(fence_fd);
    return false;
  }
  return fence_monitor_->Track(fence_fd);
}

bool SurfaceScanoutOwner::ScanoutReady() const noexcept {
  return fence_monitor_ != nullptr && fence_monitor_->ScanoutReady();
}

uint64_t SurfaceScanoutOwner::SubmittedGeneration() const noexcept {
  return fence_monitor_ == nullptr ? 0 : fence_monitor_->SubmittedGeneration();
}

bool SurfaceScanoutOwner::ClaimDisplayNotificationLocked() noexcept {
  if (notification_token_ < 0) return false;
  int changed = 0;
  if (notify_check(notification_token_, &changed) != NOTIFY_STATUS_OK ||
      changed == 0)
    return false;
  reimport_backing_ = true;
  return true;
}

bool SurfaceScanoutOwner::ClaimScanoutDirtyLocked(uint64_t embedded_frame) noexcept {
  bool claimed = false;
  if (fence_monitor_ != nullptr) {
    const uint64_t submitted = fence_monitor_->SubmittedGeneration();
    const uint64_t ready = fence_monitor_->ReadyGeneration();
    // Track may publish a newer fence after Request's first readiness check.
    // Neither dirty source may bypass that producer fence. The independently
    // completed display notification is handled before this helper instead.
    if (submitted != 0 && ready < submitted) return false;
    if (submitted != 0 && last_requested_generation_ < submitted) {
      last_requested_generation_ = submitted;
      claimed = true;
    }
  }
  if (embedded_frame != 0 &&
      last_requested_embedded_frame_ < embedded_frame) {
    last_requested_embedded_frame_ = embedded_frame;
    claimed = true;
  }
  if (!claimed) ++dirty_skipped_;
  return claimed;
}

SurfaceScanoutRequestResult SurfaceScanoutOwner::Request(
    uint64_t embedded_frame, bool on_main) noexcept {
  std::lock_guard<std::mutex> lock(mutex_);
  if (closing_) return SurfaceScanoutRequestResult::kClosed;
  ++requests_;
  const bool display_dirty = ClaimDisplayNotificationLocked();
  if (!display_dirty &&
      (fence_monitor_ == nullptr || !fence_monitor_->ScanoutReady())) {
    ++fence_gated_;
    return SurfaceScanoutRequestResult::kGated;
  }
  if (!display_dirty && !ClaimScanoutDirtyLocked(embedded_frame))
    return SurfaceScanoutRequestResult::kNoWork;
  if (on_main) {
    ++present_calls_;
    return SurfaceScanoutRequestResult::kPresentNow;
  }
  ++requested_;
  if (scheduled_) {
    ++coalesced_;
    return SurfaceScanoutRequestResult::kAlreadyPending;
  }
  scheduled_ = true;
  return SurfaceScanoutRequestResult::kWakeMain;
}

bool SurfaceScanoutOwner::BeginDrain(uint64_t* request_generation) noexcept {
  if (request_generation == nullptr) return false;
  std::lock_guard<std::mutex> lock(mutex_);
  if (closing_) {
    scheduled_ = false;
    return false;
  }
  if (!scheduled_) return false;
  *request_generation = requested_;
  return true;
}

bool SurfaceScanoutOwner::CompleteDrain(uint64_t request_generation,
                                        int32_t status) noexcept {
  bool schedule_next = false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    ++present_calls_;
    last_status_ = status;
    if (closing_) {
      scheduled_ = false;
    } else if (requested_ != request_generation) {
      schedule_next = true;
    } else {
      scheduled_ = false;
    }
  }
  return schedule_next;
}

bool SurfaceScanoutOwner::TakeReimportRequest() noexcept {
  std::lock_guard<std::mutex> lock(mutex_);
  const bool result = reimport_backing_;
  reimport_backing_ = false;
  return result;
}

bool SurfaceScanoutOwner::BeginClose() noexcept {
  int notification = -1;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    if (closing_) return false;
    closing_ = true;
    notification = notification_token_;
    notification_token_ = -1;
    scheduled_ = false;
  }
  CancelNotification(notification);
  return true;
}

void SurfaceScanoutOwner::StopAndJoin() noexcept {
  (void)BeginClose();
  if (fence_monitor_ != nullptr) fence_monitor_->Stop();
}

SurfaceScanoutCounters SurfaceScanoutOwner::SnapshotCounters() const noexcept {
  std::lock_guard<std::mutex> lock(mutex_);
  return {requests_, fence_gated_, coalesced_, dirty_skipped_, present_calls_,
          last_status_};
}

}  // namespace darwin_art::window
