#include "surface_transaction_lifetime.h"

#include "surface_transaction_builder.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <new>
#include <stdexcept>
#include <utility>

extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer);
extern "C" void ASurfaceControl_release(ASurfaceControl* control);
extern "C" int darwin_art_bionic_socket_broker_close(int fd);
extern "C" int darwin_art_bionic_socket_broker_dup(int fd);
extern "C" void darwin_art_android_mark_hardware_buffer_released(void* buffer);

namespace darwin_art::window {

namespace {
struct PreparedBufferDiscard {
  SurfaceTransaction::BufferCallback callback;
  int fence;
  PreparedBufferDiscard(SurfaceTransaction::BufferCallback value, int fd)
      : callback(value), fence(fd) {}
  PreparedBufferDiscard(const PreparedBufferDiscard&) = delete;
  PreparedBufferDiscard& operator=(const PreparedBufferDiscard&) = delete;
  PreparedBufferDiscard(PreparedBufferDiscard&& other) noexcept
      : callback(other.callback), fence(std::exchange(other.fence, -1)) {}
  ~PreparedBufferDiscard() {
    if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
  }
  void Invoke() {
    // The private producer callback consumes its duplicate descriptor.
    const int transferred = std::exchange(fence, -1);
    callback.discard(callback.context, transferred);
  }
};

std::vector<PreparedBufferDiscard> DetachBufferDiscards(
    SurfaceTransaction* transaction, ASurfaceControl* control, bool quarantine) {
  std::vector<PreparedBufferDiscard> prepared;
  const auto matches = [control](const SurfaceTransaction::BufferCallback& value) {
    return control == nullptr || value.control == control;
  };
  prepared.reserve(static_cast<size_t>(std::count_if(
      transaction->buffer_callbacks.begin(), transaction->buffer_callbacks.end(),
      matches)));
  // Prepare all fence ownership before callbacks can delete/clear/reuse the
  // source transaction. Failure before detachment leaves callback state intact.
  for (const auto& callback : transaction->buffer_callbacks) {
    if (!matches(callback) || callback.discard == nullptr) continue;
    int fence = quarantine ? -2 : -1;
    if (!quarantine) {
      for (const auto& update : transaction->updates) {
        if (update.opaque != callback.control || update.acquire_fence < 0) continue;
        fence = darwin_art_bionic_socket_broker_dup(update.acquire_fence);
        if (fence < 0) fence = -2;  // Existing producer quarantine contract.
        break;
      }
    }
    prepared.emplace_back(callback, fence);
  }
  auto& callbacks = transaction->buffer_callbacks;
  callbacks.erase(std::remove_if(callbacks.begin(), callbacks.end(), matches),
                  callbacks.end());
  return prepared;
}

int DuplicateReleaseFence(int fence) {
  if (fence < 0) return -1;
  const int duplicate = darwin_art_bionic_socket_broker_dup(fence);
  if (duplicate >= 0) return duplicate;
  // The NDK's -1 means no pending fence, not failure to retain a real fence.
  // Returning it on resource exhaustion would allow early producer reuse.
  std::fprintf(stderr,
               "ART Android SurfaceTransaction: release fence duplication failed fd=%d; terminating\n",
               fence);
  std::abort();
}
}  // namespace

SurfaceTransactionStats::~SurfaceTransactionStats() {
  for (const auto& [control, buffer] : previous_buffers) {
    (void)control;
    AHardwareBuffer_release(buffer);
  }
  if (present_fence >= 0) {
    (void)darwin_art_bionic_socket_broker_close(present_fence);
  }
}

void DiscardBufferCallbacks(SurfaceTransaction* transaction,
                            ASurfaceControl* control, bool quarantine) {
  if (transaction == nullptr) return;
  auto prepared = DetachBufferDiscards(transaction, control, quarantine);
  for (auto& callback : prepared) callback.Invoke();
}

void DiscardTransactionCallbacks(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  auto prepared = DetachBufferDiscards(transaction, nullptr, false);
  auto callbacks = std::move(transaction->discards);
  transaction->discards.clear();
  transaction->commits.clear();
  transaction->completes.clear();
  // No source access after the first callback: even buffer discard may delete
  // the transaction, not only the ordinary discard callback below.
  for (auto& callback : prepared) callback.Invoke();
  for (const auto& callback : callbacks) {
    if (callback.function != nullptr) callback.function(callback.context);
  }
}

void ReleaseTransactionControls(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  auto controls = std::move(transaction->controls);
  transaction->controls.clear();
  for (ASurfaceControl* control : controls) {
    if (control != nullptr) ASurfaceControl_release(control);
  }
}

void ReleaseTransactionBuffers(SurfaceTransaction* transaction) {
  if (transaction == nullptr) return;
  for (auto& update : transaction->updates) {
    if (update.buffer != nullptr) AHardwareBuffer_release(update.buffer);
    if (update.acquire_fence >= 0) {
      (void)darwin_art_bionic_socket_broker_close(update.acquire_fence);
    }
    update.buffer = nullptr;
    update.acquire_fence = -1;
    update.submission_cookie = 0;
  }
}

void CompleteSurfaceTransaction(SurfaceTransaction* transaction,
                                SurfaceTransactionStats* stats) {
  if (transaction == nullptr || stats == nullptr) return;
  // Detach the complete ownership payload before invoking arbitrary client
  // callbacks. A callback may clear, reuse, or delete the original opaque
  // transaction; all cleanup below must therefore target only this batch.
  SurfaceTransaction batch = std::move(*transaction);
  *transaction = SurfaceTransaction{};
  auto commits = std::move(batch.commits);
  auto completes = std::move(batch.completes);
  auto buffer_callbacks = std::move(batch.buffer_callbacks);
  batch.buffer_callbacks.clear();
  batch.commits.clear();
  batch.completes.clear();
  batch.discards.clear();
  auto* opaque_stats = reinterpret_cast<ASurfaceTransactionStats*>(stats);
  for (const auto& callback : commits) {
    if (callback.function != nullptr)
      callback.function(callback.context, opaque_stats);
  }
  for (const auto& callback : completes) {
    if (callback.function != nullptr)
      callback.function(callback.context, opaque_stats);
  }
  for (const auto& callback : buffer_callbacks) {
    if (callback.complete != nullptr)
      callback.complete(callback.context, opaque_stats);
  }
  ReleaseTransactionBuffers(&batch);
  ReleaseTransactionControls(&batch);
}

bool RegisterSurfaceTransactionBufferCallbacks(
    SurfaceTransaction* transaction, ASurfaceControl* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int)) noexcept {
  if (transaction == nullptr || complete == nullptr || discard == nullptr)
    return false;
  try {
    transaction->buffer_callbacks.push_back({control, context, complete, discard});
    return true;
  } catch (const std::bad_alloc&) {
    return false;
  } catch (const std::length_error&) {
    return false;
  }
}

}  // namespace darwin_art::window

