#include "retired_transport_drain.h"

#include "../../../compat/looper/android_looper_owner.h"

#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::input {

struct RetiredTransportDrain::Control {
  struct Slot {
    void* looper = nullptr;
    std::shared_ptr<InputTransport> transport;
    RetiredTransportDrainCallbacks callbacks;
    int fd = -1;
    Registration* current = nullptr;
    bool prepared = false;
    bool active = false;
    bool pending = false;
    bool closed = false;
    bool terminal = false;
    bool operation = false;
    bool callback_remove_pending = false;
    bool terminal_event_pending = false;
    bool failure_notified = false;
    bool continuation_scheduled = false;
    std::uint64_t continuation_generation = 0;
    std::uint64_t request_generation = 0;
    std::uint64_t operation_generation = 0;
    std::uint64_t revision = 0;
  };

  mutable std::mutex mutex;
  Slot slot;
};

struct RetiredTransportDrain::Registration {
  std::shared_ptr<Control> control;
  void* looper = nullptr;
  int fd = -1;
};

struct RetiredTransportDrain::Continuation {
  std::shared_ptr<Control> control;
  std::uint64_t generation = 0;
};

namespace {
constexpr int kOutputEvent = 0x0002;
constexpr int kErrorEvent = 0x0004;
constexpr int kHangupEvent = 0x0008;
constexpr int kInvalidEvent = 0x0010;
}  // namespace

RetiredTransportDrain::RetiredTransportDrain()
    : control_(std::make_shared<Control>()) {}

RetiredTransportDrain::~RetiredTransportDrain() { (void)Retire(); }

RetiredTransportDrainResult RetiredTransportDrain::Prepare(
    void* looper, std::shared_ptr<InputTransport> transport, int fd,
    const RetiredTransportDrainCallbacks& callbacks) {
  if (looper == nullptr || transport == nullptr || fd < 0)
    return RetiredTransportDrainResult::kProviderFailure;
  auto control = control_;
  try {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.prepared || slot.closed)
      return RetiredTransportDrainResult::kProviderFailure;
    slot.looper = looper;
    slot.transport = std::move(transport);
    slot.callbacks = callbacks;
    slot.fd = fd;
    slot.prepared = true;
    slot.pending = slot.transport->HasPendingTx();
    ++slot.revision;
  } catch (const std::bad_alloc&) {
    return RetiredTransportDrainResult::kOutOfMemory;
  }
  return RetiredTransportDrainResult::kApplied;
}

RetiredTransportDrainResult RetiredTransportDrain::Activate() {
  auto control = control_;
  const auto result = ActivateControl(control);
  return DeliverPendingTerminal(control)
             ? result : RetiredTransportDrainResult::kProviderFailure;
}

RetiredTransportDrainResult RetiredTransportDrain::ActivateControl(
    const std::shared_ptr<Control>& control) {
  auto* registration = new (std::nothrow) Registration;
  if (registration == nullptr) return RetiredTransportDrainResult::kOutOfMemory;
  void* looper = nullptr;
  int fd = -1;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (!slot.prepared || slot.closed || slot.terminal) {
      delete registration;
      return slot.terminal ? RetiredTransportDrainResult::kTerminal
                           : RetiredTransportDrainResult::kProviderFailure;
    }
    if (slot.operation || slot.current != nullptr) {
      delete registration;
      return RetiredTransportDrainResult::kDeferred;
    }
    slot.pending = slot.transport->HasPendingTx();
    if (!slot.pending) {
      delete registration;
      return RetiredTransportDrainResult::kApplied;
    }
    slot.operation = true;
    slot.failure_notified = false;
    slot.operation_generation = slot.request_generation;
    slot.current = registration;
    looper = slot.looper;
    fd = slot.fd;
  }
  registration->control = control;
  registration->looper = looper;
  registration->fd = fd;
  int added = -1;
  try {
    added = darwin_art::looper::AddFdOwned(
        looper, fd, 0, kOutputEvent, &RetiredTransportDrain::OnFd,
        registration, registration, &RetiredTransportDrain::Release);
  } catch (const std::bad_alloc&) {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    slot.current = nullptr;
    slot.operation = false;
    ++slot.revision;
    return RetiredTransportDrainResult::kOutOfMemory;
  } catch (...) {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    slot.current = nullptr;
    slot.operation = false;
    ++slot.revision;
    return RetiredTransportDrainResult::kProviderFailure;
  }
  if (added < 0) {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    slot.current = nullptr;
    slot.operation = false;
    ++slot.revision;
    return RetiredTransportDrainResult::kProviderFailure;
  }
  bool close_after_add = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    close_after_add = slot.closed || slot.terminal;
    slot.active = !close_after_add;
    slot.operation = close_after_add;
    slot.pending = slot.request_generation != slot.operation_generation ||
                   slot.transport->HasPendingTx();
    ++slot.revision;
  }
  if (close_after_add) {
    const auto removed = RemoveRegistration(control, registration);
    if (removed != RetiredTransportDrainResult::kApplied) return removed;
    return RetiredTransportDrainResult::kTerminal;
  }
  return RetiredTransportDrainResult::kApplied;
}

