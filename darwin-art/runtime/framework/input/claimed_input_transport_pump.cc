#include "claimed_input_transport_pump.h"

#include "../../../compat/looper/android_looper_owner.h"

#include <android/looper.h>

#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::input {

namespace {

constexpr int kOutputEvent = 0x0002;

struct RetentionList {
  std::mutex mutex;
  std::shared_ptr<ClaimedInputTransportPump::Control> head;
  ClaimedInputTransportPump::Control* tail = nullptr;
};

// Deliberately leaked process state: controls can outlive all Java/framework
// handles while a provider still owns a callback or an exact FD removal.
RetentionList& ProcessRetention() {
  static auto* list = new RetentionList;
  return *list;
}

}  // namespace

struct ClaimedInputTransportPump::Control {
  struct Token {
    std::weak_ptr<Control> control;
  };

  struct PumpBridge {
    std::weak_ptr<Control> control;
    InputTransportPumpCallbacks user;
  };

  mutable std::mutex mutex;
  void* const looper;
  const int fd;
  const int base_events;
  const TransportRegistrationRole role;
  const std::uint64_t identity;
  darwin_art::looper::FdCallback fd_callback = nullptr;
  void* fd_context = nullptr;
  std::shared_ptr<void> fd_owner;
  std::shared_ptr<InputTransport> transport;
  TransportRegistrationAuthority::Claim claim;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  std::shared_ptr<Token> token;
  std::shared_ptr<PumpBridge> bridge;
  std::shared_ptr<InputTransportPumpLease> pump;
  InputTransportReaderCallback reader_callback = nullptr;
  void* reader_context = nullptr;
  std::shared_ptr<void> reader_owner;
  ClaimedPumpObserver observer;
  std::shared_ptr<Control> registry_next;
  Control* registry_prev = nullptr;
  bool activated = false;
  bool claim_admitted = false;
  bool claim_released = false;
  bool release_inflight = false;
  bool closed = false;
  bool failed = false;
  bool settled = false;
  bool writable = false;
  std::uint64_t writable_revision = 0;
  std::uint64_t applied_writable_revision = 0;
  bool waiting_subscribed = false;
  bool receiver_waiting_subscribed = false;
  bool retained = false;
  bool driving = false;
  bool retire_retry_requested = false;
  ClaimedPumpState state = ClaimedPumpState::kPrepared;

  Control(void* owner_looper, int owner_fd, int owner_events,
          TransportRegistrationRole owner_role, std::uint64_t owner_identity)
      : looper(owner_looper),
        fd(owner_fd),
        base_events(owner_events & ~kOutputEvent),
        role(owner_role),
        identity(owner_identity),
        writable((owner_events & kOutputEvent) != 0) {}
};

namespace {

void Enlist(const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  auto& list = ProcessRetention();
  std::lock_guard<std::mutex> lock(list.mutex);
  control->registry_next = std::move(list.head);
  if (control->registry_next != nullptr)
    control->registry_next->registry_prev = control.get();
  else
    list.tail = control.get();
  list.head = control;
  control->retained = true;
}

void Unlink(const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  if (control == nullptr) return;
  auto& list = ProcessRetention();
  std::shared_ptr<ClaimedInputTransportPump::Control> next;
  {
    std::lock_guard<std::mutex> lock(list.mutex);
    if (!control->retained) return;
    auto previous = control->registry_prev;
    next = std::move(control->registry_next);
    if (previous != nullptr)
      previous->registry_next = std::move(next);
    else
      list.head = std::move(next);
    if (previous != nullptr && previous->registry_next != nullptr)
      previous->registry_next->registry_prev = previous;
    else if (previous == nullptr && list.head != nullptr)
      list.head->registry_prev = nullptr;
    else
      list.tail = previous;
    control->registry_prev = nullptr;
    control->retained = false;
  }
}

void SetState(const std::shared_ptr<ClaimedInputTransportPump::Control>& control,
              ClaimedPumpState state) {
  ClaimedPumpObserver observer;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->state == state || control->settled ||
        (control->closed && state != ClaimedPumpState::kRetiring &&
         state != ClaimedPumpState::kSettled))
      return;
    control->state = state;
    observer = control->observer;
  }
  if (observer.on_state != nullptr) {
    try {
      observer.on_state(observer.context, state);
    } catch (...) {
      // Observer metadata cannot strand owner-thread lifecycle work.
    }
  }
}

