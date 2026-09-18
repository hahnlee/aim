#include "receiver_endpoint_binding.h"

#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::input {

struct ReceiverEndpointBinding::Control {
  struct Token {
    const ReceiverId receiver_id;
    const std::uint64_t generation;
  };

  struct Slot {
    std::shared_ptr<ClaimedInputTransportPump> lease;
    std::shared_ptr<void> callback_context;
    int fd = -1;
    bool registered = false;
    bool terminal = false;
    bool reader_completed = false;
    bool operation = false;
    bool refresh_pending = false;
    bool desired_writable = false;
    bool callback_active = false;
  };

  mutable std::mutex mutex;
  void* looper = nullptr;
  std::shared_ptr<InputTransport> transport;
  ReceiverEndpointBindingCallbacks callbacks;
  std::shared_ptr<const Token> token;
  Slot local;
  Slot remote;
  bool registered_once = false;
  bool activated_once = false;
  bool closed = false;
  bool notifying = false;
  bool status_pending = false;
  bool retirement_notified = false;
  void (*retirement_notify)(void*) noexcept = nullptr;
  std::weak_ptr<void> retirement_context;
  std::uint64_t revision = 0;
};

struct ReceiverEndpointBinding::SlotContext {
  std::weak_ptr<Control> control;
  ReceiverEndpointBindingSlot slot;
  int fd;
  std::shared_ptr<const Control::Token> token;
};

namespace {

ReceiverEndpointBindingStatus SnapshotLocked(
    const ReceiverEndpointBinding::Control& control) {
  return ReceiverEndpointBindingStatus{
      .local_ready = !control.closed && control.local.registered &&
                     !control.local.terminal && !control.local.reader_completed,
      .remote_ready = !control.closed && control.remote.registered &&
                      !control.remote.terminal && !control.remote.reader_completed,
      .closed = control.closed,
      .revision = control.revision};
}

ReceiverEndpointBinding::Control::Slot& SlotFor(
    ReceiverEndpointBinding::Control& control,
    ReceiverEndpointBindingSlot slot) {
  return slot == ReceiverEndpointBindingSlot::kLocalWake ? control.local
                                                           : control.remote;
}

void ReportCallbackFailure(
    const ReceiverEndpointBindingCallbacks& callbacks) noexcept {
  if (callbacks.on_failure == nullptr) return;
  try {
    callbacks.on_failure(callbacks.context);
  } catch (...) {
    // A failure boundary must not let a user callback escape into the looper
    // or JNI provider, even if the supplied noexcept callback is malformed.
  }
}

void NotifyRetired(const std::shared_ptr<ReceiverEndpointBinding::Control>& control) noexcept {
  std::shared_ptr<ClaimedInputTransportPump> local;
  std::shared_ptr<ClaimedInputTransportPump> remote;
  ReceiverEndpointBindingCallbacks callbacks;
  void (*retirement_notify)(void*) noexcept = nullptr;
  std::shared_ptr<void> retirement_owner;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->closed || control->retirement_notified ||
        control->local.operation || control->remote.operation ||
        control->local.callback_active || control->remote.callback_active ||
        control->notifying)
      return;
    local = control->local.lease;
    remote = control->remote.lease;
    callbacks = control->callbacks;
    retirement_notify = control->retirement_notify;
    retirement_owner = control->retirement_context.lock();
  }
  if ((local != nullptr && !local->IsQuiescent()) ||
      (remote != nullptr && !remote->IsQuiescent()))
    return;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->closed || control->retirement_notified ||
        control->local.operation || control->remote.operation ||
        control->local.callback_active || control->remote.callback_active ||
        control->notifying ||
        control->local.lease != local || control->remote.lease != remote)
      return;
    control->retirement_notified = true;
    callbacks = control->callbacks;
    // A retirement subscriber may have been installed between the first
    // snapshot and this final commit. Refresh it under the same lock that
    // publishes retirement_notified, so that registration cannot lose the
    // completion edge to this notifier.
    retirement_notify = control->retirement_notify;
    retirement_owner = control->retirement_context.lock();
    // No status/event callback can follow full slot quiescence. Release the
    // caller context only after copying it, and never make a later destructor
    // re-enter a retired stack-owned callback context.
    control->callbacks = {};
  }
  if (callbacks.on_retired != nullptr) {
    try {
      callbacks.on_retired(callbacks.context);
    } catch (...) {
      // The completion callback is a non-throwing lifecycle boundary.
    }
  }
  if (retirement_notify != nullptr && retirement_owner != nullptr) {
    retirement_notify(retirement_owner.get());
  }
}

}  // namespace

