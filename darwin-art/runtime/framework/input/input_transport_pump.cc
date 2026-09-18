#include "input_transport_pump.h"
#include "input_transport_readiness.h"

#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::input {

struct InputTransportPumpLease::Control {
  mutable std::mutex mutex;
  void* looper = nullptr;
  int fd = -1;
  int events = 0;
  int base_events = 0;
  std::shared_ptr<InputTransport> transport;
  InputTransportPumpCallbacks callbacks;
  InputTransportReaderCallback reader_callback = nullptr;
  void* reader_context = nullptr;
  std::shared_ptr<void> reader_owner;
  darwin_art::looper::FdCallback callback = nullptr;
  void* callback_context = nullptr;
  std::shared_ptr<void> callback_owner;
  Registration* current = nullptr;
  Registration* operation_registration = nullptr;
  int desired_events = 0;
  bool operation = false;
  bool closed = false;
  bool retiring = false;
  bool refresh_requested = false;
  bool cleanup_pending = false;
  bool terminal_reported = false;
  bool reader_completed = false;
  unsigned active_callbacks = 0;
  bool quiescence_reported = false;
};

struct InputTransportPumpLease::Registration {
  std::shared_ptr<Control> control;
  void* looper = nullptr;
  int fd = -1;
  int events = 0;
  std::shared_ptr<InputTransport> transport;
  InputTransportPumpCallbacks callbacks;
  InputTransportReaderCallback reader_callback = nullptr;
  void* reader_context = nullptr;
  std::shared_ptr<void> reader_owner;
  darwin_art::looper::FdCallback callback = nullptr;
  void* callback_context = nullptr;
  std::shared_ptr<void> callback_owner;
};

namespace {

constexpr int kInputEvent = 0x0001;
constexpr int kOutputEvent = 0x0002;

// Caller holds Control::mutex. Queued requests predate policy callbacks and
// provider reentry; never let their old complete masks resurrect a direction.
int NormalizeDirectionalInterests(
    const InputTransportPumpLease::Control& control, int events) {
  if (control.transport->IsTxTerminal()) events &= ~kOutputEvent;
  if (control.reader_completed) {
    events &= ~kInputEvent;
    const auto output = control.transport->OutputSnapshot();
    if (output.pending && !output.terminal &&
        output.endpoint_fd == control.fd) {
      // Reader completion removes INPUT, but a healthy accepted TX suffix
      // still requires writable turns even if policy disabled OUTPUT earlier.
      events |= kOutputEvent;
    }
  }
  return events;
}

bool RefreshAfterProviderReentry(InputTransportPumpLease::Control& control,
                                 int installed_events) {
  control.desired_events = NormalizeDirectionalInterests(
      control, control.refresh_requested ? control.desired_events : installed_events);
  control.refresh_requested = control.desired_events != installed_events;
  return control.refresh_requested;
}

void ReleaseRetiredResources(
    const std::shared_ptr<InputTransportPumpLease::Control>& control) {
  std::shared_ptr<InputTransport> transport;
  InputTransportPumpCallbacks callbacks;
  std::shared_ptr<void> reader_owner;
  std::shared_ptr<void> callback_owner;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->closed || control->current != nullptr || control->operation ||
        control->operation_registration != nullptr || control->active_callbacks != 0)
      return;
    transport = std::move(control->transport);
    callbacks = std::move(control->callbacks);
    control->callbacks = {};
    reader_owner = std::move(control->reader_owner);
    control->reader_callback = nullptr;
    control->reader_context = nullptr;
    callback_owner = std::move(control->callback_owner);
    control->callback = nullptr;
    control->callback_context = nullptr;
  }
  // Arbitrary supplied owner destructors run outside the lifecycle mutex.
}

void PublishPumpQuiescence(
    const std::shared_ptr<InputTransportPumpLease::Control>& control) {
  InputTransportPumpCallbacks callbacks;
  std::shared_ptr<void> callback_owner;
  {
    std::lock_guard lock(control->mutex);
    if (!control->closed || control->current != nullptr || control->operation ||
        control->operation_registration != nullptr || control->active_callbacks != 0 ||
        control->quiescence_reported)
      return;
    control->quiescence_reported = true;
    callbacks = control->callbacks;
    callback_owner = control->callback_owner;
  }
  if (callbacks.on_quiescent != nullptr)
    callbacks.on_quiescent(callbacks.context);
  ReleaseRetiredResources(control);
}