bool RequestService(const std::shared_ptr<ClaimedInputTransportPump::Control>&
                        control) noexcept {
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->activated || control->settled || control->task == nullptr)
      return false;
    task = control->task;
  }
  return task->Request();
}

void RequestFromHint(void* context) noexcept {
  auto* token = static_cast<ClaimedInputTransportPump::Control::Token*>(context);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control != nullptr) (void)RequestService(control);
}

void MarkRetiring(const std::shared_ptr<ClaimedInputTransportPump::Control>&
                      control,
                  bool failed) noexcept {
  bool changed = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->settled) {
      changed = control->state != ClaimedPumpState::kRetiring;
      control->closed = true;
      // A pre-activation Retire still needs one owner-looper turn to release
      // its unadmitted claim. It does not make the registration visible.
      control->activated = true;
      control->failed = control->failed || failed;
      control->state = ClaimedPumpState::kRetiring;
    }
  }
  if (changed) {
    ClaimedPumpObserver observer;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      observer = control->observer;
    }
    if (observer.on_state != nullptr) {
      try {
        observer.on_state(observer.context, ClaimedPumpState::kRetiring);
      } catch (...) {
      }
    }
  }
  (void)RequestService(control);
}

bool ProxyPacket(void* context, const DarwinArtInputPacket& packet) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(
      context);
  return bridge != nullptr && bridge->user.on_packet != nullptr
             ? bridge->user.on_packet(bridge->user.context, packet)
             : true;
}

void ProxyWindow(void* context, int32_t left, int32_t top, int32_t right,
                 int32_t bottom, bool visible) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(
      context);
  if (bridge != nullptr && bridge->user.on_window != nullptr)
    bridge->user.on_window(bridge->user.context, left, top, right, bottom,
                           visible);
}

void ProxyAck(void* context, uint32_t sequence, bool handled) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(
      context);
  if (bridge != nullptr && bridge->user.on_ack != nullptr)
    bridge->user.on_ack(bridge->user.context, sequence, handled);
}

void ProxyAck64(void* context, uint64_t sequence, bool handled) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(context);
  if (bridge != nullptr && bridge->user.on_ack64 != nullptr)
    bridge->user.on_ack64(bridge->user.context, sequence, handled);
}

InputTransportConsumptionResult ProxyPacketConsumption(
    void* context, const DarwinArtInputPacket& packet) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(context);
  return bridge->user.on_packet_consumption(bridge->user.context, packet);
}

InputTransportConsumptionResult ProxyWindowConsumption(
    void* context, int32_t left, int32_t top, int32_t right,
    int32_t bottom, bool visible) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(context);
  return bridge->user.on_window_consumption(bridge->user.context, left, top,
                                          right, bottom, visible);
}

void ProxyProgress(void* context, InputTransportStatus status) {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(
      context);
  if (bridge != nullptr && bridge->user.on_progress != nullptr)
    bridge->user.on_progress(bridge->user.context, status);
}

void ProxyTerminal(void* context) noexcept {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(
      context);
  if (bridge == nullptr) return;
  const auto control = bridge->control.lock();
  if (control != nullptr) MarkRetiring(control, true);
  if (bridge->user.on_terminal != nullptr) {
    try {
      bridge->user.on_terminal(bridge->user.context);
    } catch (...) {
    }
  }
}

void ProxyQuiescent(void* context) noexcept {
  auto* bridge = static_cast<ClaimedInputTransportPump::Control::PumpBridge*>(
      context);
  if (bridge != nullptr && bridge->user.on_quiescent != nullptr) {
    try {
      bridge->user.on_quiescent(bridge->user.context);
    } catch (...) {
    }
  }
  const auto control = bridge == nullptr
                           ? std::shared_ptr<ClaimedInputTransportPump::Control>()
                           : bridge->control.lock();
  if (control != nullptr) (void)RequestService(control);
}

void SubscribeAvailable(
    const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  std::shared_ptr<ClaimedInputTransportPump::Control::Token> token;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->waiting_subscribed || control->settled ||
        control->claim_released || !control->claim)
      return;
    control->waiting_subscribed = true;
    token = control->token;
  }
  std::weak_ptr<void> weak_token = token;
  if (!control->claim.SubscribeAvailable(&RequestFromHint, weak_token)) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->waiting_subscribed = false;
  }
}