ReceiverEndpointBinding::ReceiverEndpointBinding()
    : control_(std::make_shared<Control>()) {}

void ReceiverEndpointBinding::MaybeNotifyRetired(
    const std::shared_ptr<Control>& control) noexcept {
  NotifyRetired(control);
}

void ReceiverEndpointBinding::FinishActivationClose(
    const std::shared_ptr<Control>& control,
    const std::shared_ptr<ClaimedInputTransportPump>& local_lease,
    const std::shared_ptr<ClaimedInputTransportPump>& remote_lease,
    bool retire_local, bool retire_remote) {
  // Activation can be closed reentrantly from a status/provider callback. The
  // claimed wrappers remain the sole owner of provider removal and claim
  // release; only invoke them outside the binding mutex.
  const bool local_retired =
      !retire_local || local_lease == nullptr || local_lease->Retire();
  const bool remote_retired =
      !retire_remote || remote_lease == nullptr || remote_lease->Retire();
  const bool local_quiescent =
      !retire_local ||
      (local_retired && local_lease != nullptr && local_lease->IsQuiescent());
  const bool remote_quiescent =
      !retire_remote ||
      (remote_retired && remote_lease != nullptr &&
       remote_lease->IsQuiescent());
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto finish_slot = [&](Control::Slot& state, bool retire,
                           bool quiescent) {
      state.registered = false;
      state.terminal = true;
      if (!retire) return;
      state.operation = false;
      if (quiescent) {
        state.lease.reset();
        state.callback_context.reset();
      }
    };
    finish_slot(control->local, retire_local, local_quiescent);
    finish_slot(control->remote, retire_remote, remote_quiescent);
    control->closed = true;
    ++control->revision;
    control->status_pending = true;
  }
  Notify(control);
  MaybeNotifyRetired(control);
}

ReceiverEndpointBinding::~ReceiverEndpointBinding() { (void)Retire(); }

bool ReceiverEndpointBinding::Register(
    void* looper, std::shared_ptr<InputTransport> transport, int local_fd,
    int remote_fd, const ReceiverEndpointBindingCallbacks& callbacks,
    ReceiverId receiver_id, std::uint64_t generation) {
  return Prepare(looper, std::move(transport), local_fd, remote_fd, callbacks,
                 receiver_id, generation) && Activate();
}