struct CallbackCompletion {
  std::shared_ptr<InputTransportPumpLease::Control> control;
  ~CallbackCompletion() {
    {
      std::lock_guard lock(control->mutex);
      --control->active_callbacks;
    }
    // All old FD/input/JNI/terminal work has ended. Only metadata publication
    // and resource-pin destruction follow; never touch Registration here.
    PublishPumpQuiescence(control);
  }
};

struct OperationCompletion {
  std::shared_ptr<InputTransportPumpLease::Control> control;
  ~OperationCompletion() { PublishPumpQuiescence(control); }
};

bool RemoveExact(InputTransportPumpLease::Control* control,
                 InputTransportPumpLease::Registration* registration) {
  if (control == nullptr || registration == nullptr) return false;
  return darwin_art::looper::RemoveFdIfOwned(
             control->looper, control->fd, &InputTransportPumpLease::OnFd,
             registration) >= 0;
}

bool FinishRearm(const std::shared_ptr<InputTransportPumpLease::Control>& control,
                 InputTransportPumpLease::Registration* observed,
                 int observed_events, int desired_events) {
  for (;;) {
    InputTransportPumpLease::Registration* old = nullptr;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (control->closed || control->retiring) {
        old = control->current;
        if (old == nullptr) {
          control->operation = false;
          control->operation_registration = nullptr;
          return true;
        }
      } else {
        const int requested = NormalizeDirectionalInterests(
            *control, control->refresh_requested ? control->desired_events : desired_events);
        control->refresh_requested = false;
        if (control->current == observed && requested == observed_events) {
          control->operation = false;
          control->operation_registration = nullptr;
          return true;
        }
        old = control->current;
        if (old == nullptr) {
          control->operation = false;
          control->operation_registration = nullptr;
          return false;
        }
        desired_events = requested;
      }
      control->operation_registration = old;
    }
    if (!RemoveExact(control.get(), old)) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
      control->cleanup_pending = true;
      control->operation = false;
      control->operation_registration = nullptr;
      return false;
    }
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (control->current == old) control->current = nullptr;
      if (control->closed || control->retiring) {
        control->operation = false;
        control->operation_registration = nullptr;
        return true;
      }
    }
    auto* next = new (std::nothrow) InputTransportPumpLease::Registration;
    if (next == nullptr) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
      control->cleanup_pending = true;
      control->operation = false;
      control->operation_registration = nullptr;
      return false;
    }
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      next->control = control;
      next->looper = control->looper;
      next->fd = control->fd;
      next->events = NormalizeDirectionalInterests(*control, desired_events);
      next->transport = control->transport;
      next->callbacks = control->callbacks;
      next->reader_callback = control->reader_callback;
      next->reader_context = control->reader_context;
      next->reader_owner = control->reader_owner;
      next->callback = control->callback;
      next->callback_context = control->callback_context;
      next->callback_owner = control->callback_owner;
      control->operation_registration = next;
    }
    // AddFdOwned consumes its owner even if shared_ptr/vector allocation throws.
    int added = -1;
    try {
      added = darwin_art::looper::AddFdOwned(
          next->looper, next->fd, 0, next->events,
          &InputTransportPumpLease::OnFd, next, next,
          &InputTransportPumpLease::Release);
    } catch (...) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
      control->cleanup_pending = true;
      control->operation = false;
      control->operation_registration = nullptr;
      return false;
    }
    if (added < 0) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
      control->cleanup_pending = true;
      control->operation = false;
      control->operation_registration = nullptr;
      return false;
    }
    bool remove_next = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (control->closed || control->retiring) {
        control->current = next;
        control->operation = true;
        remove_next = true;
      } else {
        control->current = next;
        control->events = next->events;
        // Keep the operation reservation across a coalesced refresh so a
        // concurrent Retire/SetWritable cannot race the next exact remove.
        control->operation = RefreshAfterProviderReentry(*control, next->events);
        control->operation_registration = nullptr;
        observed = next;
        observed_events = next->events;
        if (!control->refresh_requested) return true;
      }
    }
    if (remove_next) {
      if (!RemoveExact(control.get(), next)) {
        std::lock_guard<std::mutex> lock(control->mutex);
        control->cleanup_pending = true;
        control->operation = false;
        control->operation_registration = nullptr;
        return false;
      }
      std::lock_guard<std::mutex> lock(control->mutex);
      if (control->current == next) control->current = nullptr;
      control->operation = false;
      control->operation_registration = nullptr;
      return true;
    }
  }
}

}  // namespace

