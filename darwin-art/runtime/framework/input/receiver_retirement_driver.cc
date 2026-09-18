#include "receiver_retirement_driver.h"

#include "claimed_input_transport_pump.h"
#include "pending_receiver_retirement.h"
#include "receiver_retirement_barrier.h"
#include "routing_transport_dispatch.h"
#include "../../../compat/looper/android_looper_owner.h"

#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::input {

namespace {

constexpr int kOutputEvent = 0x0002;
constexpr std::uint64_t kRetiredOutputIdentity = UINT64_C(1) << 63;

}  // namespace

struct ReceiverRetirementDriver::Control {
  struct Token {
    std::weak_ptr<Control> control;
  };

  mutable std::mutex mutex;
  const std::weak_ptr<PendingReceiverRetirement> record;
  void* const looper;
  const int fd;
  const std::uint64_t identity;
  const std::shared_ptr<InputTransport> transport;
  const InputRoutingHandle routing;
  const ReceiverRetirementDriverHooks hooks;
  std::shared_ptr<Token> token;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  InputResourceProgressSource::SubscriptionHandle progress_subscription;
  InputRoutingNotificationSubscriptionHandle routing_subscription;
  std::shared_ptr<ClaimedInputTransportPump> orphan;
  bool close_requested = false;
  bool close_completed = false;
  bool pump_activation_attempted = false;
  bool pump_quiescent = false;
  bool pump_terminal = false;
  bool output_armed = false;
  bool retry_pending = false;
  bool retry_dispatch_issued = false;
  bool retry_exhausted = false;
  bool task_cancelled = false;
  bool driving = false;
  bool settled = false;
  bool failed = false;

  Control(PendingReceiverRetirementHandle owner, void* owner_looper,
          std::shared_ptr<InputTransport> owner_transport, int owner_fd,
          std::uint64_t owner_identity, InputRoutingHandle owner_routing,
          ReceiverRetirementDriverHooks owner_hooks)
      : record(std::move(owner)),
        looper(owner_looper),
        fd(owner_fd),
        identity(owner_identity),
        transport(std::move(owner_transport)),
        routing(std::move(owner_routing)),
        hooks(std::move(owner_hooks)) {}
};

std::shared_ptr<ReceiverRetirementDriver>
ReceiverRetirementDriver::Prepare(PendingReceiverRetirementHandle record,
                                  void* looper,
                                  std::shared_ptr<InputTransport> transport,
                                  int original_fd, ReceiverId registry_id,
                                  InputRoutingHandle routing,
                                  ReceiverRetirementDriverHooks hooks) {
  if (record == nullptr || looper == nullptr || transport == nullptr ||
      original_fd < 0 || registry_id == 0 ||
      (registry_id & kRetiredOutputIdentity) != 0 || routing == nullptr ||
      darwin_art::looper::Current() != looper)
    return {};

  const std::uint64_t identity = kRetiredOutputIdentity | registry_id;
  std::shared_ptr<Control> control;
  std::shared_ptr<ReceiverRetirementDriver> driver;
  try {
    control = std::make_shared<Control>(
        std::move(record), looper, std::move(transport), original_fd, identity,
        routing, std::move(hooks));
    control->token = std::make_shared<Control::Token>();
    control->token->control = control;
    control->task = darwin_art::looper::ReusableLooperTask::Prepare(
        looper,
        darwin_art::looper::ReusableLooperTaskCallbacks{
            &ReceiverRetirementDriver::OnTask, control->token.get(),
            control->token, &ReceiverRetirementDriver::OnTaskQuiescent});
    if (control->task == nullptr) return {};

    const std::weak_ptr<void> weak_token = control->token;
    const auto prepared_record = control->record.lock();
    if (!SubscribePendingReceiverRetirementQuiescence(
            prepared_record, &ReceiverRetirementDriver::OnResourceQuiescence,
            weak_token))
      return {};
    control->progress_subscription = control->transport->SubscribeProgress(
        &ReceiverRetirementDriver::OnTransportProgress, weak_token);
    if (!control->progress_subscription) return {};

    control->routing_subscription = SubscribeInputRoutingNotifications(
        routing,
        InputRoutingNotificationCallbacks{
            &ReceiverRetirementDriver::OnRoutingNotification, control->token.get(),
            weak_token});
    if (control->routing_subscription == nullptr) return {};

    InputTransportPumpCallbacks callbacks;
    callbacks.context = control->token.get();
    callbacks.context_owner = control->token;
    callbacks.on_terminal = &ReceiverRetirementDriver::OnPumpTerminal;
    callbacks.on_quiescent = &ReceiverRetirementDriver::OnPumpQuiescent;
    const ClaimedPumpObserver observer{
        &ReceiverRetirementDriver::OnPumpState, control->token.get(),
        control->token};
    const bool initial_pending_tx = control->transport->HasPendingTx();
    control->orphan = ClaimedInputTransportPump::Prepare(
        looper, control->transport, original_fd,
        initial_pending_tx ? kOutputEvent : 0,
        TransportRegistrationRole::kRetiredOutput, identity, callbacks,
        nullptr, nullptr, {}, observer);
    if (control->orphan == nullptr) return {};
    control->output_armed = initial_pending_tx;

    // The constructor is intentionally private; allocate it here rather than
    // through make_shared (whose allocator is not a friend).
    driver = std::shared_ptr<ReceiverRetirementDriver>(
        new ReceiverRetirementDriver(control));
  } catch (const std::bad_alloc&) {
    return {};
  }
  return driver;
}