extern "C" ASurfaceTransaction* ASurfaceTransaction_create() {
  return reinterpret_cast<ASurfaceTransaction*>(
      new (std::nothrow) darwin_art::window::SurfaceTransaction());
}

extern "C" void ASurfaceTransaction_delete(ASurfaceTransaction* opaque) {
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  if (transaction == nullptr) return;
  darwin_art::window::SurfaceTransaction batch = std::move(*transaction);
  *transaction = darwin_art::window::SurfaceTransaction{};
  // Delete the now-empty public allocation before arbitrary discard callbacks;
  // cleanup below owns only the detached batch.
  delete transaction;
  darwin_art::window::DiscardTransactionCallbacks(&batch);
  darwin_art::window::ReleaseTransactionBuffers(&batch);
  darwin_art::window::ReleaseTransactionControls(&batch);
}

extern "C" void darwin_art_android_surface_transaction_clear(void* opaque) {
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  if (transaction == nullptr) return;
  darwin_art::window::SurfaceTransaction batch = std::move(*transaction);
  *transaction = darwin_art::window::SurfaceTransaction{};
  darwin_art::window::DiscardTransactionCallbacks(&batch);
  darwin_art::window::ReleaseTransactionBuffers(&batch);
  darwin_art::window::ReleaseTransactionControls(&batch);
}

extern "C" void ASurfaceTransaction_setOnCommit(
    ASurfaceTransaction* opaque, void* context,
    ASurfaceTransaction_OnCommit callback) {
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  if (transaction != nullptr && callback != nullptr) {
    transaction->commits.push_back({callback, context});
  }
}

extern "C" void ASurfaceTransaction_setOnComplete(
    ASurfaceTransaction* opaque, void* context,
    ASurfaceTransaction_OnComplete callback) {
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  if (transaction != nullptr && callback != nullptr) {
    transaction->completes.push_back({callback, context});
  }
}