InputTransportStatus PumpInputTransport(
    InputTransport* transport, int fd,
    const InputTransportPumpCallbacks& callbacks) {
  return transport == nullptr ? InputTransportStatus::kTerminal
                              : transport->PumpInternal(fd, callbacks);
}

InputTransportPumpLease::~InputTransportPumpLease() { (void)Retire(); }

bool InputTransportPumpLease::Register(
    void* looper, std::shared_ptr<InputTransport> transport, int fd, int events,
    const InputTransportPumpCallbacks& callbacks,
    darwin_art::looper::FdCallback callback, void* callback_context,
    std::shared_ptr<void> callback_owner) {
  return RegisterInternal(looper, std::move(transport), fd, events, callbacks,
                          nullptr, nullptr, {}, callback, callback_context,
                          std::move(callback_owner));
}

bool InputTransportPumpLease::RegisterReader(
    void* looper, std::shared_ptr<InputTransport> transport, int fd, int events,
    const InputTransportPumpCallbacks& callbacks,
    InputTransportReaderCallback reader_callback, void* reader_context,
    std::shared_ptr<void> reader_owner) {
  if (reader_callback == nullptr || (events & kInputEvent) == 0) return false;
  return RegisterInternal(looper, std::move(transport), fd, events, callbacks,
                          reader_callback, reader_context,
                          std::move(reader_owner), nullptr, nullptr, {});
}

bool InputTransportPumpLease::RegisterInternal(
    void* looper, std::shared_ptr<InputTransport> transport, int fd, int events,
    const InputTransportPumpCallbacks& callbacks,
    InputTransportReaderCallback reader_callback, void* reader_context,
    std::shared_ptr<void> reader_owner,
    darwin_art::looper::FdCallback callback, void* callback_context,
    std::shared_ptr<void> callback_owner) {
  if (control_ != nullptr || looper == nullptr || transport == nullptr || fd < 0)
    return false;
  if ((callbacks.on_packet != nullptr && callbacks.on_packet_consumption != nullptr) ||
      (callbacks.on_window != nullptr && callbacks.on_window_consumption != nullptr))
    return false;
  // An output-only owner must watch the same immutable framed stream it
  // drains, not a local wake or replacement RX descriptor.
  if ((events & kInputEvent) == 0 && !transport->BindOutputEndpoint(fd))
    return false;
  if (transport->IsTxTerminal()) events &= ~kOutputEvent;
  std::shared_ptr<Control> control;
  try {
    control = std::make_shared<Control>();
  } catch (const std::bad_alloc&) {
    return false;
  }
  control_ = control;
  OperationCompletion completion{control};
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->looper = looper;
    control->fd = fd;
    control->events = events;
    control->base_events = events & ~kOutputEvent;
    control->desired_events = events;
    control->transport = std::move(transport);
    control->callbacks = callbacks;
    control->reader_callback = reader_callback;
    control->reader_context = reader_context;
    control->reader_owner = std::move(reader_owner);
    control->callback = callback;
    control->callback_context = callback_context;
    control->callback_owner = std::move(callback_owner);
    control->operation = true;
  }
  auto* registration = new (std::nothrow) Registration;
  if (registration == nullptr) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
    control->operation = false;
    return false;
  }
  registration->control = control;
  registration->looper = looper;
  registration->fd = fd;
  registration->events = events;
  registration->transport = control->transport;
  registration->callbacks = callbacks;
  registration->reader_callback = reader_callback;
  registration->reader_context = reader_context;
  registration->reader_owner = control->reader_owner;
  registration->callback = callback;
  registration->callback_context = callback_context;
  registration->callback_owner = control->callback_owner;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->operation_registration = registration;
  }
  // The provider consumes this owner on success, rejection and exception.
  int added = -1;
  try {
    added = darwin_art::looper::AddFdOwned(
        looper, fd, 0, events, &OnFd, registration, registration, &Release);
  } catch (...) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
    control->cleanup_pending = true;
    control->operation = false;
    control->operation_registration = nullptr;
    return false;
  }
  if (added < 0) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
    control->operation = false;
    control->operation_registration = nullptr;
    return false;
  }
  bool remove = false;
  bool refresh = false;
  int refreshed_events = events;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->closed || control->retiring) {
      remove = true;
      control->current = registration;
    } else {
      control->current = registration;
      control->events = registration->events;
      refresh = RefreshAfterProviderReentry(*control, registration->events);
      refreshed_events = control->desired_events;
      control->operation = refresh;
      control->operation_registration = nullptr;
      if (!refresh) return true;
    }
  }
  if (refresh && !remove)
    return FinishRearm(control, registration, events, refreshed_events);
  if (!RemoveExact(control.get(), registration)) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
    control->cleanup_pending = true;
    control->operation = false;
    control->operation_registration = registration;
    return false;
  }
  if (remove) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->current = nullptr;
    control->operation = false;
    control->operation_registration = nullptr;
    return false;
  }
  return false;
}