void SubscribeReceiverWaiting(
    const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  std::shared_ptr<ClaimedInputTransportPump::Control::Token> token;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->receiver_waiting_subscribed || control->settled ||
        control->claim_released || !control->claim)
      return;
    control->receiver_waiting_subscribed = true;
    token = control->token;
  }
  std::weak_ptr<void> weak_token = token;
  if (!control->claim.SubscribeReceiverWaiting(&RequestFromHint, weak_token)) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->receiver_waiting_subscribed = false;
  }
}

void Settle(const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  std::shared_ptr<InputTransportPumpLease> pump;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->settled || !control->closed || !control->claim_released)
      return;
    pump = control->pump;
    task = control->task;
  }
  if ((pump != nullptr && !pump->IsQuiescent()) ||
      (task != nullptr && !task->IsQuiescent()))
    return;
  ClaimedPumpObserver observer;
  std::shared_ptr<InputTransportPumpLease> retired_pump;
  std::shared_ptr<InputTransport> retired_transport;
  std::shared_ptr<ClaimedInputTransportPump::Control::PumpBridge>
      retired_bridge;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->settled || !control->closed || !control->claim_released)
      return;
    control->settled = true;
    control->state = ClaimedPumpState::kSettled;
    observer = control->observer;
    retired_pump = std::move(control->pump);
    retired_transport = std::move(control->transport);
    retired_bridge = std::move(control->bridge);
  }
  Unlink(control);
  if (observer.on_state != nullptr) {
    try {
      observer.on_state(observer.context, ClaimedPumpState::kSettled);
    } catch (...) {
    }
  }
}

void TaskQuiescent(void* context) noexcept {
  auto* token = static_cast<ClaimedInputTransportPump::Control::Token*>(context);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control != nullptr) Settle(control);
}

void Drive(const std::shared_ptr<ClaimedInputTransportPump::Control>& control);

void TaskCallback(void* context) noexcept {
  auto* token = static_cast<ClaimedInputTransportPump::Control::Token*>(context);
  if (token == nullptr) return;
  const auto control = token->control.lock();
  if (control != nullptr) Drive(control);
}

void MaybeReleaseAndCancel(
    const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  std::shared_ptr<InputTransportPumpLease> pump;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  bool release = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->claim_released || control->release_inflight) return;
    pump = control->pump;
    if (!control->claim) return;
    control->release_inflight = true;
    release = true;
  }
  // IsQuiescent is an independent lifecycle lock; never call it while
  // holding the coordinator mutex because provider callbacks can reenter.
  if (pump != nullptr && !pump->IsQuiescent()) {
    std::lock_guard<std::mutex> lock(control->mutex);
    control->release_inflight = false;
    return;
  }
  if (release) {
    bool released = false;
    {
      // Claim release may synchronously notify the next authority owner; no
      // coordinator lock is held across it.
      released = control->claim.ReleaseAfterQuiescence();
    }
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->release_inflight = false;
      if (released) {
        control->claim_released = true;
        task = control->task;
      }
    }
    if (released) {
      if (task != nullptr) (void)task->Cancel();
      Settle(control);
    }
  }
}

void RetryFailedRetire(
    const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  bool retry = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (!control->retire_retry_requested) {
      control->retire_retry_requested = true;
      retry = true;
    }
  }
  if (retry) (void)RequestService(control);
}

void RetirePump(
    const std::shared_ptr<ClaimedInputTransportPump::Control>& control,
    const std::shared_ptr<InputTransportPumpLease>& pump) {
  if (pump != nullptr && !pump->Retire()) RetryFailedRetire(control);
}

