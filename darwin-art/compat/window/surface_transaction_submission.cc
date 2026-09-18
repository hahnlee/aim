#include "surface_transaction_submission.h"

#include <android/hardware_buffer.h>

#include <chrono>
#include <cstddef>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <deque>
#include <memory>
#include <mutex>
#include <new>
#include <thread>
#include <utility>

extern "C" int sync_wait(int fd, int timeout_ms);
extern "C" void darwin_art_android_surface_transaction_clear(void* opaque);

namespace darwin_art::window {
namespace {

using Result = SurfaceTransactionSubmissionResult;

std::unique_ptr<SurfaceTransaction> CopyTransaction(
    const SurfaceTransaction* source) noexcept {
  if (source == nullptr) return nullptr;
  try {
    // Copy into a fresh slot before touching the public object.  The pointer
    // fields are ownership tokens, not borrowed object graphs; ownership
    // transfers only after the copy and queue insertion both succeed.
    auto copy = std::make_unique<SurfaceTransaction>();
    copy->controls = source->controls;
    copy->updates = source->updates;
    copy->commits = source->commits;
    copy->completes = source->completes;
    copy->discards = source->discards;
    copy->buffer_callbacks = source->buffer_callbacks;
    return copy;
  } catch (...) {
    return nullptr;
  }
}

void AbortTransaction(std::unique_ptr<SurfaceTransaction> transaction) noexcept {
  if (transaction == nullptr) return;
  // Discard before releasing buffers/controls.  The lifetime owner duplicates
  // the original acquire fence where possible and only quarantines on its
  // documented dup-failure path; an unsignaled producer is never reused early.
  DiscardTransactionCallbacks(transaction.get());
  ReleaseTransactionBuffers(transaction.get());
  ReleaseTransactionControls(transaction.get());
}

bool AcquireFencesReady(const SurfaceTransaction& transaction) noexcept {
  for (const auto& update : transaction.updates) {
    if (update.acquire_fence >= 0 && sync_wait(update.acquire_fence, 0) != 0) {
      return false;
    }
  }
  return true;
}

bool ApplyOwned(std::unique_ptr<SurfaceTransaction> transaction) noexcept {
  if (transaction == nullptr) return false;
  try {
    // Keep stats alive through callbacks and all resource cleanup.  The port
    // only performs a ready compositor operation; Android callback policy and
    // transaction ownership remain here.
    SurfaceTransactionStats stats;
    if (!ApplyReadySurfaceTransaction(transaction.get(), &stats)) {
      std::fprintf(stderr,
                   "ART Android SurfaceTransaction: ready port failed; aborting payload\n");
      AbortTransaction(std::move(transaction));
      return false;
    }
    CompleteSurfaceTransaction(transaction.get(), &stats);
    return true;
  } catch (...) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: apply raised; aborting payload\n");
    AbortTransaction(std::move(transaction));
    return false;
  }
}

}  // namespace

struct SurfaceTransactionSubmission::State {
  std::mutex mutex;
  std::condition_variable condition;
  std::deque<std::unique_ptr<SurfaceTransaction>> pending;
  std::thread worker;
  size_t active = 0;
  bool closed = false;
  bool ever_started = false;
  bool worker_started = false;
  bool worker_exited = false;
  bool worker_joined = true;
  bool join_in_progress = false;
};

SurfaceTransactionSubmission::SurfaceTransactionSubmission()
    : state_(new (std::nothrow) State()) {}

SurfaceTransactionSubmission::~SurfaceTransactionSubmission() {
  if (state_ == nullptr) return;
  CloseAdmission();
  std::thread joiner;
  {
    std::unique_lock<std::mutex> lock(state_->mutex);
    state_->condition.wait(lock, [this] {
      return state_->active == 0 && state_->pending.empty() &&
             (!state_->worker_started || state_->worker_exited);
    });
    if (state_->worker.joinable() &&
        state_->worker.get_id() != std::this_thread::get_id()) {
      joiner = std::move(state_->worker);
      state_->worker_joined = false;
    }
  }
  // The owner is never destroyed by its worker callback in production.  Do
  // not silently detach a live worker: a self-destruction is a programmer
  // error and retaining a joinable handle is safer than leaking work.
  if (joiner.joinable()) joiner.join();
  if (state_->worker.joinable()) std::terminate();
  delete state_;
  state_ = nullptr;
}