ReceiverRetirementDriver::~ReceiverRetirementDriver() {
  (void)Close();
}

bool ReceiverRetirementDriver::Request() noexcept {
  const auto control = control_;
  if (control == nullptr) return false;
  // Public requests are explicit progress epochs; permit one fresh retry of
  // a healthy provider even when the preceding internal attempt was spent.
  {
    std::lock_guard lock(control->mutex);
    control->retry_pending = false;
    control->retry_dispatch_issued = false;
    control->retry_exhausted = false;
  }
  return RequestControl(control);
}

bool ReceiverRetirementDriver::RequestControl(
    const std::shared_ptr<Control>& control) noexcept {
  if (control == nullptr) return false;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  {
    std::lock_guard lock(control->mutex);
    if (control->settled || control->task == nullptr) return false;
    task = control->task;
  }
  return task->Request();
}

void ReceiverRetirementDriver::RequestAfterHint(
    const std::shared_ptr<Control>& control) noexcept {
  if (control == nullptr) return;
  {
    std::lock_guard lock(control->mutex);
    // A real resource/routing progress edge opens one bounded retry epoch.
    control->retry_pending = false;
    control->retry_dispatch_issued = false;
    control->retry_exhausted = false;
  }
  (void)RequestControl(control);
}

void ReceiverRetirementDriver::RequestBoundedRetry(
    const std::shared_ptr<Control>& control) noexcept {
  if (control == nullptr) return;
  bool request = false;
  {
    std::lock_guard lock(control->mutex);
    control->retry_pending = true;
    if (!control->retry_dispatch_issued) {
      control->retry_dispatch_issued = true;
      request = true;
    } else {
      control->retry_exhausted = true;
    }
  }
  if (request) (void)RequestControl(control);
}

bool ReceiverRetirementDriver::Close() noexcept {
  const auto control = control_;
  if (control == nullptr) return true;
  PendingReceiverRetirementHandle record;
  bool need_close = false;
  {
    std::lock_guard lock(control->mutex);
    if (control->settled) return true;
    control->close_requested = true;
    need_close = !control->close_completed;
    record = control->record.lock();
  }
  bool logical_close = !need_close;
  bool close_failed = false;
  if (need_close && record != nullptr) {
    try {
      logical_close =
          ClosePendingReceiverRetirement(record) == ReceiverRoutingCloseStatus::kClosed;
      if (logical_close) {
        std::lock_guard lock(control->mutex);
        control->close_completed = true;
      }
    } catch (...) {
      std::lock_guard lock(control->mutex);
      control->failed = true;
      close_failed = true;
    }
  } else if (record == nullptr) {
    std::lock_guard lock(control->mutex);
    control->failed = true;
  }
  if (close_failed) RequestBoundedRetry(control);
  const bool queued = Request();
  // Logical close is complete even if a provider wake failed; the reusable
  // task retains its request for a later retry.
  return logical_close && queued;
}

bool ReceiverRetirementDriver::IsQuiescent() const {
  const auto control = control_;
  if (control == nullptr) return true;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  std::shared_ptr<ClaimedInputTransportPump> orphan;
  bool settled = false;
  {
    std::lock_guard lock(control->mutex);
    settled = control->settled;
    task = control->task;
    orphan = control->orphan;
  }
  // Provider quiescence may invoke user-owned metadata, so never hold the
  // driver's mutex while asking another owner about its state.
  return settled && (task == nullptr || task->IsQuiescent()) &&
         (orphan == nullptr || orphan->IsQuiescent());
}

