#include "runtime/framework/input/channel_endpoint.h"
#include "runtime/framework/input/channel_routing_continuation.h"
#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_routing_domain.h"
#include "runtime/framework/input/input_routing_focus.h"
#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/input_transport_pump.h"
#include "runtime/framework/input/routing_transport_scheduler.h"
#include "compat/looper/android_looper_owner.h"
#include "aim_bionic_socket_broker.h"

#include <android/looper.h>
#include <atomic>
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <map>
#include <mutex>
#include <new>
#include <poll.h>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {
std::atomic<int> g_fail_alloc_after{-1};

bool ShouldFailAllocation() {
  int remaining = g_fail_alloc_after.load(std::memory_order_relaxed);
  while (remaining >= 0) {
    if (remaining == 0) return true;
    if (g_fail_alloc_after.compare_exchange_weak(
            remaining, remaining - 1, std::memory_order_relaxed))
      return false;
  }
  return false;
}
}  // namespace

void* operator new(std::size_t size) {
  if (ShouldFailAllocation()) throw std::bad_alloc();
  if (void* value = std::malloc(size)) return value;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) {
  if (ShouldFailAllocation()) throw std::bad_alloc();
  if (void* value = std::malloc(size)) return value;
  throw std::bad_alloc();
}
void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
  try {
    return ::operator new(size);
  } catch (...) {
    return nullptr;
  }
}
void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
  try {
    return ::operator new[](size);
  } catch (...) {
    return nullptr;
  }
}
void operator delete(void* value) noexcept { std::free(value); }
void operator delete[](void* value) noexcept { std::free(value); }
void operator delete(void* value, std::size_t) noexcept { std::free(value); }
void operator delete[](void* value, std::size_t) noexcept { std::free(value); }

namespace {
using aim::AimInputPacket;
using aim::input::InputRoutingHandle;

intptr_t SendFd(int fd, const void* data, size_t size, int flags) {
  (void)flags;
  return write(fd, data, size);
}
intptr_t RecvFd(int fd, void* data, size_t size, int flags) {
  (void)flags;
  return read(fd, data, size);
}
int CloseFd(int fd) { return close(fd); }
int Error() { return errno == EAGAIN || errno == EWOULDBLOCK ? 11 : errno; }

struct WireFrame {
  uint32_t magic;
  uint32_t version;
  uint32_t kind;
  uint32_t payload_size;
  AimInputPacket payload;
};

void DrainWire(int fd, std::vector<uint8_t>* wire) {
  uint8_t bytes[65536];
  for (;;) {
    const ssize_t count = recv(fd, bytes, sizeof(bytes), MSG_DONTWAIT);
    if (count <= 0) break;
    wire->insert(wire->end(), bytes, bytes + count);
  }
}

struct ProductionContext {
  InputRoutingHandle routing;
  std::shared_ptr<aim::input::InputTransport> transport;
  aim::input::ChannelRoutingContinuation* continuation = nullptr;
  int peer_fd = -1;
  std::atomic<uint64_t> produced{256};
  std::atomic<bool> producer_failed{false};
  std::mutex wire_mutex;
  std::vector<uint8_t> wire;
};

void PumpProgress(void* opaque, aim::input::InputTransportStatus status) {
  auto* context = static_cast<ProductionContext*>(opaque);
  assert(status != aim::input::InputTransportStatus::kTerminal);
  std::lock_guard<std::mutex> lock(context->wire_mutex);
  DrainWire(context->peer_fd, &context->wire);
}

void ProduceRemaining(ProductionContext* context) {
  while (context->produced.load(std::memory_order_acquire) < 300) {
    const uint64_t sequence =
        context->produced.load(std::memory_order_relaxed) + 1;
    AimKeyEventV1 key{};
    key.version = 1;
    key.size = sizeof(key);
    key.sequence = sequence;
    key.action = 0;
    aim::input::InputRoutingAdmission admission;
    if (aim::input::RouteFrameworkKeyPacket(key, &admission) !=
        aim::AimInputEnqueueResult::kQueued) {
      context->producer_failed.store(true, std::memory_order_release);
      return;
    }
    aim::input::InputRoutingInflightLease reservation;
    if (!aim::input::ReserveInputRoutingPacket(
            std::move(admission), false, &reservation)) {
      std::this_thread::yield();
      continue;
    }
    context->produced.store(sequence, std::memory_order_release);
    if (!context->continuation->Request()) {
      context->producer_failed.store(true, std::memory_order_release);
      return;
    }
  }
}

struct FailureContext {
  int progress_calls = 0;
  int failure_calls = 0;
};

aim::input::RoutingTransportDrainResult ThrowProgress(void* opaque) {
  ++static_cast<FailureContext*>(opaque)->progress_calls;
  throw 42;
}

void RecordFailure(void* opaque) {
  ++static_cast<FailureContext*>(opaque)->failure_calls;
}

struct CountContext {
  int calls = 0;
};

aim::input::RoutingTransportDrainResult CountProgress(void* opaque) {
  ++static_cast<CountContext*>(opaque)->calls;
  return {};
}

struct ScheduleRetryContext {
  int progress_calls = 0;
  int failure_calls = 0;
};

aim::input::RoutingTransportDrainResult RetryProgress(void* opaque) {
  ++static_cast<ScheduleRetryContext*>(opaque)->progress_calls;
  return {};
}

void RetryFailure(void* opaque) {
  ++static_cast<ScheduleRetryContext*>(opaque)->failure_calls;
}

std::atomic<bool> g_release_marker{false};
void ReleaseMarker(void* opaque) {
  assert(opaque == &g_release_marker);
  g_release_marker.store(true, std::memory_order_release);
}
void NoopTask(void*) {}
std::map<int, int> wake_writers;
aim::input::ChannelEndpoint* g_refresh_endpoint = nullptr;
aim::input::InputTransportPumpLease* g_refresh_pump = nullptr;

bool RefreshWritableForTest(const InputRoutingHandle&) {
  if (g_refresh_endpoint == nullptr || g_refresh_pump == nullptr) return false;
  return g_refresh_pump->SetWritable(
      g_refresh_endpoint->Transport()->HasPendingTx());
}
}  // namespace

