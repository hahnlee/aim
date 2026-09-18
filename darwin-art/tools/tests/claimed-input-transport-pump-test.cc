#include "runtime/framework/input/claimed_input_transport_pump.h"
#include "compat/looper/android_looper_owner.h"
#include "darwin_art_bionic_socket_broker.h"

#include <android/looper.h>

#include <cassert>
#include <atomic>
#include <cerrno>
#include <cstdlib>
#include <ctime>
#include <fcntl.h>
#include <poll.h>
#include <unistd.h>

#include <map>
#include <memory>
#include <new>
#include <thread>
#include <vector>

namespace {
std::atomic<int> fail_allocation_after{-1};
void* Allocate(std::size_t size) {
  if (fail_allocation_after.load(std::memory_order_relaxed) >= 0 &&
      fail_allocation_after.fetch_sub(1, std::memory_order_relaxed) == 0)
    throw std::bad_alloc();
  void* value = std::malloc(size == 0 ? 1 : size);
  if (value == nullptr) throw std::bad_alloc();
  return value;
}
std::map<int, int> wake_writers;

int FdCallback(int, int, void* opaque) {
  ++*static_cast<int*>(opaque);
  return 1;
}

struct DroppingCallbackState {
  darwin_art::input::ClaimedInputTransportPumpHandle* handle;
  std::weak_ptr<int> owner;
  int calls = 0;
};

int DroppingCallback(int, int, void* context) {
  auto& state = *static_cast<DroppingCallbackState*>(context);
  ++state.calls;
  assert(*state.handle != nullptr);
  assert((*state.handle)->Retire());
  // Retirement acceptance is not proof of callback-tail quiescence.
  assert(!(*state.handle)->IsQuiescent());
  state.handle->reset();
  assert(!state.owner.expired());
  return 0;
}
}  // namespace

void* operator new(std::size_t size) { return Allocate(size); }
void* operator new[](std::size_t size) { return Allocate(size); }
void operator delete(void* value) noexcept { std::free(value); }
void operator delete[](void* value) noexcept { std::free(value); }
void operator delete(void* value, std::size_t) noexcept { std::free(value); }
void operator delete[](void* value, std::size_t) noexcept { std::free(value); }

extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t, int) {
  int fds[2];
  assert(pipe(fds) == 0);
  wake_writers[fds[0]] = fds[1];
  return fds[0];
}
extern "C" intptr_t darwin_art_bionic_socket_broker_read(int fd, void* p,
                                                           size_t n) {
  return read(fd, p, n);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_write(int fd,
                                                            const void* p,
                                                            size_t n) {
  return write(wake_writers.at(fd), p, n);
}
extern "C" int darwin_art_bionic_socket_broker_poll(DarwinArtBionicPollFd* fds,
                                                      size_t count, int timeout) {
  std::vector<pollfd> host;
  host.reserve(count);
  for (size_t i = 0; i < count; ++i)
    host.push_back({fds[i].fd, fds[i].events, 0});
  const int result = poll(host.data(), host.size(), timeout);
  for (size_t i = 0; i < count; ++i) fds[i].revents = host[i].revents;
  return result;
}
extern "C" int darwin_art_bionic_socket_broker_fcntl(int fd, int cmd,
                                                       intptr_t arg) {
  return fcntl(fd, cmd, arg);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int fd, const void* p,
                                                           size_t n, int) {
  return write(fd, p, n);
}
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int fd, void* p,
                                                           size_t n, int) {
  return read(fd, p, n);
}
extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int,
                                                             int32_t fds[2]) {
  return pipe(fds);
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) { return close(fd); }
extern "C" int darwin_art_bionic_errno_load() {
  // The broker contract exposes Bionic errno, not Darwin's EAGAIN=35.
  if (errno == EAGAIN) return 11;
  if (errno == EINTR) return 4;
  return errno;
}

namespace darwin_art {
uint64_t AndroidUptimeNanos() {
  timespec value{};
  assert(clock_gettime(CLOCK_MONOTONIC, &value) == 0);
  return static_cast<uint64_t>(value.tv_sec) * 1000000000ULL + value.tv_nsec;
}
}  // namespace darwin_art