RetiredTransportDrainResult RetiredTransportDrain::Request() {
  // Keep the owner control alive independently of this object: progress may
  // synchronously dispose the RetiredTransportDrain itself.
  const auto control = control_;
  const auto result = RequestControl(control);
  return DeliverPendingTerminal(control)
             ? result : RetiredTransportDrainResult::kProviderFailure;
}

RetiredTransportDrainResult RetiredTransportDrain::RequestControl(
    const std::shared_ptr<Control>& control) {
  for (;;) {
    bool activate = false;
    bool drain = false;
    Registration* registration = nullptr;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& slot = control->slot;
      if (slot.closed || slot.terminal)
        return slot.terminal ? RetiredTransportDrainResult::kTerminal
                             : RetiredTransportDrainResult::kProviderFailure;
      if (!slot.prepared) return RetiredTransportDrainResult::kProviderFailure;
      if (slot.operation) {
        slot.pending = true;
        ++slot.request_generation;
        return RetiredTransportDrainResult::kDeferred;
      }
      if (slot.current == nullptr || !slot.active) {
        if (slot.continuation_scheduled) {
          slot.pending = true;
          ++slot.request_generation;
          return RetiredTransportDrainResult::kDeferred;
        }
        if (!slot.transport->HasPendingTx()) {
          slot.pending = false;
          return RetiredTransportDrainResult::kApplied;
        }
        activate = true;
      } else {
        slot.operation = true;
        slot.pending = false;
        slot.failure_notified = false;
        registration = slot.current;
        slot.operation_generation = slot.request_generation;
        drain = true;
      }
    }
    if (drain) return DrainOnce(control, registration, false);
    if (activate) {
      const auto activated = ActivateControl(control);
      if (activated != RetiredTransportDrainResult::kApplied)
        return activated;
      // Activation may have published a pending OUTPUT registration. Loop to
      // perform the actual flush outside the mutex.
    }
  }
}

RetiredTransportDrainResult RetiredTransportDrain::Retire() {
  auto control = control_;
  Registration* registration = nullptr;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    slot.closed = true;
    slot.pending = false;
    ++slot.revision;
    if (slot.operation) return RetiredTransportDrainResult::kDeferred;
    registration = slot.current;
    if (registration == nullptr) {
      slot.active = false;
      return RetiredTransportDrainResult::kApplied;
    }
    slot.operation = true;
  }
  return RemoveRegistration(control, registration);
}

RetiredTransportDrainStatus RetiredTransportDrain::Status() const {
  std::lock_guard<std::mutex> lock(control_->mutex);
  const auto& slot = control_->slot;
  return RetiredTransportDrainStatus{slot.prepared,
                                     slot.active && !slot.closed,
                                     slot.pending,
                                     slot.closed,
                                     slot.terminal,
                                     slot.revision};
}