bool ReceiverEndpointBinding::Prepare(
    void* looper, std::shared_ptr<InputTransport> transport, int local_fd,
    int remote_fd, const ReceiverEndpointBindingCallbacks& callbacks,
    ReceiverId receiver_id, std::uint64_t generation) {
  if (looper == nullptr || transport == nullptr || local_fd < 0 ||
      receiver_id == 0 ||
      darwin_art::looper::Current() != looper)
    return false;
  // A local-only endpoint is valid. If a provider reports the same descriptor
  // for both logical slots, register it once as local rather than double-owning
  // one provider registration.
  if (remote_fd == local_fd) remote_fd = -1;
  const bool has_remote = remote_fd >= 0;
  if (remote_fd < -1)
    return false;
  auto control = control_;
  std::shared_ptr<const Control::Token> token;
  std::shared_ptr<ClaimedInputTransportPump> local_lease;
  std::shared_ptr<ClaimedInputTransportPump> remote_lease;
  std::shared_ptr<void> local_context;
  std::shared_ptr<void> remote_context;
  try {
    token = std::make_shared<const Control::Token>(
        Control::Token{receiver_id, generation});
    local_context = std::make_shared<SlotContext>(
        SlotContext{control, ReceiverEndpointBindingSlot::kLocalWake,
                    local_fd, token});
    if (has_remote) {
      remote_context = std::make_shared<SlotContext>(SlotContext{
          control, ReceiverEndpointBindingSlot::kRemote, remote_fd, token});
    }
  } catch (const std::bad_alloc&) {
    return false;
  }
  const auto make_callbacks = [](const std::shared_ptr<void>& context) {
    InputTransportPumpCallbacks result;
    result.context = context.get();
    result.context_owner = context;
    result.on_terminal = &ReceiverEndpointBinding::OnSlotTerminal;
    result.on_quiescent = &ReceiverEndpointBinding::OnSlotQuiescent;
    return result;
  };
  const auto observer_for = [](const std::shared_ptr<void>& context) {
    return ClaimedPumpObserver{&ReceiverEndpointBinding::OnSlotState,
                               context.get(), context};
  };
  local_lease = ClaimedInputTransportPump::PrepareReader(
      looper, transport, local_fd, 0x0001,
      TransportRegistrationRole::kReceiver,
      receiver_id,
      make_callbacks(local_context), &ReceiverEndpointBinding::OnSlotEvent,
      local_context.get(), local_context, observer_for(local_context));
  if (has_remote) {
    remote_lease = ClaimedInputTransportPump::PrepareReader(
        looper, transport, remote_fd, 0x0001,
        TransportRegistrationRole::kReceiver,
        receiver_id,
        make_callbacks(remote_context), &ReceiverEndpointBinding::OnSlotEvent,
        remote_context.get(), remote_context, observer_for(remote_context));
  }
  if (local_lease == nullptr || (has_remote && remote_lease == nullptr)) {
    if (local_lease != nullptr) (void)local_lease->Retire();
    if (remote_lease != nullptr) (void)remote_lease->Retire();
    return false;
  }
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->registered_once || control->closed) return false;
    control->registered_once = true;
    control->looper = looper;
    control->transport = std::move(transport);
    control->callbacks = callbacks;
    control->token = token;
    control->local.fd = local_fd;
    control->remote.fd = remote_fd;
    control->local.callback_context = local_context;
    control->remote.callback_context = remote_context;
    control->local.lease = local_lease;
    control->remote.lease = remote_lease;
    control->local.operation = false;
    control->remote.operation = false;
  }
  return true;
}

