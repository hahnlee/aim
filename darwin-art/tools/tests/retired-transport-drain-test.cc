#include "runtime/framework/input/retired_transport_drain.h"
#include "compat/looper/android_looper_owner.h"

#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstring>
#include <memory>
#include <new>
#include <stdexcept>
#include <vector>

using Drain = darwin_art::input::RetiredTransportDrain;
using Result = darwin_art::input::RetiredTransportDrainResult;
using Status = darwin_art::input::InputTransportStatus;
using Transport = darwin_art::input::InputTransport;

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

namespace {

struct Registration {
  int fd = -1;
  int events = 0;
  int (*callback)(int, int, void*) = nullptr;
  void* context = nullptr;
  void (*release)(void*) = nullptr;
};

Registration* registration = nullptr;
int add_failures = 0;
bool add_oom = false;
struct TimedTask {
  darwin_art::looper::TimedTaskCallback callback = nullptr;
  void* context = nullptr;
  darwin_art::looper::TimedTaskRelease release = nullptr;
};
std::vector<TimedTask> timed_tasks;
int schedule_failures = 0;
bool schedule_oom = false;
bool schedule_throw = false;
Drain* retire_during_add = nullptr;
bool terminal_during_add = false;
int remove_failures = 0;
bool blocked = false;
bool terminal_send = false;
bool terminal_event_during_send = false;
std::vector<std::uint8_t> wire;
Drain* request_during_release = nullptr;
std::shared_ptr<Transport> release_transport;

void RunTimedTasks() {
  while (!timed_tasks.empty()) {
    TimedTask task = timed_tasks.front();
    timed_tasks.erase(timed_tasks.begin());
    if (task.callback != nullptr) task.callback(task.context);
    if (task.release != nullptr) task.release(task.context);
  }
}

intptr_t Send(int, const void* bytes, size_t size, int) {
  if (blocked || terminal_send) return -1;
  if (terminal_event_during_send && registration != nullptr) {
    terminal_event_during_send = false;
    (void)registration->callback(registration->fd, 0x0004,
                                 registration->context);
  }
  const auto* begin = static_cast<const std::uint8_t*>(bytes);
  wire.insert(wire.end(), begin, begin + size);
  return static_cast<intptr_t>(size);
}

intptr_t Receive(int, void*, size_t, int) { return -1; }
int Close(int) { return 0; }
int Error() { return terminal_send ? 32 : 11; }

bool RemoveExact(void*, int fd, darwin_art::looper::FdCallback callback,
                void* context) {
  if (remove_failures > 0) {
    --remove_failures;
    return false;
  }
  if (registration == nullptr || registration->fd != fd ||
      registration->callback != callback || registration->context != context)
    return false;
  auto* old = registration;
  registration = nullptr;
  if (request_during_release != nullptr) {
    auto* drain = request_during_release;
    request_during_release = nullptr;
    blocked = true;
    assert(SendInputTransportAck(release_transport.get(), 90, false) ==
           Status::kAccepted);
    blocked = false;
    assert(drain->Request() == Result::kDeferred);
  }
  old->release(old->context);
  return true;
}

bool Fire(int events) {
  if (registration == nullptr) return false;
  auto* current = registration;
  const int result = current->callback(current->fd, events, current->context);
  if (result == 0 && registration == current)
    assert(RemoveExact(nullptr, current->fd, current->callback,
                       current->context));
  return true;
}

struct Progress {
  Drain* drain = nullptr;
  std::shared_ptr<Transport> transport;
  int calls = 0;
  Status last = Status::kAccepted;
  bool request_reentrant = false;
  bool enqueue_reentrant = false;
  bool throw_callback = false;
  std::unique_ptr<Drain>* destroy_owner = nullptr;
  Result reentrant_result = Result::kApplied;
  int failure_calls = 0;
  Result last_failure = Result::kApplied;
};

void ProgressCallback(void* opaque, Status status) {
  auto* progress = static_cast<Progress*>(opaque);
  ++progress->calls;
  progress->last = status;
  if (progress->throw_callback) throw std::bad_alloc();
  if (progress->destroy_owner != nullptr) {
    auto* owner = progress->destroy_owner;
    progress->destroy_owner = nullptr;
    owner->reset();
  }
  if (progress->request_reentrant) {
    progress->request_reentrant = false;
    if (progress->enqueue_reentrant) {
      progress->enqueue_reentrant = false;
      blocked = true;
      assert(SendInputTransportAck(progress->transport.get(), 91, false) ==
             Status::kAccepted);
      blocked = false;
    }
    progress->reentrant_result = progress->drain->Request();
  }
}

void FailureCallback(void* opaque, Result result) {
  auto* progress = static_cast<Progress*>(opaque);
  ++progress->failure_calls;
  progress->last_failure = result;
}

}  // namespace