RetiredTransportDrainResult RetiredTransportDrain::DrainOnce(
    const std::shared_ptr<Control>& control, Registration* registration,
    bool from_callback) {
  std::shared_ptr<InputTransport> transport;
  std::uint64_t operation_generation = 0;
  RetiredTransportDrainResult early_result =
      RetiredTransportDrainResult::kProviderFailure;
  bool remove_early = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.current != registration) {
      slot.operation = false;
      early_result = slot.terminal ? RetiredTransportDrainResult::kTerminal
                                   : RetiredTransportDrainResult::kProviderFailure;
    } else if (slot.closed || slot.terminal) {
      slot.active = false;
      slot.pending = false;
      slot.operation = true;
      remove_early = true;
      early_result = slot.terminal ? RetiredTransportDrainResult::kTerminal
                                   : RetiredTransportDrainResult::kProviderFailure;
    } else {
      transport = slot.transport;
      operation_generation = slot.operation_generation;
    }
  }
  if (remove_early) {
    if (from_callback) {
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        if (control->slot.current == registration)
          control->slot.callback_remove_pending = true;
      }
      return early_result;
    }
    const auto removed = RemoveRegistration(control, registration);
    return removed == RetiredTransportDrainResult::kApplied ? early_result
                                                             : removed;
  }
  if (transport == nullptr) return early_result;
  const auto flushed = FlushInputTransport(transport.get());
  const bool progress_ok = NotifyProgress(control, flushed);
  const bool pending = transport->HasPendingTx();
  bool remove = false;
  const bool progress_failed = !progress_ok;
  bool deferred_terminal_notification = false;
  RetiredTransportDrainResult result =
      flushed == InputTransportStatus::kTerminal
          ? RetiredTransportDrainResult::kTerminal
          : flushed == InputTransportStatus::kBackpressured
                ? RetiredTransportDrainResult::kDeferred
                : RetiredTransportDrainResult::kApplied;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (!progress_ok) {
      slot.closed = true;
      slot.active = false;
      slot.pending = false;
      remove = true;
      result = RetiredTransportDrainResult::kProviderFailure;
    } else if (flushed == InputTransportStatus::kTerminal) {
      slot.terminal = true;
      slot.active = false;
      slot.pending = false;
      // The flush result itself was already reported as terminal above.
      deferred_terminal_notification = false;
      slot.terminal_event_pending = false;
      remove = true;
      result = RetiredTransportDrainResult::kTerminal;
    } else if (slot.closed) {
      slot.active = false;
      slot.pending = false;
      remove = true;
      result = RetiredTransportDrainResult::kProviderFailure;
    } else if (slot.terminal) {
      slot.active = false;
      slot.pending = false;
      deferred_terminal_notification = slot.terminal_event_pending;
      slot.terminal_event_pending = false;
      remove = true;
      result = RetiredTransportDrainResult::kTerminal;
    } else if (pending ||
               slot.request_generation != operation_generation) {
      slot.active = true;
      slot.pending = true;
      slot.operation = false;
      return RetiredTransportDrainResult::kDeferred;
    } else {
      slot.active = false;
      slot.pending = false;
      remove = true;
      result = RetiredTransportDrainResult::kApplied;
    }
  }
  bool terminal_notification_failed = false;
  if (deferred_terminal_notification)
    terminal_notification_failed =
        !NotifyProgress(control, InputTransportStatus::kTerminal);
  const bool report_failure = progress_failed || terminal_notification_failed;
  if (terminal_notification_failed) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->slot.closed = true;
    control->slot.active = false;
    control->slot.pending = false;
    remove = true;
    result = RetiredTransportDrainResult::kProviderFailure;
  }
  if (from_callback) {
    // ALooper removes the exact generation after a zero callback result. Keep
    // the registration pointer until OwnerRelease clears current, while
    // allowing a concurrent Retire to claim removal if the provider fails to
    // remove the callback after it returns.
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (control->slot.current == registration)
        control->slot.callback_remove_pending = true;
    }
    if (report_failure) {
      // The looper will remove the exact registration after this callback;
      // report the policy failure now, after the state was closed and without
      // invoking user code under the drain mutex.
      NotifyFailure(control, RetiredTransportDrainResult::kProviderFailure,
                    true);
      return result;
    }
    return remove ? result : RetiredTransportDrainResult::kDeferred;
  }
  const auto removed = RemoveRegistration(control, registration);
  if (removed != RetiredTransportDrainResult::kApplied) {
    if (report_failure)
      NotifyFailure(control, RetiredTransportDrainResult::kProviderFailure,
                    true);
    return removed;
  }
  if (report_failure)
    NotifyFailure(control, RetiredTransportDrainResult::kProviderFailure,
                  true);
  return result;
}