bool ReceiverEndpointBinding::Activate() {
  const auto control = control_;
  std::shared_ptr<ClaimedInputTransportPump> local_lease;
  std::shared_ptr<ClaimedInputTransportPump> remote_lease;
  std::shared_ptr<void> local_context;
  std::shared_ptr<void> remote_context;
  int remote_fd = -1;
  {
    std::lock_guard lock(control->mutex);
    if (!control->registered_once || control->activated_once || control->closed)
      return false;
    control->activated_once = true;
    remote_fd = control->remote.fd;
    local_lease = control->local.lease;
    remote_lease = control->remote.lease;
    local_context = control->local.callback_context;
    remote_context = control->remote.callback_context;
    control->local.operation = true;
    control->remote.operation = remote_fd >= 0;
  }
  const bool has_remote = remote_fd >= 0;
  bool local_closed = false;

  const bool local_added = local_lease->Activate();
  OnSlotState(local_context.get(), local_lease->State());
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    // Keep the binding's reservation through the cleanup handoff if a
    // reentrant disposal closed it during provider publication.
    control->local.operation = !local_added || control->closed;
    if (!local_added) control->local.terminal = true;
    local_closed = control->closed;
    ++control->revision;
  }
  if (local_closed) {
    FinishActivationClose(control, local_lease, remote_lease, true,
                          has_remote);
    return false;
  }
  if (!local_added) {
    const bool retired = local_lease->Retire();
    const bool quiescent = retired && local_lease->IsQuiescent();
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->local.operation = false;
      control->local.registered = false;
      control->local.terminal = true;
      if (quiescent) control->local.lease.reset();
      ++control->revision;
      control->status_pending = true;
    }
    Notify(control);
    if (!has_remote) return false;
    // The local failure notification is reentrant. A status observer may have
    // retired the binding while it was delivered; do not publish a remote
    // registration after that closed transition. The remote slot still owns
    // only a pre-registration reservation, so release it under the lock.
    bool closed_after_local_failure = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      closed_after_local_failure = control->closed;
    }
    if (closed_after_local_failure) {
      FinishActivationClose(control, local_lease, remote_lease, true, true);
      return false;
    }
  }
  // Publish the local readiness edge before attempting the remote add. A
  // status callback may dispose the binding here; the subsequent closed check
  // prevents a remote add (or ready publication) after that transition.
  if (local_added) Notify(control);
  bool closed_after_local = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    closed_after_local = control->closed;
  }
  if (closed_after_local) {
    bool cleanup_local = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (!control->local.operation) {
        control->local.operation = true;
        cleanup_local = true;
      }
    }
    FinishActivationClose(control, local_lease, remote_lease, cleanup_local,
                          has_remote);
    return false;
  }
  if (!has_remote) return true;

  const bool remote_added = remote_lease->Activate();
  OnSlotState(remote_context.get(), remote_lease->State());
  bool remote_closed = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    // Keep the registration reservation through the cleanup handoff when a
    // reentrant status/event callback closed the binding during AddFdOwned.
    control->remote.operation = !remote_added || control->closed;
    if (!remote_added) control->remote.terminal = true;
    ++control->revision;
    remote_closed = control->closed;
  }
  if (remote_closed) {
    bool cleanup_local = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (!control->local.operation) {
        control->local.operation = true;
        cleanup_local = true;
      }
    }
    FinishActivationClose(control, local_lease, remote_lease, cleanup_local,
                          true);
    return false;
  }
  if (!remote_added) {
    // Registration is slot-independent: a failed remote add leaves a valid
    // local wake slot usable, while the remote slot remains terminal.
    const bool remote_retired = remote_lease->Retire();
    const bool remote_quiescent =
        remote_retired && remote_lease->IsQuiescent();
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->remote.operation = false;
      if (remote_quiescent) control->remote.lease.reset();
      ++control->revision;
      control->status_pending = true;
    }
    Notify(control);
    return local_added;
  }
  Notify(control);
  return true;
}

bool ReceiverEndpointBinding::RefreshWritable(
    ReceiverEndpointBindingSlot slot, int fd, bool enabled) {
  auto control = control_;
  std::shared_ptr<ClaimedInputTransportPump> lease;
  bool registered = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& state = SlotFor(*control, slot);
    if (control->closed || state.fd != fd || state.terminal ||
        state.lease == nullptr)
      return false;
    registered = state.registered;
    state.desired_writable = enabled;
    lease = state.lease;
  }

  // The claimed wrapper owns writable revisions for Prepared, Waiting, and
  // Active slots. Binding-side operation/pending state must not swallow a
  // request while Activate/Retire or provider cleanup is in flight.
  const auto refreshed = lease->SetWritableResult(enabled);
  if (refreshed == InputTransportWritableResult::kTerminal) {
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& state = SlotFor(*control, slot);
      if (state.lease == lease && state.fd == fd) {
        state.registered = false;
        state.terminal = true;
        state.refresh_pending = false;
        ++control->revision;
        control->status_pending = true;
      }
    }
    Notify(control);
    MaybeNotifyRetired(control);
    return false;
  }
  if (refreshed == InputTransportWritableResult::kDeferred) Notify(control);
  // A waiting/prepared request is accepted and retained by the wrapper even
  // though no provider registration exists yet. Active coalesced refreshes
  // retain the historical false result until the requested revision applies.
  return refreshed == InputTransportWritableResult::kApplied || !registered;
}

bool ReceiverEndpointBinding::Retire() {
  return RetireControl(control_);
}

ReceiverEndpointBinding::RetirementHandle
ReceiverEndpointBinding::RetainRetirement() const {
  return RetirementHandle(control_);
}

bool ReceiverEndpointBinding::RetirementHandle::Retire() {
  return control_ != nullptr && ReceiverEndpointBinding::RetireControl(control_);
}