int main() {
  alarm(10);
  void* looper = darwin_art::looper::PrepareCurrent();
  assert(looper != nullptr);
  auto transport = std::make_shared<darwin_art::input::InputTransport>();

  // Failed preparation must not retain provider callback owners, even when
  // task preparation catches the allocation failure and returns a null task.
  // Start before first registry initialization to exercise that allocation too.
  int failed_preparations = 0;
  for (int allocation = 0; allocation < 14; ++allocation) {
    auto owner = std::make_shared<int>(0);
    std::weak_ptr<int> weak_owner = owner;
    darwin_art::input::ClaimedPumpObserver observer{
        [](void* context, darwin_art::input::ClaimedPumpState) noexcept {
          ++*static_cast<int*>(context);
        }, owner.get(), owner};
    fail_allocation_after.store(allocation);
    auto candidate = darwin_art::input::ClaimedInputTransportPump::Prepare(
        looper, transport, 123, ALOOPER_EVENT_INPUT,
        darwin_art::input::TransportRegistrationRole::kReceiver,
        1000 + allocation, {}, nullptr, nullptr, owner, observer);
    fail_allocation_after.store(-1);
    if (candidate == nullptr) {
      ++failed_preparations;
      assert(*owner == 0);
    } else {
      assert(candidate->Retire());
      assert(candidate->IsQuiescent());
    }
    candidate.reset();
    observer.context_owner.reset();
    owner.reset();
    assert(weak_owner.expired());
  }
  assert(failed_preparations >= 7);

  // Allocation can fail after BeginRegistration has admitted the claim, in
  // the raw lease or provider registration. Preparation tests cannot prove
  // this path releases authority, so fail those allocations separately.
  int failed_activations = 0;
  for (int allocation = 0; allocation < 7; ++allocation) {
    int activation_pair[2];
    assert(pipe(activation_pair) == 0);
    auto owner = std::make_shared<int>(0);
    std::weak_ptr<int> weak_owner = owner;
    auto candidate = darwin_art::input::ClaimedInputTransportPump::Prepare(
        looper, transport, activation_pair[0], ALOOPER_EVENT_INPUT,
        darwin_art::input::TransportRegistrationRole::kReceiver,
        2000 + allocation, {}, nullptr, nullptr, owner);
    assert(candidate != nullptr);
    owner.reset();
    fail_allocation_after.store(allocation);
    assert(candidate->Activate());  // Acceptance alone does not mean ready.
    fail_allocation_after.store(-1);
    if (candidate->Failed()) ++failed_activations;
    assert(candidate->Retire());
    for (int frame = 0; frame < 8 && !candidate->IsQuiescent(); ++frame)
      (void)ALooper_pollOnce(0, nullptr, nullptr, nullptr);
    assert(candidate->IsQuiescent());
    assert(!transport->RegistrationAuthority().HasAdmittedRegistration(
        activation_pair[0]));
    candidate.reset();
    assert(weak_owner.expired());
    close(activation_pair[0]);
    close(activation_pair[1]);
  }
  assert(failed_activations >= 3);

  int pre_pair[2];
  assert(pipe(pre_pair) == 0);
  auto preactivation =
      darwin_art::input::ClaimedInputTransportPump::Prepare(
          looper, transport, pre_pair[0], ALOOPER_EVENT_INPUT,
          darwin_art::input::TransportRegistrationRole::kReceiver, 99, {});
  assert(preactivation != nullptr);
  assert(preactivation->Retire());
  assert(preactivation->IsQuiescent());
  assert(preactivation->State() ==
         darwin_art::input::ClaimedPumpState::kSettled);
  close(pre_pair[0]);
  close(pre_pair[1]);

  std::shared_ptr<darwin_art::input::ClaimedInputTransportPump> foreign;
  std::thread wrong_owner([&] {
    foreign = darwin_art::input::ClaimedInputTransportPump::Prepare(
        looper, transport, 123, ALOOPER_EVENT_INPUT,
        darwin_art::input::TransportRegistrationRole::kReceiver, 100, {});
  });
  wrong_owner.join();
  assert(foreign == nullptr);

  int pair[2];
  assert(pipe(pair) == 0);
  int callbacks = 0;
  auto active = darwin_art::input::ClaimedInputTransportPump::Prepare(
      looper, transport, pair[0], ALOOPER_EVENT_INPUT,
      darwin_art::input::TransportRegistrationRole::kReceiver, 1, {},
      FdCallback, &callbacks);
  assert(active != nullptr);
  assert(active->State() == darwin_art::input::ClaimedPumpState::kPrepared);
  assert(active->Activate());
  assert(active->State() == darwin_art::input::ClaimedPumpState::kActive);
  auto conflict = darwin_art::input::ClaimedInputTransportPump::Prepare(
      looper, transport, pair[0], ALOOPER_EVENT_INPUT,
      darwin_art::input::TransportRegistrationRole::kReceiver, 1, {});
  assert(conflict == nullptr);
  assert(write(pair[1], "x", 1) == 1);
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(callbacks == 1);
  assert(active->Retire());
  assert(active->IsQuiescent());
  assert(active->State() == darwin_art::input::ClaimedPumpState::kSettled);
  close(pair[0]);
  close(pair[1]);

  int foreign_pair[2];
  assert(pipe(foreign_pair) == 0);
  auto foreign_activate =
      darwin_art::input::ClaimedInputTransportPump::Prepare(
          looper, transport, foreign_pair[0], ALOOPER_EVENT_INPUT,
          darwin_art::input::TransportRegistrationRole::kReceiver, 4, {});
  assert(foreign_activate != nullptr);
  bool activated_from_foreign = false;
  std::thread activate_thread([&] {
    activated_from_foreign = foreign_activate->Activate();
  });
  activate_thread.join();
  assert(activated_from_foreign);
  assert(foreign_activate->State() ==
         darwin_art::input::ClaimedPumpState::kPrepared);
  (void)ALooper_pollOnce(100, nullptr, nullptr, nullptr);
  assert(foreign_activate->State() ==
         darwin_art::input::ClaimedPumpState::kActive);
  assert(foreign_activate->Retire());
  assert(foreign_activate->IsQuiescent());
  close(foreign_pair[0]);
  close(foreign_pair[1]);

  // Dispose/destruction from inside the actual provider callback must leave
  // its context alive until the raw callback and owner task have both ended.
  int drop_pair[2];
  assert(pipe(drop_pair) == 0);
  auto callback_owner = std::make_shared<int>(7);
  std::weak_ptr<int> weak_callback_owner = callback_owner;
  darwin_art::input::ClaimedInputTransportPumpHandle dropped;
  DroppingCallbackState dropping{&dropped, weak_callback_owner};
  dropped = darwin_art::input::ClaimedInputTransportPump::Prepare(
      looper, transport, drop_pair[0], ALOOPER_EVENT_INPUT,
      darwin_art::input::TransportRegistrationRole::kReceiver, 5, {},
      DroppingCallback, &dropping, callback_owner);
  assert(dropped != nullptr && dropped->Activate());
  callback_owner.reset();
  assert(write(drop_pair[1], "x", 1) == 1);
  (void)ALooper_pollOnce(100, nullptr, nullptr, nullptr);
  assert(dropping.calls == 1 && dropped == nullptr);
  for (int frame = 0; frame < 8 && !weak_callback_owner.expired(); ++frame)
    (void)ALooper_pollOnce(0, nullptr, nullptr, nullptr);
  assert(weak_callback_owner.expired());
  auto successor = darwin_art::input::ClaimedInputTransportPump::Prepare(
      looper, transport, drop_pair[0], ALOOPER_EVENT_INPUT,
      darwin_art::input::TransportRegistrationRole::kReceiver, 6, {});
  assert(successor != nullptr && successor->Activate());
  assert(successor->State() == darwin_art::input::ClaimedPumpState::kActive);
  assert(successor->Retire() && successor->IsQuiescent());
  close(drop_pair[0]);
  close(drop_pair[1]);

  // The second owner is genuinely deferred by an admitted retired-output
  // claim, then promoted only after exact raw-pump quiescence and release.
  int shared_pair[2];
  assert(pipe(shared_pair) == 0);
  auto retired = darwin_art::input::ClaimedInputTransportPump::Prepare(
      looper, transport, shared_pair[0], ALOOPER_EVENT_INPUT,
      darwin_art::input::TransportRegistrationRole::kRetiredOutput, 2, {},
      FdCallback, &callbacks);
  assert(retired != nullptr && retired->Activate());
  assert(retired->State() == darwin_art::input::ClaimedPumpState::kActive);
  auto waiting = darwin_art::input::ClaimedInputTransportPump::Prepare(
      looper, transport, shared_pair[0], ALOOPER_EVENT_INPUT,
      darwin_art::input::TransportRegistrationRole::kReceiver, 3, {},
      FdCallback, &callbacks);
  assert(waiting != nullptr);
  assert(waiting->State() == darwin_art::input::ClaimedPumpState::kWaiting);
  assert(waiting->Activate());
  assert(retired->Retire());
  for (int i = 0; i < 4 && waiting->State() !=
                              darwin_art::input::ClaimedPumpState::kActive;
       ++i)
    (void)ALooper_pollOnce(100, nullptr, nullptr, nullptr);
  assert(waiting->State() == darwin_art::input::ClaimedPumpState::kActive);
  assert(waiting->Retire());
  assert(waiting->IsQuiescent());
  close(shared_pair[0]);
  close(shared_pair[1]);
  transport.reset();
  {
    using namespace darwin_art::input;
    int framed_pair[2];
    assert(pipe(framed_pair) == 0);
    assert(fcntl(framed_pair[0], F_SETFL, O_NONBLOCK) == 0);
    auto reader = std::make_shared<InputTransport>();
    InputTransport writer;
    assert(AdoptRemoteInputTransport(reader.get(), framed_pair[0]));
    assert(AdoptRemoteInputTransport(&writer, framed_pair[1]));
    struct Observation { int packet = 0, window = 0, ack = 0; } observation;
    InputTransportPumpCallbacks ordered{
      .context = &observation,
      .on_packet_consumption = [](void* context, const darwin_art::DarwinArtInputPacket&) {
        ++static_cast<Observation*>(context)->packet;
        return InputTransportConsumptionResult::kConsumed;
      },
      .on_window_consumption = [](void* context, int32_t, int32_t, int32_t, int32_t, bool) {
        auto& state = *static_cast<Observation*>(context);
        assert(state.packet == 1);
        ++state.window;
        return InputTransportConsumptionResult::kConsumedStop;
      },
      .on_ack64 = [](void* context, uint64_t sequence, bool handled) {
        assert(sequence == (1ULL << 48) + 700 && handled);
        ++static_cast<Observation*>(context)->ack;
      },
    };
    auto pump = ClaimedInputTransportPump::Prepare(
        looper, reader, framed_pair[0], ALOOPER_EVENT_INPUT,
        TransportRegistrationRole::kReceiver, 9901, ordered);
    assert(pump != nullptr && pump->Activate());
    assert(pump->State() == ClaimedPumpState::kActive);
    darwin_art::DarwinArtInputPacket packet{};
    packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
    packet.key.version = 1;
    packet.key.size = sizeof(packet.key);
    packet.key.sequence = 71;
    assert(SendInputTransportAck64(&writer, (1ULL << 48) + 700, true) == InputTransportStatus::kAccepted);
    assert(SendInputTransportPacket(&writer, packet) == InputTransportStatus::kAccepted);
    assert(SendInputTransportWindow(&writer, 0, 0, 360, 640, true) == InputTransportStatus::kAccepted);
    for (int attempt = 0; attempt < 8 && observation.window == 0; ++attempt)
      (void)ALooper_pollOnce(100, nullptr, nullptr, nullptr);
    assert(observation.packet == 1 && observation.window == 1 && observation.ack == 1);
    assert(pump->Retire() && pump->IsQuiescent());
    close(framed_pair[0]);
    close(framed_pair[1]);
  }
  return 0;
}