RetiredTransportDrainResult RetiredTransportDrain::RemoveRegistration(
    const std::shared_ptr<Control>& control, Registration* registration) {
  void* looper = nullptr;
  int fd = -1;
  bool schedule_after_release = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.current != registration) {
      if (slot.current != nullptr)
        return RetiredTransportDrainResult::kDeferred;
      slot.operation = false;
      const bool retry = !slot.closed && !slot.terminal &&
                         slot.request_generation != slot.operation_generation;
      schedule_after_release = retry;
      if (!retry) return RetiredTransportDrainResult::kApplied;
    } else {
      looper = slot.looper;
      fd = slot.fd;
    }
  }
  // A provider can have completed the exact removal and released the owner
  // before this call observes it. In that case the generation still needs a
  // looper turn; do not make the caller rediscover the retained request.
  if (schedule_after_release) {
    ScheduleContinuation(control);
    return RetiredTransportDrainResult::kDeferred;
  }
  const int removed = darwin_art::looper::RemoveFdIfOwned(
      looper, fd, &RetiredTransportDrain::OnFd, registration);
  if (removed < 0) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->slot.operation = false;
    control->slot.closed = true;
    ++control->slot.revision;
    return RetiredTransportDrainResult::kProviderFailure;
  }
  bool retry = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.current == registration) slot.current = nullptr;
    slot.active = false;
    slot.operation = false;
    retry = !slot.closed && !slot.terminal &&
            slot.request_generation != slot.operation_generation;
    ++slot.revision;
  }
  if (retry) ScheduleContinuation(control);
  return retry ? RetiredTransportDrainResult::kDeferred
               : RetiredTransportDrainResult::kApplied;
}

int RetiredTransportDrain::OnFd(int fd, int events, void* data) {
  auto* registration = static_cast<Registration*>(data);
  if (registration == nullptr) return 0;
  const auto control = registration->control;
  if (control == nullptr) return 0;
  bool terminal_event = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.current != registration || slot.fd != fd || slot.closed ||
        slot.terminal)
      return 0;
    terminal_event = (events & (kErrorEvent | kHangupEvent | kInvalidEvent)) != 0;
    if (terminal_event) {
      slot.terminal = true;
      slot.active = false;
      slot.pending = false;
      if (slot.operation) {
        // An existing Request/OnFd owns the operation and will perform exact
        // removal after its out-of-lock transport work completes.
        slot.terminal_event_pending = true;
        ++slot.revision;
        return 1;
      }
      slot.callback_remove_pending = true;
      ++slot.revision;
    } else if ((events & kOutputEvent) == 0) {
      // A spurious event must not silently unregister the only driver for
      // retained TX. Keep the exact registration alive until OUTPUT or a
      // terminal poll result is observed.
      return 1;
    } else {
      if (slot.operation) return 1;
      slot.operation = true;
      slot.failure_notified = false;
      slot.operation_generation = slot.request_generation;
    }
  }
  if (terminal_event) {
    const bool progress_ok =
        NotifyProgress(control, InputTransportStatus::kTerminal);
    if (!progress_ok) {
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        control->slot.closed = true;
        control->slot.active = false;
        control->slot.pending = false;
      }
      NotifyFailure(control, RetiredTransportDrainResult::kProviderFailure,
                    true);
    }
    return 0;
  }
  const auto result = DrainOnce(control, registration, true);
  if (result == RetiredTransportDrainResult::kDeferred) return 1;
  return 0;
}

void RetiredTransportDrain::Release(void* data) {
  auto* registration = static_cast<Registration*>(data);
  if (registration == nullptr) return;
  const auto control = registration->control;
  bool schedule_continuation = false;
  if (control != nullptr) {
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& slot = control->slot;
      if (slot.current == registration) {
        slot.current = nullptr;
        slot.active = false;
        if (slot.callback_remove_pending) {
          slot.callback_remove_pending = false;
          slot.operation = false;
        }
        schedule_continuation = !slot.closed && !slot.terminal &&
                                slot.request_generation !=
                                    slot.operation_generation;
        ++slot.revision;
      }
    }
  }
  delete registration;
  if (control != nullptr) (void)DeliverPendingTerminal(control);
  if (control != nullptr && schedule_continuation)
    ScheduleContinuation(control);
}

