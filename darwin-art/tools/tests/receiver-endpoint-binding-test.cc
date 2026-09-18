#include "runtime/framework/input/receiver_endpoint_binding.h"
#include "compat/looper/reusable_task.h"

#include <algorithm>
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <memory>
#include <new>
#include <sys/socket.h>
#include <vector>

using Binding = darwin_art::input::ReceiverEndpointBinding;
using Callbacks = darwin_art::input::ReceiverEndpointBindingCallbacks;
using EventResult = darwin_art::input::ReceiverEndpointBindingEventResult;
using Slot = darwin_art::input::ReceiverEndpointBindingSlot;
using Status = darwin_art::input::ReceiverEndpointBindingStatus;
using Transport = darwin_art::input::InputTransport;

namespace {

darwin_art::looper::detail::ReusableTaskQueue* reusable_queue = nullptr;

struct Registration {
  int fd = -1;
  int events = 0;
  int (*callback)(int, int, void*) = nullptr;
  void* context = nullptr;
  void (*release)(void*) = nullptr;
};

std::vector<Registration> registrations;
Binding* binding_during_add = nullptr;
darwin_art::input::InputTransportPumpLease* pump_during_add = nullptr;
Binding* binding_during_remove = nullptr;
Binding* refresh_during_remove = nullptr;
int dispose_add_count = 0;
int dispose_remove_count = 0;
int add_failures = 0;
int remove_failures = 0;
bool refresh_value_during_remove = true;
Slot refresh_slot_during_remove = Slot::kLocalWake;

enum class SendMode { kAccept, kBlocked, kTerminal };
SendMode send_mode = SendMode::kAccept;
std::vector<uint8_t> sent_bytes;
int send_calls = 0;
bool receive_eof = false;

intptr_t FakeSend(int, const void* data, size_t size, int) {
  ++send_calls;
  if (send_mode == SendMode::kBlocked) {
    errno = EAGAIN;
    return -1;
  }
  if (send_mode == SendMode::kTerminal) {
    errno = EPIPE;
    return -1;
  }
  const auto* bytes = static_cast<const uint8_t*>(data);
  sent_bytes.insert(sent_bytes.end(), bytes, bytes + size);
  return static_cast<intptr_t>(size);
}
intptr_t FakeReceive(int, void*, size_t, int) {
  if (receive_eof) return 0;
  errno = EAGAIN;
  return -1;
}
int FakeClose(int) { return 0; }
int FakeError() { return errno == EAGAIN || errno == EWOULDBLOCK ? 11 : errno; }

struct CallbackState {
  Binding* binding = nullptr;
  Transport* transport = nullptr;
  const Binding::RetirementHandle* retirement = nullptr;
  bool dispose_on_event = false;
  bool terminal_on_event = false;
  bool complete_on_event = false;
  bool dispose_on_status = false;
  bool throw_on_event = false;
  bool refresh_on_event = false;
  Slot refresh_slot = Slot::kLocalWake;
  int refresh_fd = -1;
  bool refresh_result = false;
  std::vector<Status> statuses;
  int events = 0;
  int failures = 0;
  int retired = 0;
};

int FakeAddFdOwned(void*, int fd, int, int events,
                   int (*callback)(int, int, void*),
                   void* context, void*, void (*release)(void*)) {
  if (add_failures > 0) {
    --add_failures;
    return -1;
  }
  registrations.push_back(Registration{fd, events, callback, context, release});
  if (pump_during_add != nullptr) {
    auto* pump = std::exchange(pump_during_add, nullptr);
    assert(pump->Retire());
    assert(!pump->IsQuiescent());
  }
  if (binding_during_add != nullptr && dispose_add_count > 0) {
    --dispose_add_count;
    assert(binding_during_add->Retire());
  }
  return 1;
}

int RemoveFdIfOwned(void*, int fd, int (*callback)(int, int, void*),
                    void* context) {
  if (remove_failures > 0) {
    --remove_failures;
    return -1;
  }
  const auto found = std::find_if(
      registrations.begin(), registrations.end(), [&](const auto& candidate) {
        return candidate.fd == fd && candidate.callback == callback &&
               candidate.context == context;
      });
  if (found == registrations.end()) return 0;
  if (binding_during_remove != nullptr && dispose_remove_count > 0) {
    --dispose_remove_count;
    // The binding owns an operation reservation while RemoveFdIfOwned is in
    // progress. Reentrant retirement must defer rather than claim this pump.
    assert(binding_during_remove->Retire());
  }
  if (refresh_during_remove != nullptr) {
    Binding* target = refresh_during_remove;
    refresh_during_remove = nullptr;
    assert(!target->RefreshWritable(refresh_slot_during_remove, fd,
                                    refresh_value_during_remove));
  }
  const auto release = found->release;
  void* registration_context = found->context;
  registrations.erase(found);
  release(registration_context);
  return 1;
}

bool Fire(int fd, int events, int delivered_fd = -1) {
  const auto found = std::find_if(
      registrations.begin(), registrations.end(),
      [&](const auto& candidate) { return candidate.fd == fd; });
  if (found == registrations.end()) return false;
  auto* registration = &*found;
  (void)registration->callback(delivered_fd < 0 ? fd : delivered_fd, events,
                               registration->context);
  for (int i = 0; i < 16; ++i)
    (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  return true;
}

EventResult OnEvent(void* opaque, Slot, int fd, int) {
  auto* state = static_cast<CallbackState*>(opaque);
  ++state->events;
  if (state->complete_on_event) {
    assert(darwin_art::input::PumpInputTransport(state->transport, fd, {}) ==
           darwin_art::input::InputTransportStatus::kTerminal);
    return EventResult::kReaderComplete;
  }
  if (state->throw_on_event) {
    state->throw_on_event = false;
    throw std::bad_alloc();
  }
  if (state->dispose_on_event) {
    state->dispose_on_event = false;
    assert(state->binding->Retire());
    assert(state->retired == 0);
  }
  if (state->refresh_on_event) {
    state->refresh_on_event = false;
    state->refresh_result = state->binding->RefreshWritable(
        state->refresh_slot, state->refresh_fd, true);
  }
  return state->terminal_on_event ? EventResult::kTerminal
                                  : EventResult::kKeep;
}

void OnFailure(void* opaque) noexcept {
  auto* state = static_cast<CallbackState*>(opaque);
  ++state->failures;
}

void OnRetired(void* opaque) noexcept {
  ++static_cast<CallbackState*>(opaque)->retired;
}

void OnProofRetired(void* opaque) noexcept {
  auto* state = static_cast<CallbackState*>(opaque);
  assert(state->transport != nullptr);
  assert(state->retirement != nullptr);
  // Both slots were prepared before local publication.  Completion is the
  // first point at which the claimed wrappers promise that both reservations
  // and their reusable-task tails are gone.
  assert(!state->transport->RegistrationAuthority().HasLiveIntent(900));
  assert(!state->transport->RegistrationAuthority().HasLiveIntent(901));
  assert(state->retirement->IsQuiescent());
  ++state->retired;
}

struct DestroyedBindingState {
  std::unique_ptr<Binding>* binding;
  const Binding::RetirementHandle* handle;
  int retired = 0;
};
EventResult DestroyBindingOnEvent(void* opaque, Slot, int, int) {
  auto& state = *static_cast<DestroyedBindingState*>(opaque);
  state.binding->reset();
  assert(!state.handle->IsQuiescent());
  assert(state.retired == 0);
  return EventResult::kKeep;
}
void RetiredDestroyedBinding(void* opaque) noexcept {
  auto& state = *static_cast<DestroyedBindingState*>(opaque);
  assert(*state.binding == nullptr);
  assert(state.handle->IsQuiescent());
  ++state.retired;
}

struct NotificationDestruction {
  std::unique_ptr<Binding>* binding;
  std::unique_ptr<Binding::RetirementHandle>* handle;
  int calls = 0;
};
void DestroyRetirementOwners(void* opaque) noexcept {
  auto& state = *static_cast<NotificationDestruction*>(opaque);
  ++state.calls;
  state.binding->reset();
  state.handle->reset();
}

void ThrowingStatus(void*, Status) { throw std::bad_alloc(); }

void OnStatus(void* opaque, Status status) {
  auto* state = static_cast<CallbackState*>(opaque);
  state->statuses.push_back(status);
  if (state->dispose_on_status) {
    state->dispose_on_status = false;
    assert(state->binding->Retire());
  }
}

void InstallLooperSymbols() {
  reusable_queue = darwin_art::looper::detail::CreateReusableTaskQueue();
  assert(reusable_queue != nullptr);
}

}  // namespace

namespace darwin_art::looper {
void* Current() { return reinterpret_cast<void*>(0x1); }
int SignalWake(void*) { return 0; }
int AddFdOwned(void* looper, int fd, int flags, int events,
               FdCallback callback, void* context, void* owner,
               OwnerRelease release) {
  return ::FakeAddFdOwned(looper, fd, flags, events, callback, context, owner,
                           release);
}
int RemoveFdIfOwned(void* looper, int fd, FdCallback callback, void* context) {
  return ::RemoveFdIfOwned(looper, fd, callback, context);
}
}  // namespace darwin_art::looper

namespace darwin_art::looper::detail {
ReusableTaskQueue* ReusableTaskQueueForLooper(void*) { return reusable_queue; }
}  // namespace darwin_art::looper::detail

extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int,
                                                             int*) {
  return -1;
}
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int, const void*,
                                                           size_t, int) {
  return -1;
}
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int, void*, size_t,
                                                           int) {
  return -1;
}
extern "C" int darwin_art_bionic_socket_broker_close(int) { return 0; }
extern "C" int darwin_art_bionic_errno_load() { return 11; }