void Drive(const std::shared_ptr<ClaimedInputTransportPump::Control>& control) {
  if (darwin_art::looper::Current() != control->looper) {
    (void)RequestService(control);
    return;
  }
  bool reentrant = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->driving) {
      reentrant = true;
    } else {
      control->driving = true;
    }
  }
  if (reentrant) {
    (void)RequestService(control);
    return;
  }
  struct DriveGuard {
    std::shared_ptr<ClaimedInputTransportPump::Control> control;
    ~DriveGuard() {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->driving = false;
    }
  } guard;
  guard.control = control;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    // A failed exact removal gets at most one owner-task retry per dispatch;
    // the flag is re-elected on the next dispatch rather than recursively
    // spinning inside this Drive call.
    control->retire_retry_requested = false;
  }

  std::shared_ptr<InputTransport> transport;
  std::shared_ptr<ClaimedInputTransportPump::Control::PumpBridge> bridge;
  std::shared_ptr<InputTransportPumpLease> pump;
  InputTransportReaderCallback reader_callback = nullptr;
  void* reader_context = nullptr;
  std::shared_ptr<void> reader_owner;
  int events = 0;
  std::uint64_t register_revision = 0;
  bool writable_dirty = false;
  bool should_retire = false;
  bool should_activate = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->settled || !control->activated) return;
    should_retire = control->closed;
    pump = control->pump;
    if (!should_retire && pump == nullptr && !control->claim_admitted) {
      should_activate = true;
      transport = control->transport;
      bridge = control->bridge;
      reader_callback = control->reader_callback;
      reader_context = control->reader_context;
      reader_owner = control->reader_owner;
      events = control->base_events | (control->writable ? kOutputEvent : 0);
      register_revision = control->writable_revision;
    }
  }

  if (should_retire) {
    RetirePump(control, pump);
    MaybeReleaseAndCancel(control);
    return;
  }

  if (should_activate) {
    const auto result = control->claim.BeginRegistration();
    if (result == TransportRegistrationResult::kDeferred) {
      SetState(control, ClaimedPumpState::kWaiting);
      SubscribeAvailable(control);
      return;
    }
    if (result != TransportRegistrationResult::kAcquired) {
      MarkRetiring(control, true);
      MaybeReleaseAndCancel(control);
      return;
    }
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->claim_admitted = true;
      if (control->closed) should_retire = true;
    }
      if (!should_retire) {
        std::shared_ptr<InputTransportPumpLease> raw;
        InputTransportPumpCallbacks callbacks = bridge->user;
      callbacks.on_packet = &ProxyPacket;
      callbacks.on_window = &ProxyWindow;
      if (bridge->user.on_packet_consumption != nullptr) {
        callbacks.on_packet = nullptr;
        callbacks.on_packet_consumption = &ProxyPacketConsumption;
      }
      if (bridge->user.on_window_consumption != nullptr) {
        callbacks.on_window = nullptr;
        callbacks.on_window_consumption = &ProxyWindowConsumption;
      }
      callbacks.on_ack = &ProxyAck;
      callbacks.on_ack64 = &ProxyAck64;
      callbacks.on_progress = &ProxyProgress;
      callbacks.context = bridge.get();
      callbacks.context_owner = bridge;
      callbacks.on_terminal = &ProxyTerminal;
      callbacks.on_quiescent = &ProxyQuiescent;
      bool registered = false;
      try {
        raw = std::make_shared<InputTransportPumpLease>();
        registered = reader_callback == nullptr
                         ? raw->Register(control->looper, transport, control->fd,
                                         events, callbacks, control->fd_callback,
                                         control->fd_context, control->fd_owner)
                         : raw->RegisterReader(
                               control->looper, transport, control->fd, events,
                               callbacks, reader_callback, reader_context,
                               reader_owner);
      } catch (...) {
        registered = false;
      }
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        control->pump = std::move(raw);
        if (registered && control->writable_revision == register_revision)
          control->applied_writable_revision = register_revision;
        if (!registered || control->closed) {
          control->failed = true;
          control->closed = true;
          should_retire = true;
        }
      }
      if (!registered) MaybeReleaseAndCancel(control);
      else if (!should_retire) {
        SetState(control, ClaimedPumpState::kActive);
        if (control->role == TransportRegistrationRole::kRetiredOutput)
          SubscribeReceiverWaiting(control);
      }
    }
    if (should_retire) {
      std::shared_ptr<InputTransportPumpLease> to_retire;
      {
        std::lock_guard<std::mutex> lock(control->mutex);
        to_retire = control->pump;
      }
      RetirePump(control, to_retire);
      MaybeReleaseAndCancel(control);
      return;
    }
  }

  {
    std::lock_guard<std::mutex> lock(control->mutex);
    pump = control->pump;
    should_retire = control->closed;
    writable_dirty = control->writable_revision !=
                     control->applied_writable_revision;
  }
  if (should_retire) RetirePump(control, pump);
  if (should_retire) {
    MaybeReleaseAndCancel(control);
    return;
  }

  if (pump != nullptr && writable_dirty) {
    bool enabled = false;
    std::uint64_t revision = 0;
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      enabled = control->writable;
      revision = control->writable_revision;
    }
    const auto result = pump->SetWritableResult(enabled);
    if (result == InputTransportWritableResult::kApplied) {
      std::lock_guard<std::mutex> lock(control->mutex);
      if (control->writable_revision == revision)
        control->applied_writable_revision = revision;
    } else if (result == InputTransportWritableResult::kTerminal) {
      MarkRetiring(control, true);
      RetirePump(control, pump);
      MaybeReleaseAndCancel(control);
      return;
    }
  }

  // A retired-output owner yields only after a receiver intent is rechecked
  // on this owner looper. The authority hint itself is advisory.
  std::shared_ptr<InputTransport> current_transport;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    current_transport = control->transport;
  }
  if (current_transport != nullptr &&
      control->role == TransportRegistrationRole::kRetiredOutput &&
      current_transport->RegistrationAuthority().HasLiveIntent(control->fd)) {
    MarkRetiring(control, false);
    RetirePump(control, pump);
    MaybeReleaseAndCancel(control);
    return;
  }
}

}  // namespace