void RetiredTransportDrain::ScheduleContinuation(
    const std::shared_ptr<Control>& control) {
  void* looper = nullptr;
  std::uint64_t generation = 0;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (!slot.prepared || slot.closed || slot.terminal || slot.operation ||
        slot.current != nullptr || slot.continuation_scheduled ||
        (slot.request_generation == slot.operation_generation &&
         !slot.pending))
      return;
    looper = slot.looper;
    generation = slot.request_generation;
    slot.continuation_scheduled = true;
    slot.continuation_generation = generation;
    ++slot.revision;
  }

  auto* task = new (std::nothrow) Continuation;
  if (task == nullptr) {
    bool report = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& slot = control->slot;
      if (slot.continuation_scheduled &&
          slot.continuation_generation == generation) {
        slot.continuation_scheduled = false;
        ++slot.revision;
        report = !slot.closed && !slot.terminal;
      }
    }
    if (report) NotifyFailure(control, RetiredTransportDrainResult::kOutOfMemory);
    return;
  }
  task->control = control;
  task->generation = generation;
  int scheduled = 0;
  try {
    // The looper consumes the task on every rejected enqueue, including a
    // provider exception after ownership transfer. Never release it here.
    scheduled = darwin_art::looper::ScheduleTimedTaskAt(
        looper, 0, &RetiredTransportDrain::RunContinuation, task,
        &RetiredTransportDrain::ReleaseContinuation);
  } catch (const std::bad_alloc&) {
    scheduled = -2;
  } catch (...) {
    scheduled = 0;
  }
  if (scheduled == 1) return;

  bool report = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.continuation_scheduled &&
        slot.continuation_generation == generation) {
      slot.continuation_scheduled = false;
      ++slot.revision;
      report = !slot.closed && !slot.terminal;
    }
  }
  if (report)
    NotifyFailure(control, scheduled == -2
                              ? RetiredTransportDrainResult::kOutOfMemory
                              : RetiredTransportDrainResult::kProviderFailure);
}

void RetiredTransportDrain::RunContinuation(void* data) {
  auto* task = static_cast<Continuation*>(data);
  if (task == nullptr || task->control == nullptr) return;
  const auto control = task->control;
  bool run = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (slot.continuation_scheduled &&
        slot.continuation_generation == task->generation) {
      slot.continuation_scheduled = false;
      ++slot.revision;
      run = !slot.closed && !slot.terminal;
    }
  }
  if (!run) {
    (void)DeliverPendingTerminal(control);
    return;
  }
  const auto result = RequestControl(control);
  (void)DeliverPendingTerminal(control);
  if (result == RetiredTransportDrainResult::kOutOfMemory ||
      result == RetiredTransportDrainResult::kProviderFailure)
    NotifyFailure(control, result, true);
}

void RetiredTransportDrain::ReleaseContinuation(void* data) {
  delete static_cast<Continuation*>(data);
}

void RetiredTransportDrain::NotifyFailure(
    const std::shared_ptr<Control>& control,
    RetiredTransportDrainResult result, bool include_closed) {
  RetiredTransportDrainCallbacks callbacks;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    if (!include_closed && (slot.closed || slot.terminal)) return;
    if (slot.failure_notified) return;
    slot.failure_notified = true;
    callbacks = slot.callbacks;
  }
  if (callbacks.on_failure == nullptr) return;
  try {
    callbacks.on_failure(callbacks.context, result);
  } catch (...) {
    // Failure observation is advisory and must not escape the owner looper.
  }
}

bool RetiredTransportDrain::DeliverPendingTerminal(
    const std::shared_ptr<Control>& control) {
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& slot = control->slot;
    // Callback-side removal releases its reservation before reaching here.
    // A still-active operation owns both cleanup and terminal notification.
    if (slot.operation || !slot.terminal_event_pending) return true;
    slot.terminal_event_pending = false;
    ++slot.revision;
  }
  if (NotifyProgress(control, InputTransportStatus::kTerminal)) return true;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->slot.closed = true;
    control->slot.active = false;
    control->slot.pending = false;
    ++control->slot.revision;
  }
  NotifyFailure(control, RetiredTransportDrainResult::kProviderFailure, true);
  return false;
}

bool RetiredTransportDrain::NotifyProgress(
    const std::shared_ptr<Control>& control, InputTransportStatus status) {
  RetiredTransportDrainCallbacks callbacks;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    callbacks = control->slot.callbacks;
  }
  if (callbacks.on_progress == nullptr) return true;
  try {
    callbacks.on_progress(callbacks.context, status);
  } catch (...) {
    return false;
  }
  return true;
}

}  // namespace darwin_art::input