int main() {
  InstallLooperSymbols();
  auto transport = std::make_shared<Transport>(
      darwin_art::input::InputTransportIo{FakeSend, FakeReceive, FakeClose,
                                          FakeError}, false);
  darwin_art::input::AdoptRemoteInputTransport(transport.get(), 81);
  const auto callbacks = Callbacks{OnEvent, OnStatus, nullptr,
                                   std::make_shared<int>(7)};
  auto state_owner = std::make_shared<CallbackState>();
  auto& state = *state_owner;
  Binding binding;
  state.binding = &binding;
  auto callback_set = callbacks;
  callback_set.context = &state;
  callback_set.context_owner = state_owner;
  {
    Binding prepared;
    auto prepared_state_owner = std::make_shared<CallbackState>();
    auto& prepared_state = *prepared_state_owner;
    prepared_state.binding = &prepared;
    auto prepared_callbacks = callback_set;
    prepared_callbacks.context = &prepared_state;
    prepared_callbacks.context_owner = prepared_state_owner;
    assert(!prepared.Activate());
    // Invalid preparation must neither consume the binding nor expose either
    // slot. A subsequent valid preparation still owns the original pair.
    assert(!prepared.Prepare(nullptr, transport, 16, 17,
                             prepared_callbacks, 490, 8));
    assert(!prepared.Prepare(reinterpret_cast<void*>(0x1), {}, 16, 17,
                             prepared_callbacks, 490, 8));
    assert(!prepared.Prepare(reinterpret_cast<void*>(0x1), transport, -1, 17,
                             prepared_callbacks, 490, 8));
    assert(!prepared.Prepare(reinterpret_cast<void*>(0x1), transport, 16, -2,
                             prepared_callbacks, 490, 8));
    assert(!prepared.Prepare(reinterpret_cast<void*>(0x1), transport, 16, 17,
                             prepared_callbacks, 0, 8));
    assert(registrations.empty() && prepared_state.statuses.empty());
    assert(prepared.Prepare(reinterpret_cast<void*>(0x1), transport, 16, 17,
                            prepared_callbacks, 490, 8));
    assert(!prepared.Prepare(reinterpret_cast<void*>(0x1), transport, 18, 19,
                             prepared_callbacks, 491, 9));
    assert(registrations.empty());
    assert(!prepared.Status().local_ready && !prepared.Status().remote_ready);
    assert(prepared_state.statuses.empty());
    assert(prepared.Retire());
    assert(prepared.RetainRetirement().IsQuiescent());
    assert(!prepared.Activate() && registrations.empty());
  }
  assert(binding.Prepare(reinterpret_cast<void*>(0x1), transport, 10, 11,
                         callback_set, 501, 9));
  assert(registrations.empty() && state.statuses.empty());
  assert(binding.Activate());
  assert(!binding.Activate());
  assert(binding.Status().local_ready && binding.Status().remote_ready);
  assert(state.statuses.size() >= 2);
  assert(state.statuses.front().local_ready &&
         !state.statuses.front().remote_ready);
  assert(state.statuses.back().local_ready &&
         state.statuses.back().remote_ready);
  for (size_t i = 1; i < state.statuses.size(); ++i)
    assert(state.statuses[i - 1].revision < state.statuses[i].revision);
  assert(binding.RefreshWritable(Slot::kLocalWake, 10, true));
  assert(!binding.RefreshWritable(Slot::kLocalWake, 99, true));
  assert(Fire(10, 1) && state.events == 1);
  assert(binding.Retire());
  assert(binding.Status().closed);

  // A queued ACK is retained by the transport while the provider reports
  // guest EAGAIN. Refreshing the actual binding slot must rearm OUTPUT even
  // when Remove/Add re-enters RefreshWritable; the subsequent writable event
  // flushes the ACK once before dispatching the user callback.
  auto ack_transport = std::make_shared<Transport>(
      darwin_art::input::InputTransportIo{FakeSend, FakeReceive, FakeClose,
                                          FakeError}, false);
  darwin_art::input::AdoptRemoteInputTransport(ack_transport.get(), 81);
  Binding writable;
  auto writable_state_owner = std::make_shared<CallbackState>();
  auto& writable_state = *writable_state_owner;
  writable_state.binding = &writable;
  auto writable_callbacks = callback_set;
  writable_callbacks.context = &writable_state;
  writable_callbacks.context_owner = writable_state_owner;
  send_mode = SendMode::kBlocked;
  sent_bytes.clear();
  send_calls = 0;
  assert(writable.Register(reinterpret_cast<void*>(0x1), ack_transport, 200, 81,
                           writable_callbacks, 520, 30));
  assert(darwin_art::input::SendInputTransportAck(ack_transport.get(), 700, true) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(ack_transport->HasPendingTx());
  // The provider-side reentry requests the opposite value. The outer call
  // must not retry its stale `true` after the wrapper has observed `false`.
  refresh_value_during_remove = false;
  refresh_slot_during_remove = Slot::kRemote;
  refresh_during_remove = &writable;
  assert(!writable.RefreshWritable(Slot::kRemote, 81, true));
  refresh_value_during_remove = true;
  for (int i = 0; i < 4; ++i)
    (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  const auto writable_registration = std::find_if(
      registrations.begin(), registrations.end(),
      [](const auto& registration) { return registration.fd == 81; });
  assert(writable_registration != registrations.end());
  assert((writable_registration->events & 0x0002) == 0);
  send_mode = SendMode::kAccept;
  assert(Fire(81, 0x0002));
  assert(writable_state.events == 1);
  assert(send_calls == 2);
  assert(!ack_transport->HasPendingTx() && !sent_bytes.empty());
  assert(writable.Retire());

  // A terminal TX flush must not revoke the remote slot's independent RX.
  // The original readiness callback can drain final RX; failed TX retains no
  // writable interest and cannot synthesize an ERROR receiver event.
  Binding terminal_flush;
  auto terminal_flush_state_owner = std::make_shared<CallbackState>();
  auto& terminal_flush_state = *terminal_flush_state_owner;
  terminal_flush_state.binding = &terminal_flush;
  auto terminal_flush_callbacks = callback_set;
  terminal_flush_callbacks.context = &terminal_flush_state;
  terminal_flush_callbacks.context_owner = terminal_flush_state_owner;
  send_mode = SendMode::kBlocked;
  sent_bytes.clear();
  assert(terminal_flush.Register(reinterpret_cast<void*>(0x1), ack_transport, 210,
                                 81, terminal_flush_callbacks, 521, 31));
  assert(darwin_art::input::SendInputTransportAck(ack_transport.get(), 701, false) ==
         darwin_art::input::InputTransportStatus::kAccepted);
  assert(terminal_flush.RefreshWritable(Slot::kRemote, 81, true));
  send_mode = SendMode::kTerminal;
  assert(Fire(81, 0x0002));
  assert(terminal_flush_state.events == 1);
  assert(terminal_flush.Status().remote_ready);
  assert(ack_transport->IsTxTerminal() && !ack_transport->IsRxTerminal());
  assert(terminal_flush.RefreshWritable(Slot::kRemote, 81, true));
  const auto rx_registration = std::find_if(
      registrations.begin(), registrations.end(),
      [](const auto& registration) { return registration.fd == 81; });
  assert(rx_registration != registrations.end());
  assert((rx_registration->events & 0x0001) != 0 &&
         (rx_registration->events & 0x0002) == 0);
  assert(terminal_flush.Retire());
  send_mode = SendMode::kAccept;

  // Reader completion changes readiness without releasing the retained pump.
  // Its private writable turn must flush the ACK without calling the reader.
  {
    auto stream = std::make_shared<Transport>(
        darwin_art::input::InputTransportIo{FakeSend, FakeReceive, FakeClose,
                                           FakeError}, false);
    assert(darwin_art::input::AdoptRemoteInputTransport(stream.get(), 82));
    Binding completed;
    auto owner = std::make_shared<CallbackState>();
    owner->binding = &completed;
    owner->transport = stream.get();
    owner->complete_on_event = true;
    auto cb = callback_set;
    cb.context = owner.get();
    cb.context_owner = owner;
    cb.on_retired = OnRetired;
    assert(completed.Register(reinterpret_cast<void*>(1), stream, 211, 82,
                              cb, 522, 32));
    send_mode = SendMode::kBlocked;
    sent_bytes.clear();
    assert(darwin_art::input::SendInputTransportAck(stream.get(), 702, true) ==
           darwin_art::input::InputTransportStatus::kAccepted);
    const auto fence = stream->CaptureAcceptedTxFence();
    receive_eof = true;
    assert(Fire(82, 0x0001));
    receive_eof = false;
    assert(owner->events == 1 && owner->retired == 0);
    assert(!completed.Status().remote_ready && completed.Status().local_ready);
    assert(stream->IsRxTerminal() && !stream->IsTxTerminal());
    assert(stream->QueryTxFence(fence) ==
           darwin_art::input::InputTransportTxFenceStatus::kPending);
    // A stale disable cannot strand accepted TX or restore reader readiness.
    (void)completed.RefreshWritable(Slot::kRemote, 82, false);
    send_mode = SendMode::kAccept;
    // Stale RX-HUP is not a TX failure; the real writer settles direction.
    assert(Fire(82, 0x0002 | 0x0008));
    assert(owner->events == 1 && !stream->HasPendingTx());
    assert(stream->QueryTxFence(fence) ==
           darwin_art::input::InputTransportTxFenceStatus::kFlushed);
    assert(!sent_bytes.empty() && !completed.Status().remote_ready);
    assert(completed.Retire());
    // Retirement acceptance is not claimed-task-tail quiescence.
    for (int i = 0; i < 16; ++i)
      (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
    assert(completed.RetainRetirement().IsQuiescent());
    assert(owner->retired == 1);
  }

  // A status observer may explicitly dispose at the reader-completion edge.
  // The completed-RX result must not revive that exact registration.
  {
    auto stream = std::make_shared<Transport>(
        darwin_art::input::InputTransportIo{FakeSend, FakeReceive, FakeClose,
                                           FakeError}, false);
    assert(darwin_art::input::AdoptRemoteInputTransport(stream.get(), 83));
    Binding disposed;
    auto owner = std::make_shared<CallbackState>();
    owner->binding = &disposed;
    owner->transport = stream.get();
    owner->complete_on_event = true;
    auto cb = callback_set;
    cb.context = owner.get();
    cb.context_owner = owner;
    cb.on_retired = OnRetired;
    assert(disposed.Register(reinterpret_cast<void*>(1), stream, 212, 83,
                             cb, 523, 33));
    send_mode = SendMode::kBlocked;
    sent_bytes.clear();
    assert(darwin_art::input::SendInputTransportAck(stream.get(), 703, true) ==
           darwin_art::input::InputTransportStatus::kAccepted);
    owner->dispose_on_status = true;
    receive_eof = true;
    assert(Fire(83, 0x0001));
    receive_eof = false;
    assert(disposed.Status().closed && owner->events == 1 && owner->retired == 1);
    assert(stream->HasPendingTx() && sent_bytes.empty());
    assert(!Fire(83, 0x0002));
    send_mode = SendMode::kAccept;
  }

  Binding local_only;
  auto local_only_state_owner = std::make_shared<CallbackState>();
  auto& local_only_state = *local_only_state_owner;
  local_only_state.binding = &local_only;
  auto local_only_callbacks = callback_set;
  local_only_callbacks.context = &local_only_state;
  local_only_callbacks.context_owner = local_only_state_owner;
  assert(local_only.Register(reinterpret_cast<void*>(0x1), transport, 60, -1,
                             local_only_callbacks, 506, 14));
  assert(local_only.Status().local_ready &&
         !local_only.Status().remote_ready);
  assert(local_only.Retire());

  Binding same_fd;
  auto same_fd_state_owner = std::make_shared<CallbackState>();
  auto& same_fd_state = *same_fd_state_owner;
  same_fd_state.binding = &same_fd;
  auto same_fd_callbacks = callback_set;
  same_fd_callbacks.context = &same_fd_state;
  same_fd_callbacks.context_owner = same_fd_state_owner;
  assert(same_fd.Register(reinterpret_cast<void*>(0x1), transport, 70, 70,
                          same_fd_callbacks, 507, 15));
  assert(same_fd.Status().local_ready && !same_fd.Status().remote_ready);
  assert(same_fd.Retire());

  // A failed local provider add does not poison a valid remote slot.
  Binding remote_only;
  auto remote_only_state_owner = std::make_shared<CallbackState>();
  auto& remote_only_state = *remote_only_state_owner;
  remote_only_state.binding = &remote_only;
  auto remote_only_callbacks = callback_set;
  remote_only_callbacks.context = &remote_only_state;
  remote_only_callbacks.context_owner = remote_only_state_owner;
  add_failures = 1;
  assert(remote_only.Register(reinterpret_cast<void*>(0x1), transport, 80, 81,
                              remote_only_callbacks, 508, 16));
  assert(!remote_only.Status().local_ready &&
         remote_only.Status().remote_ready);
  assert(remote_only.Retire());

  // A reentrant Retire from the local-failure status edge prevents the
  // optional remote slot from being registered after the binding is closed.
  Binding failed_closed;
  auto failed_closed_state_owner = std::make_shared<CallbackState>();
  auto& failed_closed_state = *failed_closed_state_owner;
  failed_closed_state.binding = &failed_closed;
  failed_closed_state.dispose_on_status = true;
  auto failed_closed_callbacks = callback_set;
  failed_closed_callbacks.context = &failed_closed_state;
  failed_closed_callbacks.context_owner = failed_closed_state_owner;
  const auto registrations_before_failed_closed = registrations.size();
  add_failures = 1;
  assert(!failed_closed.Register(reinterpret_cast<void*>(0x1), transport, 82,
                                 83, failed_closed_callbacks, 513, 21));
  assert(failed_closed.Status().closed);
  assert(registrations.size() == registrations_before_failed_closed);
  assert(failed_closed.Retire());

  // Both slots are prepared before local publication. If the local ready
  // status callback retires the binding, the preprepared remote wrapper must
  // remain retained until its claim and reusable-task tail are quiescent.
  auto proof_binding = std::make_unique<Binding>();
  auto proof_retirement = proof_binding->RetainRetirement();
  auto proof_state_owner = std::make_shared<CallbackState>();
  auto& proof_state = *proof_state_owner;
  proof_state.binding = proof_binding.get();
  proof_state.transport = transport.get();
  proof_state.retirement = &proof_retirement;
  proof_state.dispose_on_status = true;
  auto proof_callbacks = callback_set;
  proof_callbacks.context = &proof_state;
  proof_callbacks.context_owner = proof_state_owner;
  proof_callbacks.on_retired = &OnProofRetired;
  assert(!proof_binding->Register(reinterpret_cast<void*>(0x1), transport, 900,
                                  901, proof_callbacks, 530, 40));
  for (int i = 0; i < 16 && proof_state.retired == 0; ++i)
    (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  assert(proof_state.retired == 1);
  assert(proof_retirement.IsQuiescent());
  proof_binding.reset();

  // Receiver identity is global and independent of the readiness generation;
  // two different receivers may therefore queue on the same transport FD.
  Binding collision_first;
  auto collision_first_state_owner = std::make_shared<CallbackState>();
  auto& collision_first_state = *collision_first_state_owner;
  collision_first_state.binding = &collision_first;
  auto collision_first_callbacks = callback_set;
  collision_first_callbacks.context = &collision_first_state;
  collision_first_callbacks.context_owner = collision_first_state_owner;
  assert(collision_first.Register(reinterpret_cast<void*>(0x1), transport, 302,
                                  -1, collision_first_callbacks, 2, 2));
  Binding collision_second;
  auto collision_second_state_owner = std::make_shared<CallbackState>();
  auto& collision_second_state = *collision_second_state_owner;
  collision_second_state.binding = &collision_second;
  auto collision_second_callbacks = callback_set;
  collision_second_callbacks.context = &collision_second_state;
  collision_second_callbacks.context_owner = collision_second_state_owner;
  assert(collision_second.Prepare(reinterpret_cast<void*>(0x1), transport, 302,
                                  -1, collision_second_callbacks, 8, 8));
  assert(collision_second.Activate());
  assert(!collision_second.Status().local_ready);
  assert(collision_first.Retire());
  for (int i = 0; i < 16 && !collision_second.Status().local_ready; ++i)
    (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  assert(collision_second.Status().local_ready);
  assert(collision_second.Retire());

  // A local-only binding can be closed reentrantly from its first ready edge;
  // the cleanup path must not assume a preprepared remote slot exists.
  Binding local_closed;
  auto local_closed_state_owner = std::make_shared<CallbackState>();
  auto& local_closed_state = *local_closed_state_owner;
  local_closed_state.binding = &local_closed;
  local_closed_state.dispose_on_status = true;
  auto local_closed_callbacks = callback_set;
  local_closed_callbacks.context = &local_closed_state;
  local_closed_callbacks.context_owner = local_closed_state_owner;
  assert(!local_closed.Register(reinterpret_cast<void*>(0x1), transport, 304,
                                -1, local_closed_callbacks, 531, 41));
  assert(local_closed.Status().closed);
  assert(local_closed.Retire());

  // The stale fd token is sticky-terminal and cannot restore local readiness.
  Binding stale;
  auto stale_state_owner = std::make_shared<CallbackState>();
  auto& stale_state = *stale_state_owner;
  stale_state.binding = &stale;
  auto stale_callbacks = callback_set;
  stale_callbacks.context = &stale_state;
  stale_callbacks.context_owner = stale_state_owner;
  assert(stale.Register(reinterpret_cast<void*>(0x1), transport, 20, 21,
                        stale_callbacks, 502, 10));
  assert(Fire(20, 1, 999));
  assert(!stale.Status().local_ready && !stale.Status().closed);
  assert(!stale.RefreshWritable(Slot::kLocalWake, 20, true));
  assert(stale.Retire());

  Binding terminal;
  auto terminal_state_owner = std::make_shared<CallbackState>();
  auto& terminal_state = *terminal_state_owner;
  terminal_state.binding = &terminal;
  terminal_state.terminal_on_event = true;
  auto terminal_callbacks = callback_set;
  terminal_callbacks.context = &terminal_state;
  terminal_callbacks.context_owner = terminal_state_owner;
  assert(terminal.Register(reinterpret_cast<void*>(0x1), transport, 90, 91,
                           terminal_callbacks, 509, 17));
  assert(Fire(90, 1));
  assert(!terminal.Status().local_ready && !terminal.Status().closed);
  assert(terminal.Retire());

  // An event callback exception is reported at the narrow boundary and keeps
  // a healthy registration alive; it is not an endpoint-terminal event.
  Binding event_failed;
  auto event_failed_state_owner = std::make_shared<CallbackState>();
  auto& event_failed_state = *event_failed_state_owner;
  event_failed_state.binding = &event_failed;
  event_failed_state.throw_on_event = true;
  auto event_failed_callbacks = callback_set;
  event_failed_callbacks.context = &event_failed_state;
  event_failed_callbacks.context_owner = event_failed_state_owner;
  event_failed_callbacks.on_failure = &OnFailure;
  assert(event_failed.Register(reinterpret_cast<void*>(0x1), transport, 92,
                               93, event_failed_callbacks, 514, 22));
  assert(Fire(92, 1));
  assert(event_failed_state.failures == 1);
  assert(event_failed.Status().local_ready);
  assert(!event_failed.Status().closed);
  assert(event_failed.Retire());

  // A throwing status observer cannot leave Notify permanently coalesced;
  // the edge remains pending and the explicit failure hook is called.
  Binding status_failed;
  auto status_failed_state_owner = std::make_shared<CallbackState>();
  auto& status_failed_state = *status_failed_state_owner;
  status_failed_state.binding = &status_failed;
  auto status_failed_callbacks = callback_set;
  status_failed_callbacks.on_status = &ThrowingStatus;
  status_failed_callbacks.context = &status_failed_state;
  status_failed_callbacks.context_owner = status_failed_state_owner;
  status_failed_callbacks.on_failure = &OnFailure;
  assert(status_failed.Register(reinterpret_cast<void*>(0x1), transport, 94,
                                95, status_failed_callbacks, 515, 23));
  assert(status_failed_state.failures >= 1);
  assert(status_failed.Status().local_ready);
  assert(status_failed.Retire());

  Binding event_disposed;
  auto event_state_owner = std::make_shared<CallbackState>();
  auto& event_state = *event_state_owner;
  event_state.binding = &event_disposed;
  event_state.dispose_on_event = true;
  auto event_callbacks = callback_set;
  event_callbacks.context = &event_state;
  event_callbacks.context_owner = event_state_owner;
  event_callbacks.on_retired = &OnRetired;
  assert(event_disposed.Register(reinterpret_cast<void*>(0x1), transport, 100,
                                 101, event_callbacks, 510, 18));
  assert(Fire(100, 1));
  assert(event_disposed.Status().closed);
  assert(event_state.retired == 1);
  assert(event_disposed.Retire());

  // Refresh from an admitted event callback is coalesced by the pump while
  // its callback operation is active. It must not turn a transient false into
  // a terminal binding state.
  Binding refresh_reentrant;
  auto refresh_state_owner = std::make_shared<CallbackState>();
  auto& refresh_state = *refresh_state_owner;
  refresh_state.binding = &refresh_reentrant;
  refresh_state.refresh_on_event = true;
  refresh_state.refresh_slot = Slot::kLocalWake;
  refresh_state.refresh_fd = 110;
  auto refresh_callbacks = callback_set;
  refresh_callbacks.context = &refresh_state;
  refresh_callbacks.context_owner = refresh_state_owner;
  assert(refresh_reentrant.Register(reinterpret_cast<void*>(0x1), transport,
                                    110, 111, refresh_callbacks, 511, 19));
  assert(Fire(110, 1));
  assert(!refresh_state.refresh_result);
  assert(refresh_reentrant.Status().local_ready);
  assert(!refresh_reentrant.Status().closed);
  assert(refresh_reentrant.Retire());

  // A real rearm Add failure is terminal, unlike the deferred false returned
  // while the pump owns an in-flight callback operation.
  Binding refresh_failed;
  auto refresh_failed_state_owner = std::make_shared<CallbackState>();
  auto& refresh_failed_state = *refresh_failed_state_owner;
  refresh_failed_state.binding = &refresh_failed;
  auto refresh_failed_callbacks = callback_set;
  refresh_failed_callbacks.context = &refresh_failed_state;
  refresh_failed_callbacks.context_owner = refresh_failed_state_owner;
  assert(refresh_failed.Register(reinterpret_cast<void*>(0x1), transport,
                                 112, 113, refresh_failed_callbacks, 516,
                                 24));
  add_failures = 1;
  assert(!refresh_failed.RefreshWritable(Slot::kLocalWake, 112, true));
  assert(!refresh_failed.Status().local_ready);
  assert(refresh_failed.Status().remote_ready);
  assert(!refresh_failed.Status().closed);
  assert(refresh_failed.Retire());

  // Disposal during each Add closes publication and leaves no ready edge.
  Binding add_disposed;
  auto add_state_owner = std::make_shared<CallbackState>();
  auto& add_state = *add_state_owner;
  add_state.binding = &add_disposed;
  auto add_callbacks = callback_set;
  add_callbacks.context = &add_state;
  add_callbacks.context_owner = add_state_owner;
  binding_during_add = &add_disposed;
  dispose_add_count = 1;
  assert(!add_disposed.Register(reinterpret_cast<void*>(0x1), transport, 30,
                                31, add_callbacks, 503, 11));
  binding_during_add = nullptr;
  assert(add_disposed.Status().closed);
  assert(!add_disposed.Status().local_ready);
  assert(add_disposed.Retire());

  // Reentrant disposal from the status callback is safe and coalesced.
  Binding status_disposed;
  auto status_state_owner = std::make_shared<CallbackState>();
  auto& status_state = *status_state_owner;
  status_state.binding = &status_disposed;
  status_state.dispose_on_status = true;
  auto status_callbacks = callback_set;
  status_callbacks.context = &status_state;
  status_callbacks.context_owner = status_state_owner;
  assert(!status_disposed.Register(reinterpret_cast<void*>(0x1), transport,
                                   40, 41, status_callbacks, 504, 12));
  assert(status_disposed.Status().closed);
  assert(status_disposed.Retire());

  // Failed provider removal retains the closed binding for an exact retry.
  Binding remove_retry;
  auto retry_state_owner = std::make_shared<CallbackState>();
  auto& retry_state = *retry_state_owner;
  retry_state.binding = &remove_retry;
  auto retry_callbacks = callback_set;
  retry_callbacks.context = &retry_state;
  retry_callbacks.context_owner = retry_state_owner;
  retry_callbacks.on_retired = &OnRetired;
  assert(remove_retry.Register(reinterpret_cast<void*>(0x1), transport, 50,
                               51, retry_callbacks, 505, 13));
  remove_failures = 1;
  assert(remove_retry.Retire());
  assert(remove_retry.Status().closed);
  assert(retry_state.retired == 0);
  (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  assert(retry_state.retired == 1);

  // A concurrent/reentrant Retire during each exact provider cleanup handoff
  // cannot steal the live reservation or release the wrong registration.
  Binding remove_reentrant;
  auto remove_reentrant_state_owner = std::make_shared<CallbackState>();
  auto& remove_reentrant_state = *remove_reentrant_state_owner;
  remove_reentrant_state.binding = &remove_reentrant;
  auto remove_reentrant_callbacks = callback_set;
  remove_reentrant_callbacks.context = &remove_reentrant_state;
  remove_reentrant_callbacks.context_owner = remove_reentrant_state_owner;
  assert(remove_reentrant.Register(reinterpret_cast<void*>(0x1), transport,
                                   120, 121, remove_reentrant_callbacks, 512,
                                   20));
  binding_during_remove = &remove_reentrant;
  dispose_remove_count = 1;
  assert(remove_reentrant.Retire());
  binding_during_remove = nullptr;
  assert(remove_reentrant.Status().closed);
  // Reentrant receiver/binding destruction must not destroy the independent
  // completion state before the pump finishes its admitted callback.
  auto destroyed_binding = std::make_unique<Binding>();
  auto retirement = destroyed_binding->RetainRetirement();
  DestroyedBindingState destroyed_state{&destroyed_binding, &retirement};
  assert(destroyed_binding->Register(
      reinterpret_cast<void*>(0x1), transport, 130, 131,
      {.on_event = DestroyBindingOnEvent, .context = &destroyed_state,
       .on_retired = RetiredDestroyedBinding}, 520, 30));
  assert(!retirement.IsQuiescent());
  assert(Fire(130, 1));
  assert(destroyed_binding == nullptr && destroyed_state.retired == 1);
  assert(retirement.IsQuiescent());
  assert(retirement.Retire());
  assert(destroyed_state.retired == 1);

  // Failed exact removal can also be retried after the JNI container is gone.
  auto retry_destroyed = std::make_unique<Binding>();
  auto retry_retirement = retry_destroyed->RetainRetirement();
  DestroyedBindingState retry_destroyed_state{&retry_destroyed, &retry_retirement};
  assert(retry_destroyed->Register(
      reinterpret_cast<void*>(0x1), transport, 132, -1,
      {.context = &retry_destroyed_state,
       .on_retired = RetiredDestroyedBinding}, 521, 31));
  remove_failures = 1;
  retry_destroyed.reset();
  assert(!retry_retirement.IsQuiescent());
  assert(retry_destroyed_state.retired == 0);
  assert(retry_retirement.Retire());
  assert(retry_retirement.IsQuiescent() && retry_destroyed_state.retired == 1);
  // Notification can delete either object whose Retire method is active.
  // RetireControl must pin a value, not reference that object's member storage.
  for (const bool through_handle : {false, true}) {
    auto notifying_binding = std::make_unique<Binding>();
    auto notifying_handle = std::make_unique<Binding::RetirementHandle>(
        notifying_binding->RetainRetirement());
    NotificationDestruction state{&notifying_binding, &notifying_handle};
    assert(notifying_binding->Register(
        reinterpret_cast<void*>(0x1), transport, 134, -1,
        {.context = &state, .on_retired = DestroyRetirementOwners}, 522, 32));
    const bool retired = through_handle ? notifying_handle->Retire()
                                        : notifying_binding->Retire();
    assert(retired && state.calls == 1);
    assert(!notifying_binding && !notifying_handle);
  }

  // A binding may be published while its receiver claim waits behind an
  // admitted retired-output owner. The waiting slot is accepted but not
  // ready; releasing the owner promotes it and the owner task publishes the
  // later readiness edge.
  auto retired_owner = darwin_art::input::ClaimedInputTransportPump::Prepare(
      reinterpret_cast<void*>(0x1), transport, 150, 1,
      darwin_art::input::TransportRegistrationRole::kRetiredOutput, 900, {});
  assert(retired_owner != nullptr && retired_owner->Activate());
  auto deferred_binding = std::make_unique<Binding>();
  auto deferred_state_owner = std::make_shared<CallbackState>();
  auto& deferred_state = *deferred_state_owner;
  deferred_state.binding = deferred_binding.get();
  auto deferred_callbacks = callback_set;
  deferred_callbacks.context = &deferred_state;
  deferred_callbacks.context_owner = deferred_state_owner;
  assert(deferred_binding->Prepare(reinterpret_cast<void*>(0x1), transport,
                                   150, -1, deferred_callbacks, 901, 1));
  assert(deferred_binding->Activate());
  assert(!deferred_binding->Status().local_ready);
  assert(deferred_binding->RefreshWritable(Slot::kLocalWake, 150, true));
  assert(retired_owner->Retire());
  for (int i = 0; i < 8 && !deferred_binding->Status().local_ready; ++i)
    (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  assert(deferred_binding->Status().local_ready);
  const auto promoted_registration = std::find_if(
      registrations.begin(), registrations.end(),
      [](const auto& registration) { return registration.fd == 150; });
  assert(promoted_registration != registrations.end());
  assert((promoted_registration->events & 0x0002) != 0);
  assert(deferred_binding->Retire());
  for (int i = 0; i < 4; ++i)
    (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  deferred_binding.reset();
  retired_owner.reset();

  // Registration/rearm completion must publish quiescence even when no FD
  // callback follows a Retire that reentered provider AddFdOwned.
  for (const bool rearm : {false, true}) {
    darwin_art::input::InputTransportPumpLease pump;
    auto resource = std::make_shared<Transport>(
        darwin_art::input::InputTransportIo{FakeSend, FakeReceive, FakeClose, FakeError}, false);
    struct Completion {
      darwin_art::input::InputTransportPumpLease* pump;
      int calls = 0;
    } completion{&pump};
    darwin_art::input::InputTransportPumpCallbacks callbacks{
      .context = &completion,
      .on_quiescent = [](void* opaque) noexcept {
        auto* state = static_cast<Completion*>(opaque);
        assert(state->pump->IsQuiescent());
        ++state->calls;
        assert(state->pump->Retire());
      }};
    if (rearm) {
      assert(pump.Register(reinterpret_cast<void*>(0x23), resource, 140, 1, callbacks));
      pump_during_add = &pump;
      assert(pump.SetWritableResult(true) ==
             darwin_art::input::InputTransportWritableResult::kTerminal);
    } else {
      pump_during_add = &pump;
      assert(!pump.Register(reinterpret_cast<void*>(0x23), resource, 140, 1, callbacks));
    }
    assert(pump_during_add == nullptr && completion.calls == 1);
    assert(pump.IsQuiescent() && pump.Retire() && completion.calls == 1);
  }
  (void)darwin_art::looper::detail::DispatchReusableTasks(reusable_queue);
  std::puts("receiver-endpoint-binding: PASS slots/status/reentrancy/retry/pump-completion");
  return 0;
}