extern "C" void darwin_art_android_surface_transaction_set_on_discard(
    void* opaque, void* context, void (*callback)(void*)) {
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  if (transaction != nullptr && callback != nullptr) {
    transaction->discards.push_back({callback, context});
  }
}

extern "C" bool darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
    void* opaque, void* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int)) {
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  return darwin_art::window::RegisterSurfaceTransactionBufferCallbacks(
      transaction, reinterpret_cast<ASurfaceControl*>(control), context,
      complete, discard);
}

extern "C" void darwin_art_android_surface_transaction_set_buffer_callbacks(
    void* opaque, void* control, void* context,
    void (*complete)(void*, ASurfaceTransactionStats*),
    void (*discard)(void*, int)) {
  if (opaque == nullptr || complete == nullptr || discard == nullptr) return;
  // The historical void contract cannot report failed ownership transfer.
  // Recoverable product callers use the checked operation instead.
  if (!darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
          opaque, control, context, complete, discard)) std::abort();
}

extern "C" void ASurfaceTransactionStats_getASurfaceControls(
    ASurfaceTransactionStats* opaque, ASurfaceControl*** out, size_t* count) {
  if (out == nullptr || count == nullptr) return;
  auto* stats = reinterpret_cast<darwin_art::window::SurfaceTransactionStats*>(
      opaque);
  *count = stats == nullptr ? 0 : stats->controls.size();
  if (*count == 0) {
    *out = nullptr;
    return;
  }
  *out = static_cast<ASurfaceControl**>(std::malloc(*count * sizeof(**out)));
  if (*out == nullptr) {
    *count = 0;
    return;
  }
  std::memcpy(*out, stats->controls.data(), *count * sizeof(**out));
}

extern "C" void ASurfaceTransactionStats_releaseASurfaceControls(
    ASurfaceControl** controls) {
  std::free(controls);
}

extern "C" int ASurfaceTransactionStats_getPreviousReleaseFenceFd(
    ASurfaceTransactionStats* opaque, ASurfaceControl* control) {
  auto* stats = reinterpret_cast<darwin_art::window::SurfaceTransactionStats*>(
      opaque);
  if (stats == nullptr || control == nullptr ||
      std::find(stats->controls.begin(), stats->controls.end(), control) ==
          stats->controls.end()) {
    return -1;
  }
  const auto previous = stats->previous_buffers.find(control);
  if (previous == stats->previous_buffers.end()) return -1;
  const int fence = darwin_art::window::DuplicateReleaseFence(stats->present_fence);
  darwin_art_android_mark_hardware_buffer_released(previous->second);
  return fence;
}

extern "C" bool
ASurfaceTransactionStats_getPreviousBufferMetadata(
    ASurfaceTransactionStats* opaque, ASurfaceControl* control,
    AHardwareBuffer** buffer, uint64_t* submission_cookie) {
  if (buffer == nullptr || submission_cookie == nullptr) return false;
  *buffer = nullptr;
  *submission_cookie = 0;
  auto* stats = reinterpret_cast<darwin_art::window::SurfaceTransactionStats*>(
      opaque);
  if (stats == nullptr || control == nullptr) return false;
  const auto previous = stats->previous_buffers.find(control);
  const auto cookie = stats->previous_submission_cookies.find(control);
  if (previous == stats->previous_buffers.end() ||
      cookie == stats->previous_submission_cookies.end() ||
      previous->second == nullptr || cookie->second == 0) {
    return false;
  }
  *buffer = previous->second;
  *submission_cookie = cookie->second;
  return true;
}

extern "C" int ASurfaceTransactionStats_getPresentFenceFd(
    ASurfaceTransactionStats* opaque) {
  auto* stats = reinterpret_cast<darwin_art::window::SurfaceTransactionStats*>(
      opaque);
  return stats == nullptr
             ? -1
             : darwin_art::window::DuplicateReleaseFence(stats->present_fence);
}

extern "C" int64_t ASurfaceTransactionStats_getLatchTime(
    ASurfaceTransactionStats*) {
  return 0;
}

extern "C" int64_t ASurfaceTransactionStats_getAcquireTime(
    ASurfaceTransactionStats*, ASurfaceControl*) {
  return -1;
}