bool ReceiverRetirementDriver::Failed() const {
  const auto control = control_;
  if (control == nullptr) return true;
  std::lock_guard lock(control->mutex);
  return control->failed;
}

void ReceiverRetirementDriver::OnTask(void* data) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control != nullptr) Drive(control);
}

void ReceiverRetirementDriver::OnTaskQuiescent(void* data) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control != nullptr) TrySettle(control);
}

void ReceiverRetirementDriver::OnTransportProgress(
    void* data, InputResourceProgress) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control != nullptr) {
    bool close_requested = false;
    {
      std::lock_guard lock(control->mutex);
      close_requested = control->close_requested;
    }
    if (close_requested) RequestAfterHint(control);
  }
}

void ReceiverRetirementDriver::OnRoutingNotification(
    void* data, const InputRoutingNotification& notification) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control == nullptr ||
      notification.kind == InputRoutingNotificationKind::kActionUnclaimed)
    return;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  {
    std::lock_guard lock(control->mutex);
    if (!control->close_requested || control->settled) return;
    task = control->task;
  }
  if (task != nullptr) RequestAfterHint(control);
}

void ReceiverRetirementDriver::OnPumpTerminal(void* data) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control == nullptr) return;
  // The pump reports both a genuine transport terminal and a provider
  // registration/rearm failure. Only the former forbids a fresh OUTPUT
  // orphan; a healthy transport must be allowed to retry after full Q.
  const bool transport_terminal = control->transport == nullptr ||
                                  control->transport->IsTxTerminal();
  {
    std::lock_guard lock(control->mutex);
    // Do not spend the retry credit here: a raw terminal callback precedes
    // full claimed-pump quiescence, where Failed() can classify provider
    // registration failure versus a deliberate authority yield exactly once.
    control->pump_terminal = transport_terminal;
    control->failed = true;
  }
  (void)RequestControl(control);
}

void ReceiverRetirementDriver::OnPumpQuiescent(void* data) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control == nullptr) return;
  // InputTransport's metadata callback precedes the ClaimedPump's own
  // authority/task settlement. OnPumpState(kSettled) is the durable proof.
  (void)RequestControl(control);
}

void ReceiverRetirementDriver::OnResourceQuiescence(void* data) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control == nullptr) return;
  RequestAfterHint(control);
}

void ReceiverRetirementDriver::OnPumpState(void* data,
                                            ClaimedPumpState state) noexcept {
  auto* token = static_cast<Control::Token*>(data);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control == nullptr) return;
  if (state == ClaimedPumpState::kSettled) {
    std::shared_ptr<ClaimedInputTransportPump> orphan;
    {
      std::lock_guard lock(control->mutex);
      control->pump_quiescent = true;
      orphan = control->orphan;
    }
    const bool failed = orphan != nullptr && orphan->Failed();
    if (failed) {
      const bool transport_terminal = control->transport == nullptr ||
                                      control->transport->IsTxTerminal();
      std::lock_guard lock(control->mutex);
      if (transport_terminal) {
        control->pump_terminal = true;
      } else if (control->retry_dispatch_issued) {
        control->retry_exhausted = true;
      } else {
        control->retry_dispatch_issued = true;
      }
    }
    (void)RequestControl(control);
  }
}

void ReceiverRetirementDriver::TrySettle(
    const std::shared_ptr<Control>& control) noexcept {
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  std::shared_ptr<ClaimedInputTransportPump> orphan;
  std::weak_ptr<PendingReceiverRetirement> weak_record;
  {
    std::lock_guard lock(control->mutex);
    if (control->settled || !control->task_cancelled || control->driving)
      return;
    task = control->task;
    orphan = control->orphan;
    weak_record = control->record;
  }
  if (task != nullptr && !task->IsQuiescent()) return;
  if (orphan != nullptr && !orphan->IsQuiescent()) return;
  const auto record = weak_record.lock();
  if (record == nullptr) return;
  if (!ReleaseSettledReceiverRetirement(record)) return;
  InputResourceProgressSource::SubscriptionHandle progress_subscription;
  InputRoutingNotificationSubscriptionHandle routing_subscription;
  {
    std::lock_guard lock(control->mutex);
    control->settled = true;
    progress_subscription = std::move(control->progress_subscription);
    routing_subscription = std::move(control->routing_subscription);
    // Keep these leases pinned until after the lock is released. Their
    // destructors unsubscribe from independent owners and may reenter.
    task = std::move(control->task);
    orphan = std::move(control->orphan);
  }
  routing_subscription.reset();
  progress_subscription = {};
  orphan.reset();
  task.reset();
}

