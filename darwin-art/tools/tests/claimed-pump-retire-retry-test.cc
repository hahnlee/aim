#include "runtime/framework/input/claimed_input_transport_pump.h"
#include "compat/looper/reusable_task.h"

#include <cassert>
#include <cstdio>
#include <memory>

namespace {
void* const owner_looper = reinterpret_cast<void*>(1);
darwin_art::looper::detail::ReusableTaskQueue* queue;
struct Registration {
  int fd = -1;
  darwin_art::looper::FdCallback callback = nullptr;
  void* context = nullptr;
  void* owner = nullptr;
  darwin_art::looper::OwnerRelease release = nullptr;
} registration;
int removal_attempts = 0;
}

// Only the provider is injected. The production claim, pump, retained-control
// registry and reusable-task queue all execute their actual implementations.
namespace darwin_art::looper {
void* Current() { return owner_looper; }
int SignalWake(void* looper) {
  assert(looper == owner_looper);
  return 0;
}
int AddFdOwned(void* looper, int fd, int, int, FdCallback callback,
               void* context, void* owner, OwnerRelease release) {
  assert(looper == owner_looper && registration.fd == -1);
  registration = {fd, callback, context, owner, release};
  return 1;
}
int RemoveFdIfOwned(void* looper, int fd, FdCallback callback, void* context) {
  assert(looper == owner_looper);
  assert(fd == registration.fd && callback == registration.callback &&
         context == registration.context);
  if (++removal_attempts <= 2) return -1;
  const auto removed = registration;
  registration = {};
  removed.release(removed.owner);
  return 1;
}
namespace detail {
ReusableTaskQueue* ReusableTaskQueueForLooper(void* looper) {
  return looper == owner_looper ? queue : nullptr;
}
}
}

extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int*) {
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
  using namespace darwin_art::input;
  queue = darwin_art::looper::detail::CreateReusableTaskQueue();
  assert(queue != nullptr);
  auto transport = std::make_shared<InputTransport>();
  auto provider_owner = std::make_shared<int>(7);
  std::weak_ptr<int> weak_provider_owner = provider_owner;
  auto old = ClaimedInputTransportPump::Prepare(
      owner_looper, transport, 42, 0,
      TransportRegistrationRole::kRetiredOutput, 1, {},
      nullptr, nullptr, provider_owner);
  assert(old != nullptr && old->Activate());
  assert(old->State() == ClaimedPumpState::kActive);
  provider_owner.reset();
  old.reset();
  assert(removal_attempts == 1 && !weak_provider_owner.expired());

  TransportRegistrationAuthority::Claim next;
  assert(transport->RegistrationAuthority().Reserve(
             42, TransportRegistrationRole::kReceiver, 2, owner_looper, &next) ==
         TransportRegistrationResult::kDeferred);
  assert(next.BeginRegistration() == TransportRegistrationResult::kDeferred);
  // First queued retry fails too. Permission must remain unavailable; no
  // external wrapper handle remains to initiate another attempt.
  (void)darwin_art::looper::detail::DispatchReusableTasks(queue);
  assert(removal_attempts == 2);
  assert(next.BeginRegistration() == TransportRegistrationResult::kDeferred);
  assert(!weak_provider_owner.expired());
  (void)darwin_art::looper::detail::DispatchReusableTasks(queue);
  assert(removal_attempts == 3 && registration.fd == -1);
  assert(weak_provider_owner.expired());
  assert(next.BeginRegistration() == TransportRegistrationResult::kAcquired);
  assert(next.ReleaseAfterQuiescence());
  darwin_art::looper::detail::DestroyReusableTaskQueue(queue);
  std::puts("claimed pump retry: dropped handles/two failures/exact claim PASS");
}