extern "C" int aim_bionic_socket_broker_eventfd(uint32_t, int) {
  int fds[2];
  assert(pipe(fds) == 0);
  wake_writers[fds[0]] = fds[1];
  return fds[0];
}
extern "C" intptr_t aim_bionic_socket_broker_read(int fd, void* p,
                                                           size_t n) {
  return read(fd, p, n);
}
extern "C" intptr_t aim_bionic_socket_broker_write(int fd, const void* p,
                                                            size_t n) {
  return write(wake_writers.at(fd), p, n);
}
extern "C" intptr_t aim_bionic_socket_broker_send(
    int fd, const void* p, size_t n, int flags) {
  (void)flags;
  return write(fd, p, n);
}
extern "C" intptr_t aim_bionic_socket_broker_recv(
    int fd, void* p, size_t n, int flags) {
  (void)flags;
  return read(fd, p, n);
}
extern "C" int aim_bionic_socket_broker_close(int fd) { return close(fd); }
extern "C" int aim_bionic_errno_load() { return errno; }
extern "C" int aim_bionic_socket_broker_poll(
    AimBionicPollFd* fds, size_t count, int timeout) {
  std::vector<pollfd> host;
  host.reserve(count);
  for (size_t i = 0; i < count; ++i)
    host.push_back({fds[i].fd, fds[i].events, 0});
  const int result = poll(host.data(), host.size(), timeout);
  for (size_t i = 0; i < count; ++i) fds[i].revents = host[i].revents;
  return result;
}
extern "C" int aim_bionic_socket_broker_fcntl(int fd, int cmd,
                                                       intptr_t arg) {
  return fcntl(fd, cmd, arg);
}
extern "C" int aim_bionic_socket_broker_socketpair(
    int, int, int, int32_t fds[2]) {
  return socketpair(AF_UNIX, SOCK_STREAM, 0, reinterpret_cast<int*>(fds));
}

namespace aim {
uint64_t AndroidUptimeNanos() { return 0; }
}  // namespace aim