void ReceiverRetirementDriver::Drive(
    const std::shared_ptr<Control>& control) noexcept {
  if (darwin_art::looper::Current() != control->looper) {
    (void)RequestControl(control);
    return;
  }
  bool reentrant = false;
  {
    std::lock_guard lock(control->mutex);
    if (control->settled) return;
    if (control->driving) {
      reentrant = true;
    } else {
      control->driving = true;
    }
  }
  if (reentrant) {
    (void)RequestControl(control);
    return;
  }
  struct DrivingGuard {
    std::shared_ptr<Control> control;
    ~DrivingGuard() {
      std::lock_guard lock(control->mutex);
      control->driving = false;
    }
  } guard{control};

  const auto record = control->record.lock();
  if (record == nullptr) {
    std::lock_guard lock(control->mutex);
    control->failed = true;
    control->close_requested = true;
  } else {
    bool close_requested = false;
    {
      std::lock_guard lock(control->mutex);
      close_requested = control->close_requested;
    }
    if (close_requested) {
      try {
        const auto status = ClosePendingReceiverRetirement(record);
        if (status == ReceiverRoutingCloseStatus::kClosed) {
          std::lock_guard lock(control->mutex);
          control->close_completed = true;
        }
      } catch (...) {
        {
          std::lock_guard lock(control->mutex);
          control->failed = true;
        }
        RequestBoundedRetry(control);
      }
    }
  }

  ReceiverRetirementBarrierQuery barrier;
  bool close_requested_now = false;
  {
    std::lock_guard lock(control->mutex);
    close_requested_now = control->close_requested;
  }
  if (close_requested_now && control->routing != nullptr) {
    RoutingTransportDrainResult drained;
    bool drain_failed = false;
    try {
      drained = DrainInputRoutingTransport(control->routing);
    } catch (...) {
      std::lock_guard lock(control->mutex);
      control->failed = true;
      drain_failed = true;
    }
    if (drain_failed) RequestBoundedRetry(control);
    std::shared_ptr<void> hook_pin;
    if (control->hooks.context_token.use_count() != 0)
      hook_pin = control->hooks.context_token.lock();
    const bool hooks_requested =
        control->hooks.refresh_writable != nullptr ||
        control->hooks.wake_local != nullptr;
    // A supplied raw context is never usable after its weak lifetime token
    // expires. Hooks without callbacks are inert and need no token.
    const bool hooks_live = !hooks_requested || hook_pin != nullptr ||
                            control->hooks.context_owner != nullptr;
    if (hooks_live && drained.remote_admitted &&
        control->hooks.refresh_writable != nullptr)
      (void)control->hooks.refresh_writable(control->hooks.context,
                                             control->routing);
    if (hooks_live && drained.local_queued &&
        control->hooks.wake_local != nullptr)
      (void)control->hooks.wake_local(control->hooks.context);
    // A bounded drain may continue only when it made progress. Backpressure
    // waits for the resource/routing hint; this avoids an ActionUnclaimed
    // notification turning into a self-sustaining owner-task spin.
    if (drained.continuation_needed && !drained.backpressured)
      (void)RequestControl(control);
  }
  if (record != nullptr) {
    bool poll_failed = false;
    try {
      barrier = PollPendingReceiverRetirement(record);
    } catch (...) {
      std::lock_guard lock(control->mutex);
      control->failed = true;
      poll_failed = true;
    }
    if (poll_failed) RequestBoundedRetry(control);
  } else {
    barrier.status = ReceiverRetirementBarrierStatus::kInvalid;
  }

  // RetryInputRoutingCancellations intentionally hides allocation failure and
  // leaves the CANCEL obligation pending. With no TX wake to drive another
  // pass, grant one bounded follow-up; subsequent attempts require an
  // external routing/progress epoch.
  if (record != nullptr && control->routing != nullptr &&
      barrier.status == ReceiverRetirementBarrierStatus::kRouting &&
      barrier.routing.status == InputRoutingRetirementStatus::kRunnable &&
      !control->transport->HasPendingTx())
    RequestBoundedRetry(control);

  // Hints can race preparation. They never activate an orphan for a live
  // receiver; only an explicit Close starts the retirement state machine.
  if (!close_requested_now) return;
  if (record == nullptr) return;

  bool close_completed = false;
  bool pump_quiescent = false;
  bool pump_terminal = false;
  bool retry_exhausted = false;
  std::shared_ptr<ClaimedInputTransportPump> orphan;
  {
    std::lock_guard lock(control->mutex);
    close_completed = control->close_completed;
    pump_quiescent = control->pump_quiescent;
    pump_terminal = control->pump_terminal;
    retry_exhausted = control->retry_exhausted;
    orphan = control->orphan;
  }

  if (barrier.status == ReceiverRetirementBarrierStatus::kReady) {
    if (orphan != nullptr && !pump_quiescent) {
      (void)orphan->Retire();
      // Retire acceptance is not full quiescence. Keep this exact wrapper and
      // its claim alive until its observer reports kSettled.
      if (!orphan->IsQuiescent()) return;
      pump_quiescent = true;
      std::lock_guard lock(control->mutex);
      control->pump_quiescent = true;
    }
    if (orphan != nullptr && !orphan->IsQuiescent()) return;
    std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
    {
      std::lock_guard lock(control->mutex);
      task = control->task;
      control->task_cancelled = true;
    }
    if (task != nullptr) (void)task->Cancel();
    TrySettle(control);
    return;
  }

  // A terminal/retired orphan may be replaced only after its complete claim
  // and task quiescence. Move the wrapper out under the state lock, then let
  // its destructor run without holding the driver lock.
  if (orphan != nullptr && pump_quiescent && !pump_terminal) {
    std::shared_ptr<ClaimedInputTransportPump> retired;
    {
      std::lock_guard lock(control->mutex);
      retired = std::move(control->orphan);
      control->pump_quiescent = false;
      control->pump_terminal = false;
      control->output_armed = false;
      orphan.reset();
    }
    retired.reset();
  }

  if (orphan == nullptr && !pump_terminal && !retry_exhausted &&
      record != nullptr &&
      close_completed) {
    const bool pending_tx = control->transport->HasPendingTx();
    bool next_failed = false;
    InputTransportPumpCallbacks callbacks;
    callbacks.context = control->token.get();
    callbacks.context_owner = control->token;
    callbacks.on_terminal = &ReceiverRetirementDriver::OnPumpTerminal;
    callbacks.on_quiescent = &ReceiverRetirementDriver::OnPumpQuiescent;
    auto next = ClaimedInputTransportPump::Prepare(
        control->looper, control->transport, control->fd,
        pending_tx ? kOutputEvent : 0,
        TransportRegistrationRole::kRetiredOutput,
        control->identity, callbacks, nullptr, nullptr, {},
        ClaimedPumpObserver{&ReceiverRetirementDriver::OnPumpState,
                            control->token.get(), control->token});
    if (next == nullptr) {
      std::lock_guard lock(control->mutex);
      control->failed = true;
      control->retry_pending = true;
      next_failed = true;
    } else {
      {
        std::lock_guard lock(control->mutex);
        control->orphan = std::move(next);
        control->pump_activation_attempted = false;
        control->output_armed = pending_tx;
        control->retry_pending = false;
        orphan = control->orphan;
      }
    }
    if (next_failed) RequestBoundedRetry(control);
  }

  if (!close_completed) return;

  bool activate = false;
  {
    std::lock_guard lock(control->mutex);
    orphan = control->orphan;
    if (orphan != nullptr && !control->pump_activation_attempted) {
      control->pump_activation_attempted = true;
      activate = true;
    }
  }
  if (activate && orphan != nullptr) {
    if (!orphan->Activate()) {
      std::lock_guard lock(control->mutex);
      control->failed = true;
    }
  }

  // OUTPUT is level-triggered only while the transport owns pending TX.
  // Keeping it armed for an empty queue creates an owner-looper spin while
  // admission/binding quiescence is still pending.
  if (orphan != nullptr) {
    const auto state = orphan->State();
    if (state != ClaimedPumpState::kActive &&
        state != ClaimedPumpState::kPrepared &&
        state != ClaimedPumpState::kWaiting)
      return;
    const bool pending_tx = control->transport->HasPendingTx();
    bool output_armed = false;
    {
      std::lock_guard lock(control->mutex);
      output_armed = control->output_armed;
    }
    if (pending_tx != output_armed) {
      const auto result = orphan->SetWritableResult(pending_tx);
      if (result == InputTransportWritableResult::kApplied ||
          result == InputTransportWritableResult::kDeferred) {
        std::lock_guard lock(control->mutex);
        control->output_armed = pending_tx;
      } else if (result == InputTransportWritableResult::kTerminal) {
        std::lock_guard lock(control->mutex);
        control->failed = true;
        if (control->transport == nullptr || control->transport->IsTxTerminal())
          control->pump_terminal = true;
      }
    }
  }
}

}  // namespace darwin_art::input
