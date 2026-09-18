#include "runtime/framework/input/receiver_admission.h"
#include "runtime/framework/input/receiver_registry.h"

#include <atomic>
#include <cassert>
#include <memory>
#include <thread>

using darwin_art::input::ReceiverAdmission;

struct Notification {
  int calls = 0;
  ReceiverAdmission::RetirementHandle retirement;
  std::unique_ptr<ReceiverAdmission>* destroy_gate = nullptr;
};
void Quiescent(void* opaque) noexcept {
  auto* notification = static_cast<Notification*>(opaque);
  assert(notification->retirement.IsQuiescent());
  ++notification->calls;
  if (notification->destroy_gate != nullptr)
    notification->destroy_gate->reset();
}

namespace darwin_art::input {
struct InputReceiver {
  ReceiverAdmission admission;
};
}  // namespace darwin_art::input

int main() {
  // Construction can release without winning cleanup after retirement when
  // another callback is admitted. Token validity must be checked separately.
  auto constructing = std::make_shared<darwin_art::input::InputReceiver>();
  assert(constructing->admission.Admit());
  assert(constructing->admission.Admit());
  const auto token = darwin_art::input::RegisterInputReceiver(constructing);
  const auto retired_lease = darwin_art::input::RetireInputReceiver(token);
  assert(retired_lease == constructing);
  assert(!constructing->admission.Retire());
  assert(!constructing->admission.Release());
  assert(darwin_art::input::AcquireInputReceiver(token) == nullptr);
  assert(constructing->admission.Release());

  // Construction holds admission across registry publication and JNI setup;
  // retirement during that window must wait for construction to release.
  ReceiverAdmission held;
  assert(!held.IsQuiescent());
  assert(held.Admit());
  assert(!held.Retire());
  assert(!held.IsQuiescent());
  assert(!held.Admit());
  assert(held.Release());
  assert(held.IsQuiescent());
  assert(!held.Release());

  ReceiverAdmission concurrent;
  assert(concurrent.Admit());
  std::atomic<bool> retired{false};
  std::atomic<bool> released{false};
  std::thread retire_thread([&] { retired = concurrent.Retire(); });
  std::thread release_thread([&] { released = concurrent.Release(); });
  retire_thread.join();
  release_thread.join();
  assert(!retired.load() || !released.load());
  assert(retired.load() || released.load());
  assert(!concurrent.Admit());
  assert(concurrent.IsQuiescent());

  // The cleanup winner is returned without invoking user code or holding the
  // mutex, so it may safely re-enter the gate before doing external cleanup.
  ReceiverAdmission reentrant;
  assert(reentrant.Admit());
  assert(!reentrant.Retire());
  assert(reentrant.Release());
  assert(!reentrant.Admit());

  ReceiverAdmission retired_before_admit;
  assert(retired_before_admit.Retire());
  assert(!retired_before_admit.Admit());
  assert(!retired_before_admit.Release());

  // The independent handle waits beyond cleanup election until the JNI
  // adapter confirms cleanup, and notification may destroy its former owner.
  auto gate = std::make_unique<ReceiverAdmission>();
  auto retirement = gate->RetainRetirement();
  auto notification = std::make_shared<Notification>();
  notification->retirement = retirement;
  notification->destroy_gate = &gate;
  assert(retirement.SetQuiescenceNotification(Quiescent, notification));
  assert(gate->Admit());
  assert(!gate->Retire());
  assert(!gate->CompleteCleanup());
  assert(!retirement.IsQuiescent() && notification->calls == 0);
  assert(gate->Release());
  assert(gate->IsQuiescent() && !retirement.IsQuiescent());
  assert(gate->CompleteCleanup());
  assert(gate == nullptr && retirement.IsQuiescent());
  assert(notification->calls == 1);

  ReceiverAdmission completed_before_subscribe;
  auto late = completed_before_subscribe.RetainRetirement();
  assert(completed_before_subscribe.Retire());
  assert(completed_before_subscribe.CompleteCleanup());
  auto late_notification = std::make_shared<Notification>();
  late_notification->retirement = late;
  assert(late.SetQuiescenceNotification(Quiescent, late_notification));
  assert(late_notification->calls == 1);
  assert(!completed_before_subscribe.CompleteCleanup());
  assert(!late.SetQuiescenceNotification(Quiescent, late_notification));
  return 0;
}