bool InputTransportPumpLease::Retire() {
  const auto control = control_;
  if (control == nullptr) return true;
  Registration* registration = nullptr;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
    if (control->operation) {
      control->cleanup_pending = true;
      return true;
    }
    registration = control->current;
    if (registration == nullptr) {
      control->operation_registration = nullptr;
    } else {
      control->operation = true;
      control->operation_registration = registration;
    }
  }
  if (registration == nullptr) {
    PublishPumpQuiescence(control);
    return true;
  }
  if (!RemoveExact(control.get(), registration)) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->cleanup_pending = true;
    control->operation = false;
    control->operation_registration = registration;
    return false;
  }
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->current == registration) control->current = nullptr;
    control->operation = false;
    control->operation_registration = nullptr;
  }
  PublishPumpQuiescence(control);
  return true;
}

bool InputTransportPumpLease::SetWritable(bool enabled) {
  return SetWritableResult(enabled) == InputTransportWritableResult::kApplied;
}

InputTransportWritableResult InputTransportPumpLease::SetWritableResult(
    bool enabled) {
  const auto control = control_;
  if (control == nullptr) return InputTransportWritableResult::kTerminal;
  OperationCompletion completion{control};
  int desired;
  Registration* current = nullptr;
  int current_events = 0;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->closed || control->retiring)
      return InputTransportWritableResult::kTerminal;
    if (enabled) {
      const auto output = control->transport->OutputSnapshot();
      if (output.endpoint_fd >= 0 && output.endpoint_fd != control->fd)
        return InputTransportWritableResult::kTerminal;
      if (output.terminal) {
        if ((control->base_events & kInputEvent) == 0)
          return InputTransportWritableResult::kTerminal;
        enabled = false; // Failed TX must not retire a healthy reader.
      }
    }
    desired = control->base_events | (enabled ? kOutputEvent : 0);
    if (control->events == desired && !control->operation)
      return InputTransportWritableResult::kApplied;
    control->desired_events = desired;
    if (control->operation) {
      control->refresh_requested = true;
      return InputTransportWritableResult::kDeferred;
    }
    if (control->current == nullptr) {
      control->refresh_requested = true;
      return InputTransportWritableResult::kDeferred;
    }
    control->operation = true;
    current = control->current;
    current_events = control->events;
  }
  const bool finished =
      FinishRearm(control, current, current_events, desired);
  std::lock_guard<std::mutex> lock(control->mutex);
  if (control->closed || control->retiring)
    return InputTransportWritableResult::kTerminal;
  return finished ? InputTransportWritableResult::kApplied
                  : InputTransportWritableResult::kDeferred;
}

bool InputTransportPumpLease::IsQuiescent() const {
  const auto control = control_;
  if (control == nullptr) return true;
  std::lock_guard<std::mutex> lock(control->mutex);
  return control->closed && control->current == nullptr && !control->operation &&
         control->operation_registration == nullptr && control->active_callbacks == 0;
}