std::shared_ptr<ClaimedInputTransportPump>
ClaimedInputTransportPump::Prepare(
    void* looper, std::shared_ptr<InputTransport> transport, int fd, int events,
    TransportRegistrationRole role, std::uint64_t identity,
    const InputTransportPumpCallbacks& callbacks,
    darwin_art::looper::FdCallback fd_callback, void* fd_context,
    std::shared_ptr<void> fd_owner, const ClaimedPumpObserver& observer) {
  return PrepareInternal(looper, std::move(transport), fd, events, role,
                         identity, callbacks, nullptr, nullptr, {},
                         fd_callback, fd_context, std::move(fd_owner), observer);
}

std::shared_ptr<ClaimedInputTransportPump>
ClaimedInputTransportPump::PrepareReader(
    void* looper, std::shared_ptr<InputTransport> transport, int fd, int events,
    TransportRegistrationRole role, std::uint64_t identity,
    const InputTransportPumpCallbacks& callbacks,
    InputTransportReaderCallback reader_callback, void* reader_context,
    std::shared_ptr<void> reader_owner,
    const ClaimedPumpObserver& observer) {
  if (reader_callback == nullptr || (events & 0x0001) == 0) return {};
  return PrepareInternal(looper, std::move(transport), fd, events, role,
                         identity, callbacks, reader_callback, reader_context,
                         std::move(reader_owner), nullptr, nullptr, {},
                         observer);
}

std::shared_ptr<ClaimedInputTransportPump>
ClaimedInputTransportPump::PrepareInternal(
    void* looper, std::shared_ptr<InputTransport> transport, int fd, int events,
    TransportRegistrationRole role, std::uint64_t identity,
    const InputTransportPumpCallbacks& callbacks,
    InputTransportReaderCallback reader_callback, void* reader_context,
    std::shared_ptr<void> reader_owner,
    darwin_art::looper::FdCallback fd_callback, void* fd_context,
    std::shared_ptr<void> fd_owner, const ClaimedPumpObserver& observer) {
  if (looper == nullptr || transport == nullptr || fd < 0 || identity == 0 ||
      darwin_art::looper::Current() != looper)
    return {};
  if ((callbacks.on_packet != nullptr && callbacks.on_packet_consumption != nullptr) ||
      (callbacks.on_window != nullptr && callbacks.on_window_consumption != nullptr))
    return {};
  std::shared_ptr<Control> control;
  std::shared_ptr<ClaimedInputTransportPump> wrapper;
  try {
    control = std::make_shared<Control>(looper, fd, events, role, identity);
    control->transport = transport;
    control->observer = observer;
    control->token = std::make_shared<Control::Token>();
    control->token->control = control;
    control->bridge = std::make_shared<Control::PumpBridge>();
    control->bridge->control = control;
    control->bridge->user = callbacks;
    control->fd_callback = fd_callback;
    control->fd_context = fd_context;
    control->fd_owner = std::move(fd_owner);
    control->reader_callback = reader_callback;
    control->reader_context = reader_context;
    control->reader_owner = std::move(reader_owner);
    control->task = darwin_art::looper::ReusableLooperTask::Prepare(
        looper,
        darwin_art::looper::ReusableLooperTaskCallbacks{
            &TaskCallback, control->token.get(), control->token, &TaskQuiescent});
    wrapper = std::make_shared<ClaimedInputTransportPump>(control);
  } catch (const std::bad_alloc&) {
    if (control != nullptr) {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->settled = true;
      control->state = ClaimedPumpState::kSettled;
    }
    wrapper.reset();
    if (control != nullptr && control->task != nullptr)
      (void)control->task->Cancel();
    return {};
  }
  if (control->task == nullptr) {
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->settled = true;
      control->state = ClaimedPumpState::kSettled;
    }
    wrapper.reset();
    return {};
  }

  try {
    Enlist(control);
  } catch (const std::bad_alloc&) {
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->settled = true;
      control->state = ClaimedPumpState::kSettled;
    }
    wrapper.reset();
    (void)control->task->Cancel();
    return {};
  }

  const auto result = transport->RegistrationAuthority().Reserve(
      fd, role, identity, looper, &control->claim);
  if (result != TransportRegistrationResult::kAcquired &&
      result != TransportRegistrationResult::kDeferred) {
    {
      std::lock_guard<std::mutex> lock(control->mutex);
      control->closed = true;
      control->settled = true;
      control->state = ClaimedPumpState::kSettled;
    }
    wrapper.reset();
    (void)control->task->Cancel();
    Unlink(control);
    return {};
  }
  if (result == TransportRegistrationResult::kDeferred)
    SetState(control, ClaimedPumpState::kWaiting);
  else
    SetState(control, ClaimedPumpState::kPrepared);
  // Keep the caller-supplied FD callback and owner in the raw registration;
  // the bridge is only for transport policy callbacks.
  return wrapper;
}