bool ReceiverEndpointBinding::RetirementHandle::IsQuiescent() const {
  if (control_ == nullptr) return false;
  std::shared_ptr<ClaimedInputTransportPump> local;
  std::shared_ptr<ClaimedInputTransportPump> remote;
  {
    std::lock_guard<std::mutex> lock(control_->mutex);
    if (!control_->closed || control_->local.operation ||
        control_->remote.operation || control_->local.callback_active ||
        control_->remote.callback_active || control_->notifying) return false;
    local = control_->local.lease;
    remote = control_->remote.lease;
  }
  if ((local != nullptr && !local->IsQuiescent()) ||
      (remote != nullptr && !remote->IsQuiescent())) return false;
  std::lock_guard<std::mutex> lock(control_->mutex);
  return control_->closed && !control_->local.operation &&
         !control_->remote.operation && !control_->local.callback_active &&
         !control_->remote.callback_active && !control_->notifying &&
         control_->local.lease == local && control_->remote.lease == remote;
}

bool ReceiverEndpointBinding::RetirementHandle::SetQuiescenceNotification(
    void (*notify)(void*) noexcept, std::weak_ptr<void> context) {
  const auto control = control_;
  const auto owner = context.lock();
  if (control == nullptr || notify == nullptr || owner == nullptr) return false;
  bool already_notified = false;
  {
    std::lock_guard lock(control->mutex);
    if (control->retirement_notify != nullptr &&
        !control->retirement_context.expired())
      return false;
    control->retirement_notify = notify;
    control->retirement_context = std::move(context);
    already_notified = control->retirement_notified;
  }
  if (already_notified)
    notify(owner.get());
  else
    NotifyRetired(control);
  return true;
}

bool ReceiverEndpointBinding::RetireControl(
    std::shared_ptr<Control> control) {
  bool result = true;
  bool notify = false;
  bool was_closed = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    was_closed = control->closed;
    control->closed = true;
    control->status_pending = true;
  }
  for (const auto slot : {ReceiverEndpointBindingSlot::kLocalWake,
                          ReceiverEndpointBindingSlot::kRemote}) {
    std::shared_ptr<ClaimedInputTransportPump> lease;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (!was_closed) notify = true;
      control->closed = true;
      control->status_pending = true;
      auto& state = SlotFor(*control, slot);
      if (state.operation) continue;
      if (state.lease == nullptr) {
        state.registered = false;
        continue;
      }
      if (!was_closed) notify = true;
      state.operation = true;
      lease = state.lease;
    }
    const bool retired = lease->Retire();
    const bool quiescent = retired && lease->IsQuiescent();
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& state = SlotFor(*control, slot);
      state.operation = false;
      if (quiescent) {
        notify = true;
        state.registered = false;
        state.lease.reset();
        state.callback_context.reset();
      }
      ++control->revision;
    }
    result = result && retired;
  }
  if (notify) Notify(control);
  MaybeNotifyRetired(control);
  return result;
}

ReceiverEndpointBindingStatus ReceiverEndpointBinding::Status() const {
  std::lock_guard<std::mutex> lock(control_->mutex);
  return SnapshotLocked(*control_);
}