namespace darwin_art::looper {
int AddFdOwned(void*, int fd, int, int events, FdCallback callback,
               void* context, void*, OwnerRelease release) {
  if (add_oom) {
    add_oom = false;
    release(context);
    throw std::bad_alloc();
  }
  if (add_failures > 0) {
    --add_failures;
    release(context);
    return -1;
  }
  assert(registration == nullptr);
  registration = new Registration{fd, events, callback, context, release};
  if (terminal_during_add) {
    terminal_during_add = false;
    // Publication still owns the operation: latch, do not remove or notify
    // before the activation owner exits the provider boundary.
    assert(callback(fd, 0x0008, context) == 1);
  }
  if (retire_during_add != nullptr) {
    auto* drain = retire_during_add;
    retire_during_add = nullptr;
    assert(drain->Retire() == Result::kDeferred);
  }
  return 1;
}

int RemoveFdIfOwned(void* looper, int fd, FdCallback callback, void* context) {
  return RemoveExact(looper, fd, callback, context) ? 1 : -1;
}

int ScheduleTimedTaskAt(void*, int64_t, TimedTaskCallback callback,
                        void* context, TimedTaskRelease release) {
  if (schedule_oom) {
    schedule_oom = false;
    release(context);
    throw std::bad_alloc();
  }
  if (schedule_throw) {
    schedule_throw = false;
    release(context);
    throw std::runtime_error("timed task provider rejected");
  }
  if (schedule_failures > 0) {
    --schedule_failures;
    release(context);
    return 0;
  }
  timed_tasks.push_back(TimedTask{callback, context, release});
  return 1;
}
}  // namespace darwin_art::looper