static void DeliverFocusForKeys(
    const aim::input::InputRoutingHandle& state,
    std::uint64_t epoch) {
  const auto recipient =
      aim::input::SnapshotInputRoutingSelection(state).recipient;
  const auto result = aim::input::ApplyInputRoutingFocusControl(
      recipient, aim::input::FocusControl{epoch, true});
  assert(result.Accepted() && result.ShouldNotify());
  assert(aim::input::CommitInputRoutingFocusNotification(result));
  assert(aim::input::CommitInputRoutingFocusReadiness(result, true));
}

int main() {
  alarm(15);
  void* looper = aim::looper::PrepareCurrent();
  assert(looper != nullptr);
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  assert(fcntl(pair[0], F_SETFL, O_NONBLOCK) == 0);
  auto transport = std::make_shared<aim::input::InputTransport>(
      aim::input::InputTransportIo{SendFd, RecvFd, CloseFd, Error}, true);
  aim::input::AdoptRemoteInputTransport(transport.get(), pair[0]);
  auto endpoint = std::make_shared<const aim::input::InputRoutingEndpoint>(
      aim::input::InputRoutingEndpoint{transport, 1});
  const auto routing = aim::input::CreateInputRoutingState();
  assert(aim::input::SetInputRoutingConsumer(routing, 1, endpoint) == 0);
  assert(!aim::input::PublishInputRoutingWmsFrame(
      routing, 0, 0, 100, 100, true));
  assert(aim::input::SetInputRoutingFocus(routing, 1));
  DeliverFocusForKeys(routing, 1);

  aim::input::ChannelEndpoint channel(transport);
  g_refresh_endpoint = &channel;
  aim::input::InputTransportPumpLease pump;
  g_refresh_pump = &pump;
  aim::input::ChannelRoutingContinuation continuation;
  ProductionContext production{routing, transport, &continuation, pair[1],
                               256, false, {}, {}};
  auto channel_handle = std::shared_ptr<aim::input::ChannelEndpoint>(
      &channel, [](auto*) {});
  assert(continuation.Bind(looper, channel_handle, routing,
                           RefreshWritableForTest));
  aim::input::InputTransportPumpCallbacks pump_callbacks{
      .on_progress = PumpProgress, .context = &production};
  assert(pump.Register(looper, transport, pair[0], ALOOPER_EVENT_INPUT,
                       pump_callbacks));

  for (uint64_t sequence = 1; sequence <= 256; ++sequence) {
    AimKeyEventV1 key{};
    key.version = 1;
    key.size = sizeof(key);
    key.sequence = sequence;
    key.action = 0;
    aim::input::InputRoutingAdmission admission;
    assert(aim::input::RouteFrameworkKeyPacket(key, &admission) ==
           aim::AimInputEnqueueResult::kQueued);
    aim::input::InputRoutingInflightLease reservation;
    assert(aim::input::ReserveInputRoutingPacket(
        std::move(admission), false, &reservation));
  }

  std::thread producer(ProduceRemaining, &production);
  assert(continuation.Request());
  assert(continuation.Request());
  while (!production.producer_failed.load(std::memory_order_acquire) &&
         (production.produced.load(std::memory_order_acquire) < 300 ||
          aim::input::InputRoutingHasRunnableAction(routing) ||
          transport->HasPendingTx())) {
    const int poll = ALooper_pollOnce(100, nullptr, nullptr, nullptr);
    assert(poll == ALOOPER_POLL_CALLBACK || poll == ALOOPER_POLL_TIMEOUT);
    std::lock_guard<std::mutex> lock(production.wire_mutex);
    DrainWire(pair[1], &production.wire);
  }
  producer.join();
  assert(!production.producer_failed.load(std::memory_order_acquire));
  for (int i = 0; i < 4 &&
                  (aim::input::InputRoutingHasRunnableAction(routing) ||
                   transport->HasPendingTx());
       ++i) {
    assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
           ALOOPER_POLL_CALLBACK);
    std::lock_guard<std::mutex> lock(production.wire_mutex);
    DrainWire(pair[1], &production.wire);
  }
  {
    std::lock_guard<std::mutex> lock(production.wire_mutex);
    DrainWire(pair[1], &production.wire);
    assert(production.wire.size() >= 300 * sizeof(WireFrame));
    for (uint64_t i = 0; i < 300; ++i) {
      WireFrame frame{};
      std::memcpy(&frame, production.wire.data() + i * sizeof(WireFrame),
                  sizeof(frame));
      assert(frame.magic == 0x44414950 && frame.payload.key.sequence == i + 1);
    }
  }
  continuation.Retire();
  assert(pump.Retire());
  g_refresh_endpoint = nullptr;
  g_refresh_pump = nullptr;

  FailureContext failure;
  auto failing = aim::input::RoutingTransportScheduler::Create(
      looper, routing,
      {.on_progress = ThrowProgress, .on_failure = RecordFailure,
       .context = &failure});
  assert(failing != nullptr && failing->Request());
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(failure.progress_calls == 1 && failure.failure_calls == 1);
  assert(!failing->Request());
  assert(failing->Retire());

  // A provider enqueue allocation is consumed by the provider's release
  // callback. The scheduler reports the failure and remains recoverable after
  // the caller retries once allocation succeeds.
  ScheduleRetryContext schedule_retry_context;
  std::atomic<bool> schedule_done{false};
  std::thread schedule_retry([&] {
    void* retry_looper = aim::looper::PrepareCurrent();
    assert(retry_looper != nullptr);
    auto recovered = aim::input::RoutingTransportScheduler::Create(
        retry_looper, routing,
        {.on_progress = RetryProgress, .on_failure = RetryFailure,
         .context = &schedule_retry_context});
    assert(recovered != nullptr);
    g_fail_alloc_after.store(1, std::memory_order_release);
    assert(!recovered->Request());
    g_fail_alloc_after.store(-1, std::memory_order_release);
    assert(schedule_retry_context.failure_calls == 1);
    assert(recovered->Request());
    assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
           ALOOPER_POLL_CALLBACK);
    assert(schedule_retry_context.progress_calls == 1);
    assert(recovered->Retire());
    schedule_done.store(true, std::memory_order_release);
  });
  schedule_retry.join();
  assert(schedule_done.load(std::memory_order_acquire));

  CountContext count;
  auto suppressed = aim::input::RoutingTransportScheduler::Create(
      looper, routing, {.on_progress = CountProgress, .context = &count});
  assert(suppressed != nullptr && suppressed->Request());
  assert(suppressed->Retire());
  assert(!suppressed->Request());
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(count.calls == 0);

  CountContext concurrent_count;
  auto concurrent = aim::input::RoutingTransportScheduler::Create(
      looper, routing,
      {.on_progress = CountProgress, .context = &concurrent_count});
  assert(concurrent != nullptr);
  std::thread requester([&] {
    for (int i = 0; i < 100; ++i) (void)concurrent->Request();
  });
  std::thread retiree([&] { assert(concurrent->Retire()); });
  requester.join();
  retiree.join();
  assert(!concurrent->Request());
  // No owner callback is allowed after the racing retirement, even if the
  // requester won the provider enqueue race.
  (void)ALooper_pollOnce(100, nullptr, nullptr, nullptr);
  assert(concurrent_count.calls == 0);

  std::atomic<bool> provider_done{false};
  std::thread provider([&] {
    void* provider_looper = aim::looper::PrepareCurrent();
    assert(provider_looper != nullptr);
    g_release_marker.store(false, std::memory_order_release);
    g_fail_alloc_after.store(0, std::memory_order_release);
    assert(aim::looper::ScheduleTimedTaskAt(
               provider_looper, 0, NoopTask, &g_release_marker,
               ReleaseMarker) == 0);
    g_fail_alloc_after.store(-1, std::memory_order_release);
    assert(g_release_marker.load(std::memory_order_acquire));
    provider_done.store(true, std::memory_order_release);
  });
  provider.join();
  assert(provider_done.load(std::memory_order_acquire));
  close(pair[1]);
  return 0;
}