SurfaceTransactionSubmission::Result
SurfaceTransactionSubmission::SubmitSurfaceTransaction(
    SurfaceTransaction* transaction) noexcept {
  if (state_ == nullptr || transaction == nullptr) return Result::kFailed;

  {
    std::lock_guard<std::mutex> lock(state_->mutex);
    if (state_->closed) return Result::kClosed;
  }

  std::unique_ptr<SurfaceTransaction> copy = CopyTransaction(transaction);
  if (copy == nullptr) return Result::kFailed;

  bool synchronous = false;
  {
    std::unique_lock<std::mutex> lock(state_->mutex);
    if (state_->closed) return Result::kClosed;

    // A ready transaction can be applied by the submitting thread only when
    // it cannot overtake a queued or active predecessor.  Fence readiness is
    // a zero-time poll; no synchronous fence wait is permitted here.
    synchronous = state_->pending.empty() && state_->active == 0 &&
                  AcquireFencesReady(*copy);
    if (synchronous) {
      ++state_->active;
      *transaction = SurfaceTransaction{};
    } else {
      if (!state_->worker_started) {
        try {
          state_->worker_started = true;
          state_->worker_exited = false;
          state_->worker_joined = false;
          state_->ever_started = true;
          state_->worker = std::thread([state = state_] {
            for (;;) {
              std::unique_ptr<SurfaceTransaction> item;
              bool discard = false;
              {
                std::unique_lock<std::mutex> worker_lock(state->mutex);
                state->condition.wait(worker_lock, [state] {
                  return state->active == 0 &&
                         (state->closed || !state->pending.empty());
                });
                if (state->pending.empty() && state->closed &&
                    state->active == 0) {
                  state->worker_exited = true;
                  state->condition.notify_all();
                  return;
                }
                item = std::move(state->pending.front());
                state->pending.pop_front();
                ++state->active;
                discard = state->closed;
              }

              if (!discard) {
                // Poll finite intervals so CloseAdmission can cancel an
                // unlatched batch promptly without parking this worker in
                // sync_wait(..., -1).
                for (;;) {
                  {
                    std::unique_lock<std::mutex> worker_lock(state->mutex);
                    if (state->closed) {
                      discard = true;
                      break;
                    }
                  }
                  if (AcquireFencesReady(*item)) break;
                  std::unique_lock<std::mutex> worker_lock(state->mutex);
                  state->condition.wait_for(worker_lock,
                                            std::chrono::milliseconds(1));
                }
              }

              if (discard) {
                AbortTransaction(std::move(item));
              } else {
                (void)ApplyOwned(std::move(item));
              }
              {
                std::lock_guard<std::mutex> worker_lock(state->mutex);
                --state->active;
                state->condition.notify_all();
              }
            }
          });
        } catch (...) {
          state_->worker_started = false;
          state_->worker_exited = false;
          state_->worker_joined = true;
          return Result::kFailed;
        }
      }
      try {
        state_->pending.push_back(std::move(copy));
      } catch (...) {
        return Result::kFailed;
      }
      *transaction = SurfaceTransaction{};
      state_->condition.notify_one();
      return Result::kAccepted;
    }
  }

  (void)ApplyOwned(std::move(copy));
  {
    std::lock_guard<std::mutex> lock(state_->mutex);
    --state_->active;
    state_->condition.notify_all();
  }
  // Admission succeeded before the ready port was called.  A port failure is
  // an already-admitted resource abort (reported by ApplyOwned); it must not
  // be confused with pre-admission Failed, since the public transaction was
  // detached and an arbitrary callback may already have reused/deleted it.
  return Result::kAccepted;
}

void SurfaceTransactionSubmission::CloseAdmission() noexcept {
  if (state_ == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(state_->mutex);
    state_->closed = true;
  }
  state_->condition.notify_all();
}

bool SurfaceTransactionSubmission::PollQuiesced() noexcept {
  if (state_ == nullptr) return false;
  std::thread joiner;
  {
    std::lock_guard<std::mutex> lock(state_->mutex);
    if (!state_->closed || !state_->pending.empty() || state_->active != 0 ||
        (state_->worker_started && !state_->worker_exited) ||
        state_->join_in_progress) {
      return false;
    }
    if (state_->worker.joinable()) {
      if (state_->worker.get_id() == std::this_thread::get_id()) return false;
      joiner = std::move(state_->worker);
      state_->worker_joined = false;
      state_->join_in_progress = true;
    }
  }
  if (joiner.joinable()) joiner.join();
  {
    std::lock_guard<std::mutex> lock(state_->mutex);
    state_->worker_joined = true;
    state_->worker_started = false;
    state_->join_in_progress = false;
    state_->condition.notify_all();
  }
  return true;
}

bool SurfaceTransactionSubmission::Reopen() noexcept {
  if (state_ == nullptr) return false;
  std::lock_guard<std::mutex> lock(state_->mutex);
  if (!state_->closed) {
    return !state_->ever_started && state_->pending.empty() &&
           state_->active == 0;
  }
  if (!state_->pending.empty() || state_->active != 0 ||
      (state_->worker_started && !state_->worker_exited) ||
      state_->worker.joinable() || !state_->worker_joined ||
      state_->join_in_progress) {
    return false;
  }
  state_->closed = false;
  state_->worker_started = false;
  state_->worker_exited = false;
  return true;
}

bool SurfaceTransactionSubmission::Reset() noexcept { return Reopen(); }

SurfaceTransactionSubmission& GlobalSubmission() {
  static SurfaceTransactionSubmission submission;
  return submission;
}
}  // namespace darwin_art::window

extern "C" void ASurfaceTransaction_apply(ASurfaceTransaction* opaque) {
  if (opaque == nullptr) return;
  auto* transaction =
      reinterpret_cast<darwin_art::window::SurfaceTransaction*>(opaque);
  const auto result =
      darwin_art::window::GlobalSubmission().SubmitSurfaceTransaction(transaction);
  if (result == darwin_art::window::SurfaceTransactionSubmissionResult::kClosed) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: apply rejected after admission close\n");
    // Keep the public allocation valid for the caller's ordinary delete.  The
    // real lifetime owner performs discard callbacks/resource release.
    darwin_art_android_surface_transaction_clear(transaction);
  } else if (result ==
             darwin_art::window::SurfaceTransactionSubmissionResult::kFailed) {
    std::fprintf(stderr,
                 "ART Android SurfaceTransaction: admission allocation failed; terminating\n");
    // The void NDK API cannot report a failed admission. Returning would
    // silently drop the caller's transaction. No callback has run here.
    std::abort();
  }
}

extern "C" void CloseSurfaceTransactionSubmissionAdmission() {
  darwin_art::window::GlobalSubmission().CloseAdmission();
}

extern "C" bool PollSurfaceTransactionSubmissionQuiesced() {
  return darwin_art::window::GlobalSubmission().PollQuiesced();
}

extern "C" bool ResetSurfaceTransactionSubmission() {
  return darwin_art::window::GlobalSubmission().Reset();
}