int main() {
  using namespace darwin_art::input;
  auto transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(transport.get(), 77);
  Drain drain;
  Progress progress;
  progress.drain = &drain;
  RetiredTransportDrainCallbacks callbacks{ProgressCallback, &progress, {}};
  assert(drain.Prepare(reinterpret_cast<void*>(1), transport, 77, callbacks) ==
         Result::kApplied);
  assert(drain.Status().prepared && !drain.Status().active);
  assert(drain.Activate() == Result::kApplied);
  assert(registration == nullptr);  // No idle OUTPUT registration.

  blocked = true;
  assert(SendInputTransportAck(transport.get(), 1, true) ==
         Status::kAccepted);
  assert(drain.Activate() == Result::kApplied);
  assert(registration != nullptr && registration->events == 0x0002);
  assert(drain.Request() == Result::kDeferred);
  assert(progress.last == Status::kBackpressured);
  blocked = false;
  assert(drain.Request() == Result::kApplied);
  assert(transport->HasPendingTx() == false && registration == nullptr);

  // A callback-side Request is deferred while the drain operation is owned;
  // it must not keep an idle OUTPUT registration alive after the flush.
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 2, false) ==
         Status::kAccepted);
  blocked = false;
  progress.transport = transport;
  progress.request_reentrant = true;
  progress.enqueue_reentrant = true;
  assert(drain.Activate() == Result::kApplied);
  assert(Fire(0x0002));
  assert(progress.reentrant_result == Result::kDeferred);
  assert(registration != nullptr && transport->HasPendingTx());
  assert(Fire(0x0002));
  assert(registration == nullptr && !transport->HasPendingTx());

  // Add failure is retryable and does not claim a successful drain.
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 3, false) ==
         Status::kAccepted);
  add_failures = 1;
  assert(drain.Activate() == Result::kProviderFailure);
  assert(registration == nullptr);
  blocked = false;
  assert(drain.Activate() == Result::kApplied);
  assert(drain.Request() == Result::kApplied);

  // Provider OOM is distinct from an ordinary Add failure and remains
  // retryable after the provider consumes the callback owner.
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 4, false) ==
         Status::kAccepted);
  add_oom = true;
  assert(drain.Activate() == Result::kOutOfMemory);
  assert(registration == nullptr);
  blocked = false;
  assert(drain.Activate() == Result::kApplied);
  assert(drain.Request() == Result::kApplied);

  // Exact removal failure leaves the old registration retained for Retire's
  // explicit retry; no unrelated registration can be removed.
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 5, false) ==
         Status::kAccepted);
  blocked = false;
  assert(drain.Activate() == Result::kApplied);
  remove_failures = 1;
  assert(drain.Request() == Result::kProviderFailure);
  assert(drain.Status().closed && registration != nullptr);
  assert(drain.Retire() == Result::kApplied);
  assert(registration == nullptr);

  // A producer and Request racing with provider OwnerRelease retain the new
  // request generation. The old remove completion must not clear a successor
  // operation or lose the newly queued bytes.
  Drain coalesced;
  callbacks.on_failure = FailureCallback;
  assert(coalesced.Prepare(reinterpret_cast<void*>(5), transport, 77,
                           callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 9, false) ==
         Status::kAccepted);
  blocked = false;
  assert(coalesced.Activate() == Result::kApplied);
  release_transport = transport;
  request_during_release = &coalesced;
  assert(coalesced.Request() == Result::kDeferred);
  request_during_release = nullptr;
  release_transport.reset();
  assert(registration == nullptr && transport->HasPendingTx());
  RunTimedTasks();
  assert(registration == nullptr && !transport->HasPendingTx());
  assert(timed_tasks.empty());

  // A queued continuation belongs to the old control epoch. Retiring before
  // its looper turn suppresses the task without invoking progress/failure or
  // reopening an OUTPUT registration.
  Drain closed_epoch;
  assert(closed_epoch.Prepare(reinterpret_cast<void*>(9), transport, 77,
                              callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 15, false) ==
         Status::kAccepted);
  blocked = false;
  assert(closed_epoch.Activate() == Result::kApplied);
  release_transport = transport;
  request_during_release = &closed_epoch;
  assert(closed_epoch.Request() == Result::kDeferred);
  request_during_release = nullptr;
  release_transport.reset();
  const int progress_before_closed_epoch = progress.calls;
  assert(!timed_tasks.empty() && registration == nullptr);
  assert(closed_epoch.Retire() == Result::kApplied);
  RunTimedTasks();
  assert(timed_tasks.empty() && registration == nullptr);
  assert(closed_epoch.Status().closed && !closed_epoch.Status().pending);
  assert(progress.calls == progress_before_closed_epoch);

  // The same retained request is also driven when the looper removes an
  // OUTPUT callback after its callback-side flush. No second Request call is
  // made by the producer or by this test.
  Drain callback_removed;
  assert(callback_removed.Prepare(reinterpret_cast<void*>(6), transport, 77,
                                  callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 10, false) ==
         Status::kAccepted);
  blocked = false;
  assert(callback_removed.Activate() == Result::kApplied);
  release_transport = transport;
  request_during_release = &callback_removed;
  assert(Fire(0x0002));
  request_during_release = nullptr;
  release_transport.reset();
  assert(registration == nullptr && transport->HasPendingTx());
  RunTimedTasks();
  assert(registration == nullptr && !transport->HasPendingTx());

  // Rejected and throwing timed-task providers consume the continuation
  // owner exactly once. The pending request remains observable and can be
  // retried; each failure has its distinct typed result.
  Drain continuation_failure;
  Progress continuation_progress;
  continuation_progress.drain = &continuation_failure;
  RetiredTransportDrainCallbacks continuation_callbacks{
      ProgressCallback, &continuation_progress, {}};
  continuation_callbacks.on_failure = FailureCallback;
  assert(continuation_failure.Prepare(reinterpret_cast<void*>(7), transport, 77,
                                      continuation_callbacks) ==
         Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 11, false) ==
         Status::kAccepted);
  blocked = false;
  schedule_failures = 1;
  release_transport = transport;
  request_during_release = &continuation_failure;
  assert(continuation_failure.Request() == Result::kDeferred);
  request_during_release = nullptr;
  release_transport.reset();
  assert(continuation_progress.failure_calls == 1);
  assert(continuation_progress.last_failure == Result::kProviderFailure);
  assert(continuation_failure.Status().pending && registration == nullptr);
  assert(continuation_failure.Request() == Result::kApplied);
  assert(!transport->HasPendingTx());

  blocked = true;
  assert(SendInputTransportAck(transport.get(), 12, false) ==
         Status::kAccepted);
  blocked = false;
  schedule_oom = true;
  release_transport = transport;
  request_during_release = &continuation_failure;
  assert(continuation_failure.Request() == Result::kDeferred);
  request_during_release = nullptr;
  release_transport.reset();
  assert(continuation_progress.failure_calls == 2);
  assert(continuation_progress.last_failure == Result::kOutOfMemory);
  assert(continuation_failure.Status().pending && registration == nullptr);
  assert(continuation_failure.Request() == Result::kApplied);

  blocked = true;
  assert(SendInputTransportAck(transport.get(), 13, false) ==
         Status::kAccepted);
  blocked = false;
  schedule_throw = true;
  release_transport = transport;
  request_during_release = &continuation_failure;
  assert(continuation_failure.Request() == Result::kDeferred);
  request_during_release = nullptr;
  release_transport.reset();
  assert(continuation_progress.failure_calls == 3);
  assert(continuation_progress.last_failure == Result::kProviderFailure);
  assert(continuation_failure.Status().pending && registration == nullptr);
  assert(continuation_failure.Request() == Result::kApplied);

  // A continuation's progress exception closes its epoch and reports one
  // failure; the RunContinuation fallback must not duplicate that report.
  Drain continuation_progress_failed;
  Progress deferred_failure_progress;
  deferred_failure_progress.drain = &continuation_progress_failed;
  RetiredTransportDrainCallbacks deferred_failure_callbacks{
      ProgressCallback, &deferred_failure_progress, {}};
  deferred_failure_callbacks.on_failure = FailureCallback;
  auto deferred_failure_transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(deferred_failure_transport.get(), 83);
  assert(continuation_progress_failed.Prepare(
             reinterpret_cast<void*>(12), deferred_failure_transport, 83,
             deferred_failure_callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(deferred_failure_transport.get(), 18, false) ==
         Status::kAccepted);
  blocked = false;
  assert(continuation_progress_failed.Activate() == Result::kApplied);
  release_transport = deferred_failure_transport;
  request_during_release = &continuation_progress_failed;
  assert(continuation_progress_failed.Request() == Result::kDeferred);
  request_during_release = nullptr;
  release_transport.reset();
  deferred_failure_progress.throw_callback = true;
  RunTimedTasks();
  assert(deferred_failure_progress.failure_calls == 1);
  assert(deferred_failure_progress.last_failure == Result::kProviderFailure);
  assert(continuation_progress_failed.Status().closed && registration == nullptr);

  // Retirement during provider publication closes the prepared epoch before
  // it can become active; activation removes exactly that registration.
  Drain publication_retired;
  assert(publication_retired.Prepare(reinterpret_cast<void*>(3), transport, 77,
                                     callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(transport.get(), 7, false) ==
         Status::kAccepted);
  blocked = false;
  retire_during_add = &publication_retired;
  assert(publication_retired.Activate() == Result::kTerminal);
  assert(registration == nullptr);
  assert(publication_retired.Retire() == Result::kApplied);

  // HUP observed during Add must reach the policy after the activation
  // reservation ends, including a throwing terminal-progress observer.
  for (int throwing = 0; throwing != 2; ++throwing) {
    Drain publication_terminal;
    Progress publication_progress;
    publication_progress.throw_callback = throwing != 0;
    RetiredTransportDrainCallbacks publication_callbacks{
        ProgressCallback, &publication_progress, {}};
    publication_callbacks.on_failure = FailureCallback;
    auto publication_transport = std::make_shared<Transport>(
        InputTransportIo{Send, Receive, Close, Error}, false);
    AdoptRemoteInputTransport(publication_transport.get(), 91);
    blocked = true;
    assert(SendInputTransportAck(publication_transport.get(), 8, false) ==
           Status::kAccepted);
    assert(publication_terminal.Prepare(reinterpret_cast<void*>(3),
        publication_transport, 91, publication_callbacks) == Result::kApplied);
    terminal_during_add = true;
    assert(publication_terminal.Activate() ==
           (throwing ? Result::kProviderFailure : Result::kTerminal));
    assert(registration == nullptr);
    assert(publication_progress.calls == 1);
    assert(publication_progress.last == Status::kTerminal);
    assert(publication_progress.failure_calls == throwing);
    assert(publication_terminal.Retire() == Result::kApplied);
  }
  blocked = false;

  // Policy progress exceptions are contained at this owner boundary and do
  // not escape through an OUTPUT looper callback as a false drain success.
  Drain progress_failed;
  Progress failed_progress;
  failed_progress.drain = &progress_failed;
  failed_progress.throw_callback = true;
  RetiredTransportDrainCallbacks failed_callbacks{ProgressCallback,
                                                  &failed_progress, {}};
  failed_callbacks.on_failure = FailureCallback;
  auto failed_transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(failed_transport.get(), 79);
  assert(progress_failed.Prepare(reinterpret_cast<void*>(4), failed_transport,
                                 79, failed_callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(failed_transport.get(), 8, false) ==
         Status::kAccepted);
  blocked = false;
  assert(progress_failed.Activate() == Result::kApplied);
  assert(progress_failed.Request() == Result::kProviderFailure);
  assert(progress_failed.Status().closed && registration == nullptr);
  assert(failed_progress.failure_calls == 1);
  assert(failed_progress.last_failure == Result::kProviderFailure);
  assert(progress_failed.Retire() == Result::kApplied);

  // Even when exact cleanup itself fails, the asynchronous progress failure
  // is reported before returning; Retire can then retry that retained owner.
  Drain cleanup_failed;
  Progress cleanup_progress;
  cleanup_progress.drain = &cleanup_failed;
  cleanup_progress.throw_callback = true;
  RetiredTransportDrainCallbacks cleanup_callbacks{ProgressCallback,
                                                   &cleanup_progress, {}};
  cleanup_callbacks.on_failure = FailureCallback;
  auto cleanup_transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(cleanup_transport.get(), 82);
  assert(cleanup_failed.Prepare(reinterpret_cast<void*>(11), cleanup_transport,
                                82, cleanup_callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(cleanup_transport.get(), 17, false) ==
         Status::kAccepted);
  blocked = false;
  assert(cleanup_failed.Activate() == Result::kApplied);
  remove_failures = 1;
  assert(cleanup_failed.Request() == Result::kProviderFailure);
  assert(cleanup_progress.failure_calls == 1);
  assert(cleanup_progress.last_failure == Result::kProviderFailure);
  assert(cleanup_failed.Status().closed && registration != nullptr);
  assert(cleanup_failed.Retire() == Result::kApplied);
  assert(registration == nullptr);

  // An ERROR arriving while Request owns the operation is deferred to that
  // operation; its exact cleanup and terminal notification remain intact.
  Drain concurrent_terminal;
  Progress concurrent_terminal_progress;
  concurrent_terminal_progress.drain = &concurrent_terminal;
  RetiredTransportDrainCallbacks concurrent_terminal_callbacks{
      ProgressCallback, &concurrent_terminal_progress, {}};
  auto concurrent_terminal_transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(concurrent_terminal_transport.get(), 81);
  assert(concurrent_terminal.Prepare(
             reinterpret_cast<void*>(10), concurrent_terminal_transport, 81,
             concurrent_terminal_callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(concurrent_terminal_transport.get(), 16, false) ==
         Status::kAccepted);
  blocked = false;
  assert(concurrent_terminal.Activate() == Result::kApplied);
  terminal_event_during_send = true;
  assert(concurrent_terminal.Request() == Result::kTerminal);
  assert(concurrent_terminal.Status().terminal && registration == nullptr);
  assert(concurrent_terminal_progress.last == Status::kTerminal);

  // Request pins the control before progress re-enters and destroys the
  // public drain object. The in-flight operation still removes its exact
  // registration and returns a typed failure instead of using freed state.
  auto destroyed = std::make_unique<Drain>();
  Progress destruction_progress;
  destruction_progress.destroy_owner = &destroyed;
  RetiredTransportDrainCallbacks destruction_callbacks{
      ProgressCallback, &destruction_progress, {}};
  auto destruction_transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(destruction_transport.get(), 80);
  assert(destroyed->Prepare(reinterpret_cast<void*>(8), destruction_transport,
                            80, destruction_callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(destruction_transport.get(), 14, false) ==
         Status::kAccepted);
  blocked = false;
  assert(destroyed->Activate() == Result::kApplied);
  assert(destroyed->Request() == Result::kProviderFailure);
  assert(destroyed == nullptr && registration == nullptr);

  // A terminal transport result is not reported as an accepted drain.
  Drain terminal;
  Progress terminal_progress;
  terminal_progress.drain = &terminal;
  RetiredTransportDrainCallbacks terminal_callbacks{ProgressCallback,
                                                    &terminal_progress, {}};
  terminal_callbacks.on_failure = FailureCallback;
  auto terminal_transport = std::make_shared<Transport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  AdoptRemoteInputTransport(terminal_transport.get(), 78);
  assert(terminal.Prepare(reinterpret_cast<void*>(2), terminal_transport, 78,
                          terminal_callbacks) == Result::kApplied);
  blocked = true;
  assert(SendInputTransportAck(terminal_transport.get(), 6, false) ==
         Status::kAccepted);
  assert(terminal.Activate() == Result::kApplied);
  blocked = false;
  // Error/HUP/INVALID events terminate the exact owner instead of returning
  // zero and silently dropping the only driver for retained bytes.
  terminal_progress.throw_callback = true;
  assert(Fire(0x0004 | 0x0008 | 0x0010));
  assert(terminal.Status().terminal && registration == nullptr);
  assert(terminal_progress.failure_calls == 1);
  assert(terminal_progress.last_failure == Result::kProviderFailure);
  terminal_send = true;
  assert(terminal.Request() == Result::kTerminal);
  terminal_send = false;
  assert(terminal.Status().terminal && registration == nullptr);
  assert(terminal.Retire() == Result::kApplied);

  std::puts("retired transport drain: PASS prepare/activate/retry/terminal");
  return 0;
}