int InputTransportPumpLease::OnFd(int fd, int events, void* data) {
  auto* registration = static_cast<Registration*>(data);
  if (registration == nullptr || registration->control == nullptr) return 0;
  const auto control = registration->control;
  int base_events = 0;
  bool reader_completed = false;
  InputTransportReadiness readiness;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->closed || control->retiring)
      return 0;
    if (control->operation && control->operation_registration == registration)
      return 1;  // Add may dispatch before publication; do not unregister it.
    if (control->current != registration) return 0;
    // A provider callback can arrive while a rearm operation owns the same
    // registration. Keep the live looper entry; the operation will finish it.
    if (control->operation) return 1;
    control->operation = true;
    control->operation_registration = registration;
    readiness = InputTransportReadiness{
        .transport = registration->transport,
        .callbacks = registration->callbacks,
        .reader_callback = registration->reader_callback,
        .reader_context = registration->reader_context,
        .reader_owner = registration->reader_owner,
        .callback = registration->callback,
        .callback_context = registration->callback_context,
        .callback_owner = registration->callback_owner};
    base_events = control->base_events;
    reader_completed = control->reader_completed;
    ++control->active_callbacks;
  }
  const auto& transport = readiness.transport;
  const auto& callbacks = readiness.callbacks;
  const auto reader_callback = readiness.reader_callback;
  // Pins outlive CallbackCompletion, including its quiescence notification.
  CallbackCompletion completion{control};
  try {
    const auto result = ServiceInputTransportReadiness(
        readiness, fd, events, (base_events & kInputEvent) != 0,
        reader_completed);
    if (result.reader_complete_requested) {
      bool accepted = false;
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        // Service requests completion; only this exact resource owner can
        // remove reader authority from the still-current registration.
        if (!control->closed && !control->retiring &&
            control->current == registration &&
            (control->base_events & kInputEvent) != 0 &&
            fd == transport->RemoteEndpointFd() && transport->IsRxTerminal()) {
          base_events &= ~kInputEvent;
          reader_completed = true;
          control->base_events = base_events;
          control->reader_completed = true;
          accepted = true;
        }
      }
      if (!accepted && reader_callback != nullptr) {
        std::lock_guard<std::mutex> lock(control->mutex);
        control->closed = true;
        control->retiring = true;
      }
    }
    if (result.retire) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
    }
    // Default progress observes committed RX completion and may enqueue TX.
    // Keep its invocation and the fresh drain predicate in lifecycle order.
    if (result.default_progress.has_value()) {
      if (callbacks.on_progress != nullptr)
        callbacks.on_progress(callbacks.context, *result.default_progress);
      if ((base_events & kInputEvent) == 0 &&
          (transport->IsTxTerminal() ||
           (reader_completed && !transport->HasPendingTx()))) {
        std::lock_guard<std::mutex> lock(control->mutex);
        control->closed = true;
        control->retiring = true;
      }
    }
    if ((base_events & kInputEvent) == 0 &&
        (reader_completed || reader_callback != nullptr) &&
        (transport->IsTxTerminal() || !transport->HasPendingTx())) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
    }
    // A first completion callback still carried INPUT on admission. Fatal
    // provider readiness must win after final authorized RX work as well,
    // not only on a later private writable turn. A failed local wake slot
    // has no authority to terminate another descriptor's framed TX stream.
    if ((events & (0x0004 | 0x0010)) != 0) {
      if (transport->OutputSnapshot().endpoint_fd == fd)
        TerminateInputTransportTx(transport.get());
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->retiring = true;
    }
  } catch (...) {
    // Do not unwind through the C Looper boundary or strand the operation
    // reservation. Exact removal below owns cleanup even after policy throws.
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
  }
  const auto output = transport->OutputSnapshot();
  const int desired = base_events |
      (output.pending && !output.terminal && output.endpoint_fd == fd ? kOutputEvent : 0);
  // Registration may be released by exact removal; snapshot before completion.
  const int observed_events = registration->events;
  bool finished = false;
  try {
    finished = FinishRearm(control, registration, observed_events, desired);
  } catch (...) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->closed = true;
    control->retiring = true;
    control->cleanup_pending = true;
    control->operation = false;
    control->operation_registration = nullptr;
  }
  bool keep = false;
  bool report = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    keep = finished && !control->closed && control->current != nullptr;
    if (!keep && !control->terminal_reported) {
      control->terminal_reported = true;
      report = true;
    }
  }
  if (report && callbacks.on_terminal != nullptr)
    callbacks.on_terminal(callbacks.context);
  return keep ? 1 : 0;
}

void InputTransportPumpLease::Release(void* data) {
  delete static_cast<Registration*>(data);
}

}  // namespace darwin_art::input