InputTransportReaderResult ReceiverEndpointBinding::OnSlotEvent(int fd, int events, void* data) {
  auto* context = static_cast<SlotContext*>(data);
  if (context == nullptr) return InputTransportReaderResult::kRetireRegistration;
  const auto control = context->control.lock();
  if (control == nullptr) return InputTransportReaderResult::kRetireRegistration;
  decltype(ReceiverEndpointBindingCallbacks::on_event) callback = nullptr;
  void* callback_context = nullptr;
  bool stale = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    const auto& state = SlotFor(*control, context->slot);
    callback = control->callbacks.on_event;
    if (control->closed) return InputTransportReaderResult::kRetireRegistration;
    if (state.callback_context.get() != context || state.terminal || state.reader_completed ||
        (!state.registered && !state.operation))
      return InputTransportReaderResult::kRetireRegistration;
    if (state.fd != fd || state.fd != context->fd) {
      auto& mutable_state = SlotFor(*control, context->slot);
      mutable_state.terminal = true;
      ++control->revision;
      control->status_pending = true;
      // The stale provider token is terminal for this slot. Do not permit a
      // later callback carrying the old fd to restore readiness.
      stale = true;
      callback_context = control->callbacks.context;
    } else {
      callback_context = control->callbacks.context;
    }
  }
  if (stale) {
    Notify(control);
    return InputTransportReaderResult::kRetireRegistration;
  }
  // The pump synthesizes ERROR when the resource cannot flush or its
  // descriptor is terminal. Publish the slot edge without entering the
  // receiver/JNI callback with a fabricated input event.
  if ((events & (0x0004 | 0x0010)) != 0) {
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& state = SlotFor(*control, context->slot);
      if (state.fd == fd && state.callback_context.get() == context) {
        state.registered = false;
        state.terminal = true;
        state.refresh_pending = false;
        ++control->revision;
        control->status_pending = true;
      }
    }
    Notify(control);
    MaybeNotifyRetired(control);
    return InputTransportReaderResult::kRetireRegistration;
  }
  if (callback == nullptr) return InputTransportReaderResult::kKeepReading;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& state = SlotFor(*control, context->slot);
    if (state.callback_context.get() != context || control->closed)
      return InputTransportReaderResult::kRetireRegistration;
    state.callback_active = true;
  }
  ReceiverEndpointBindingEventResult outcome =
      ReceiverEndpointBindingEventResult::kKeep;
  try {
    outcome = callback(callback_context, context->slot, fd, events);
  } catch (...) {
    ReceiverEndpointBindingCallbacks failure_callbacks;
    bool healthy = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      const auto& state = SlotFor(*control, context->slot);
      failure_callbacks = control->callbacks;
      auto& mutable_state = SlotFor(*control, context->slot);
      mutable_state.callback_active = false;
      if (!control->closed && state.callback_context.get() == context) {
        // The event was not terminal. Preserve a status edge for observers,
        // then keep the endpoint alive when the failure boundary returns.
        control->status_pending = true;
        healthy = true;
      }
    }
    ReportCallbackFailure(failure_callbacks);
    if (healthy) Notify(control);
    MaybeNotifyRetired(control);
    std::lock_guard<std::mutex> lock(control->mutex);
    return healthy && !control->closed ? InputTransportReaderResult::kKeepReading
                                      : InputTransportReaderResult::kRetireRegistration;
  }
  if (outcome == ReceiverEndpointBindingEventResult::kReaderComplete) {
    bool accepted = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& state = SlotFor(*control, context->slot);
      state.callback_active = false;
      accepted = !control->closed && state.callback_context.get() == context &&
          !state.terminal && context->slot == ReceiverEndpointBindingSlot::kRemote &&
          state.fd == fd && control->transport != nullptr &&
          fd == control->transport->RemoteEndpointFd() &&
          control->transport->IsRxTerminal();
      if (accepted) {
        state.reader_completed = true;
        ++control->revision;
        control->status_pending = true;
      }
    }
    if (accepted) {
      // Completion removes reader readiness, not pump ownership. Status
      // observers can retire reentrantly; their authority always wins.
      Notify(control);
      std::lock_guard<std::mutex> lock(control->mutex);
      const auto& state = SlotFor(*control, context->slot);
      return !control->closed && !state.terminal &&
                     state.callback_context.get() == context
                 ? InputTransportReaderResult::kReaderComplete
                 : InputTransportReaderResult::kRetireRegistration;
    }
    outcome = ReceiverEndpointBindingEventResult::kTerminal;
  }
  if (outcome == ReceiverEndpointBindingEventResult::kKeep) {
    // Disposal may reenter the user callback. Return terminal to the pump so
    // its in-flight operation performs the exact provider removal before the
    // callback context can be released.
    bool keep = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      auto& state = SlotFor(*control, context->slot);
      state.callback_active = false;
      keep = !control->closed;
    }
    MaybeNotifyRetired(control);
    return keep ? InputTransportReaderResult::kKeepReading
                : InputTransportReaderResult::kRetireRegistration;
  }
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& state = SlotFor(*control, context->slot);
    if (state.fd == fd && state.callback_context.get() == context) {
      state.callback_active = false;
      state.terminal = true;
      ++control->revision;
      control->status_pending = true;
    }
  }
  Notify(control);
  MaybeNotifyRetired(control);
  return InputTransportReaderResult::kRetireRegistration;
}