ClaimedInputTransportPump::~ClaimedInputTransportPump() { (void)Retire(); }

bool ClaimedInputTransportPump::Activate() {
  const auto control = control_;
  if (control == nullptr) return false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->settled || control->closed) return false;
    control->activated = true;
  }
  if (darwin_art::looper::Current() == control->looper)
    Drive(control);
  else
    (void)RequestService(control);
  return true;
}

ClaimedPumpState ClaimedInputTransportPump::State() const {
  const auto control = control_;
  if (control == nullptr) return ClaimedPumpState::kSettled;
  std::lock_guard<std::mutex> lock(control->mutex);
  return control->state;
}

bool ClaimedInputTransportPump::Failed() const {
  const auto control = control_;
  if (control == nullptr) return true;
  std::lock_guard<std::mutex> lock(control->mutex);
  return control->failed;
}

InputTransportWritableResult ClaimedInputTransportPump::SetWritableResult(
    bool enabled) {
  const auto control = control_;
  if (control == nullptr) return InputTransportWritableResult::kTerminal;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    if (control->settled || control->closed)
      return InputTransportWritableResult::kTerminal;
    control->writable = enabled;
    ++control->writable_revision;
    if (control->state != ClaimedPumpState::kActive || control->pump == nullptr)
      return InputTransportWritableResult::kDeferred;
  }
  if (darwin_art::looper::Current() != control->looper) {
    (void)RequestService(control);
    return InputTransportWritableResult::kDeferred;
  }
  // Drive is the sole owner-looper operation path, including reentrant calls.
  Drive(control);
  std::lock_guard<std::mutex> lock(control->mutex);
  if (control->closed || control->settled)
    return InputTransportWritableResult::kTerminal;
  return control->applied_writable_revision == control->writable_revision
             ? InputTransportWritableResult::kApplied
             : InputTransportWritableResult::kDeferred;
}

bool ClaimedInputTransportPump::SetWritable(bool enabled) {
  return SetWritableResult(enabled) == InputTransportWritableResult::kApplied;
}

bool ClaimedInputTransportPump::Retire() {
  const auto control = control_;
  if (control == nullptr) return true;
  bool already = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    already = control->settled;
  }
  if (already) return true;
  MarkRetiring(control, false);
  if (darwin_art::looper::Current() == control->looper)
    Drive(control);
  else
    (void)RequestService(control);
  return true;
}

bool ClaimedInputTransportPump::IsQuiescent() const {
  const auto control = control_;
  if (control == nullptr) return true;
  std::shared_ptr<InputTransportPumpLease> pump;
  std::shared_ptr<darwin_art::looper::ReusableLooperTask> task;
  bool closed = false;
  bool released = false;
  {
    std::lock_guard<std::mutex> lock(control->mutex);
    closed = control->closed;
    released = control->claim_released;
    pump = control->pump;
    task = control->task;
  }
  return closed && released && (pump == nullptr || pump->IsQuiescent()) &&
         (task == nullptr || task->IsQuiescent());
}

}  // namespace darwin_art::input