void ReceiverEndpointBinding::OnSlotState(void* data,
                                           ClaimedPumpState observed) noexcept {
  auto* context = static_cast<SlotContext*>(data);
  if (context == nullptr) return;
  const auto control = context->control.lock();
  if (control == nullptr) return;
  std::shared_ptr<ClaimedInputTransportPump> lease;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& state = SlotFor(*control, context->slot);
    if (state.callback_context.get() != context || control->token != context->token)
      return;
    lease = state.lease;
  }
  if (lease == nullptr || lease->State() != observed) return;
  const bool failed = lease->Failed();
  const bool quiescent = lease->IsQuiescent();
  bool changed = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& state = SlotFor(*control, context->slot);
    if (state.callback_context.get() != context || control->token != context->token ||
        state.lease != lease)
      return;
    const bool was_registered = state.registered;
    const bool owns_operation = state.operation;
    if (observed == ClaimedPumpState::kActive && !failed && !control->closed &&
        !state.terminal) {
      state.registered = true;
      if (!owns_operation) state.operation = false;
    } else if (observed == ClaimedPumpState::kPrepared ||
               observed == ClaimedPumpState::kWaiting) {
      // Deferred authority admission is accepted, not terminal. Preserve the
      // wrapper so a later availability hint can make this slot active.
      state.registered = false;
      if (!owns_operation) state.operation = false;
    } else {
      state.registered = false;
      if (!owns_operation) state.operation = false;
      state.terminal = true;
      state.refresh_pending = false;
    }
    changed = was_registered != state.registered || failed ||
              observed == ClaimedPumpState::kRetiring ||
              observed == ClaimedPumpState::kSettled;
    if (changed) {
      ++control->revision;
      control->status_pending = true;
    }
  }
  if (changed) Notify(control);
  // Full claimed quiescence includes the reusable-task tail; raw pump
  // completion alone is intentionally insufficient for retirement.
  if (quiescent || observed == ClaimedPumpState::kSettled)
    MaybeNotifyRetired(control);
}

void ReceiverEndpointBinding::OnSlotQuiescent(void* data) noexcept {
  const auto* context = static_cast<const SlotContext*>(data);
  if (context == nullptr) return;
  if (const auto control = context->control.lock()) MaybeNotifyRetired(control);
}

void ReceiverEndpointBinding::OnSlotTerminal(void* data) noexcept {
  auto* context = static_cast<SlotContext*>(data);
  if (context == nullptr) return;
  const auto control = context->control.lock();
  if (control == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    auto& state = SlotFor(*control, context->slot);
    if (state.callback_context.get() != context) return;
    state.registered = false;
    state.terminal = true;
    state.refresh_pending = false;
    ++control->revision;
    control->status_pending = true;
  }
  Notify(control);
  MaybeNotifyRetired(control);
}

void ReceiverEndpointBinding::Notify(const std::shared_ptr<Control>& control) {
  bool start = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->status_pending = true;
    if (!control->notifying) {
      control->notifying = true;
      start = true;
    }
  }
  if (!start) return;
  for (;;) {
    ReceiverEndpointBindingCallbacks callbacks;
    ReceiverEndpointBindingStatus status;
    bool done = false;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (!control->status_pending) {
        control->notifying = false;
        done = true;
      } else {
        control->status_pending = false;
        callbacks = control->callbacks;
        status = SnapshotLocked(*control);
      }
    }
    if (done) {
      // Completion is checked after the observer has fully returned; a
      // reentrant Retire during on_status must not announce early.
      MaybeNotifyRetired(control);
      return;
    }
    try {
      if (callbacks.on_status != nullptr)
        callbacks.on_status(callbacks.context, status);
    } catch (...) {
      // Do not claim that a status edge was delivered when its observer
      // failed. Leave it pending, clear the coalescing guard, and report the
      // failure outside the mutex so Retire/Notify can safely reenter.
      ReceiverEndpointBindingCallbacks failure_callbacks;
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        control->status_pending = true;
        control->notifying = false;
        failure_callbacks = control->callbacks;
      }
      ReportCallbackFailure(failure_callbacks);
      return;
    }
  }
}

}  // namespace darwin_art::input
