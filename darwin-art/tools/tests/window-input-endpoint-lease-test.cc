#include "runtime/framework/wm/window_input_endpoint_lease.h"

#include <atomic>
#include <cassert>
#include <chrono>
#include <cstdint>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <thread>
#include <utility>

#include <sys/socket.h>
#include <fcntl.h>
#include <unistd.h>

using darwin_art::input::InputChannelResources;
using darwin_art::input::InputTransportStatus;
using darwin_art::framework::wm::WindowInputEndpointLease;
using darwin_art::framework::wm::WindowInputEndpointLeaseAcquireStatus;

namespace {
std::atomic<bool> g_block_send{false};
std::atomic<bool> g_send_entered{false};
std::atomic<bool> g_reentrant_terminate{false};
std::atomic<bool> g_reentrant_called{false};
std::atomic<bool> g_reentrant_result{true};
std::atomic<std::int64_t> g_reentrant_elapsed_us{-1};
WindowInputEndpointLease* g_reentrant_owner = nullptr;
std::atomic<std::uint64_t> g_reentrant_token{0};

void PeerWaitForSend() {
  g_send_entered.store(true, std::memory_order_release);
  while (g_block_send.load(std::memory_order_acquire)) std::this_thread::yield();
}

void AssertQuickTerminateFailure(const std::shared_ptr<WindowInputEndpointLease>& owner,
                                 std::uint64_t token) {
  const auto started = std::chrono::steady_clock::now();
  assert(!owner->TerminateAndQuiesce(token));
  const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
      std::chrono::steady_clock::now() - started);
  assert(elapsed.count() < 100);
}

void RunTerminateAndQuiesceScenario() {
  auto original_endpoint = darwin_art::input::ChannelEndpoint::CreateLocal();
  auto replacement_endpoint = darwin_art::input::ChannelEndpoint::CreateLocal();
  assert(original_endpoint != nullptr && replacement_endpoint != nullptr);
  auto original_resources =
      std::make_shared<InputChannelResources>("terminate-original", original_endpoint);
  auto replacement_resources = std::make_shared<InputChannelResources>(
      "terminate-replacement", replacement_endpoint);
  auto owner = WindowInputEndpointLease::Create();
  assert(owner != nullptr);
  const auto original = owner->Acquire(original_resources);
  const auto replacement = owner->Acquire(replacement_resources);
  assert(original.status == WindowInputEndpointLeaseAcquireStatus::kAcquired);
  assert(replacement.status == WindowInputEndpointLeaseAcquireStatus::kAcquired);
  assert(owner->size() == 2);

  const auto original_transport = original_endpoint->Transport();
  const auto replacement_transport = replacement_endpoint->Transport();
  assert(original_transport != nullptr && replacement_transport != nullptr);
  assert(owner->PublishWindow(original.token, 0, 0, 320, 240, true) ==
         InputTransportStatus::kAccepted);
  const auto original_fence = original_transport->CaptureAcceptedTxFence();

  g_send_entered.store(false, std::memory_order_release);
  g_block_send.store(true, std::memory_order_release);
  InputTransportStatus in_flight_status = InputTransportStatus::kTerminal;
  std::thread in_flight([&] {
    in_flight_status = owner->PublishWindow(original.token, 1, 2, 321, 242, true);
  });
  while (!g_send_entered.load(std::memory_order_acquire)) std::this_thread::yield();

  const std::weak_ptr<InputChannelResources> retained_original = original_resources;
  original_resources.reset();
  assert(!retained_original.expired());
  assert(owner->size() == 2);
  AssertQuickTerminateFailure(owner, original.token);
  assert(original_transport->IsTxTerminal());
  assert(!original_transport->IsRxTerminal());
  assert(owner->size() == 2);  // Terminal TX does not revoke the lease.
  assert(owner->PublishWindow(original.token, 5, 6, 7, 8, true) ==
         InputTransportStatus::kTerminal);
  assert(owner->PublishFocus(original.token, 1, true) == InputTransportStatus::kTerminal);

  g_block_send.store(false, std::memory_order_release);
  in_flight.join();
  assert(in_flight_status == InputTransportStatus::kAccepted);
  assert(!original_transport->IsRxTerminal());
  assert(!retained_original.expired());
  assert(owner->TerminateAndQuiesce(original.token));
  assert(!original_transport->IsRxTerminal());
  const auto fence_status = original_transport->QueryTxFence(original_fence);
  assert(fence_status == darwin_art::input::InputTransportTxFenceStatus::kFlushed);

  // A successor lease has an independent TX lane and remains usable.
  assert(owner->PublishWindow(replacement.token, 10, 20, 30, 40, true) ==
         InputTransportStatus::kAccepted);
  assert(!replacement_transport->IsTxTerminal());
  assert(owner->Release(original.token));
  assert(retained_original.expired());
  assert(!owner->TerminateAndQuiesce(original.token));  // Released token.
  assert(!owner->TerminateAndQuiesce(0));               // Invalid token.
  assert(!owner->TerminateAndQuiesce(0xfeedfaceULL));  // Stale token.
  assert(owner->Close());
  assert(!owner->TerminateAndQuiesce(replacement.token));  // Closed owner.
  g_block_send.store(false, std::memory_order_release);
}

void RunReentrantTerminateScenario() {
  auto endpoint = darwin_art::input::ChannelEndpoint::CreateLocal();
  assert(endpoint != nullptr);
  auto resources = std::make_shared<InputChannelResources>("terminate-reentrant", endpoint);
  auto owner = WindowInputEndpointLease::Create();
  assert(owner != nullptr);
  const auto acquired = owner->Acquire(resources);
  assert(acquired.status == WindowInputEndpointLeaseAcquireStatus::kAcquired);
  const auto transport = endpoint->Transport();
  assert(transport != nullptr);

  g_reentrant_owner = owner.get();
  g_reentrant_token.store(acquired.token, std::memory_order_release);
  g_reentrant_called.store(false, std::memory_order_release);
  g_reentrant_result.store(true, std::memory_order_release);
  g_reentrant_elapsed_us.store(-1, std::memory_order_release);
  g_reentrant_terminate.store(true, std::memory_order_release);
  assert(owner->PublishWindow(acquired.token, 0, 0, 100, 100, true) ==
         InputTransportStatus::kAccepted);
  assert(g_reentrant_called.load(std::memory_order_acquire));
  assert(!g_reentrant_result.load(std::memory_order_acquire));
  assert(g_reentrant_elapsed_us.load(std::memory_order_acquire) < 100000);
  assert(transport->IsTxTerminal());
  assert(!transport->IsRxTerminal());
  assert(owner->TerminateAndQuiesce(acquired.token));  // Retry after send unwound.
  assert(!transport->IsRxTerminal());
  assert(owner->Release(acquired.token));
  g_reentrant_owner = nullptr;
  g_reentrant_token.store(0, std::memory_order_release);
}
}

extern "C" int darwin_art_bionic_socket_broker_socketpair(int domain, int type,
                                                            int protocol, int* fds) {
  assert(domain == 1 && (type & 0xf) == 1 && protocol == 0);
  // The actual endpoint speaks Linux socket flags; this test peer uses host
  // sockets, rather than passing Linux CLOEXEC/NONBLOCK bits to Darwin.
  const int result = ::socketpair(AF_UNIX, SOCK_STREAM, 0, fds);
  if (result == 0) for (int i = 0; i != 2; ++i) {
    assert(::fcntl(fds[i], F_SETFL, O_NONBLOCK) == 0);
    assert(::fcntl(fds[i], F_SETFD, FD_CLOEXEC) == 0);
  }
  return result;
}
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int fd, const void* data,
                                                           size_t size, int) {
  if (g_block_send.load(std::memory_order_acquire)) PeerWaitForSend();
  if (g_reentrant_terminate.exchange(false, std::memory_order_acq_rel)) {
    assert(g_reentrant_owner != nullptr);
    const auto started = std::chrono::steady_clock::now();
    const bool result = g_reentrant_owner->TerminateAndQuiesce(
        g_reentrant_token.load(std::memory_order_acquire));
    const auto elapsed = std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::steady_clock::now() - started);
    g_reentrant_result.store(result, std::memory_order_release);
    g_reentrant_elapsed_us.store(elapsed.count(), std::memory_order_release);
    g_reentrant_called.store(true, std::memory_order_release);
  }
  return static_cast<intptr_t>(::send(fd, data, size, MSG_NOSIGNAL));
}
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int fd, void* data,
                                                           size_t size, int) {
  return static_cast<intptr_t>(::recv(fd, data, size, 0));
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) { return ::close(fd); }
extern "C" int darwin_art_bionic_errno_load() { return errno == EAGAIN ? 11 : errno; }

// This contract does not bind a Looper or schedule continuation work. Fail
// loudly if that scope changes; these are not success-returning providers.
namespace darwin_art::looper {
int AddFdOwned(void*, int, int, int, FdCallback, void*, void*, OwnerRelease) {
  std::abort();
}
int RemoveFdIfOwned(void*, int, FdCallback, void*) { std::abort(); }
int ScheduleTimedTaskAt(void*, int64_t, TimedTaskCallback, void*, TimedTaskRelease) {
  std::abort();
}
}

int main() {
  alarm(10);
  RunTerminateAndQuiesceScenario();
  RunReentrantTerminateScenario();

  auto endpoint = darwin_art::input::ChannelEndpoint::CreateLocal();
  assert(endpoint != nullptr);
  auto resources = std::make_shared<InputChannelResources>("lease-test", endpoint);
  auto owner = WindowInputEndpointLease::Create();
  assert(owner != nullptr);
  assert(owner->Acquire(nullptr).status ==
         WindowInputEndpointLeaseAcquireStatus::kInvalidArgument);
  auto invalid = std::make_shared<InputChannelResources>("invalid", nullptr);
  assert(owner->Acquire(invalid).status ==
         WindowInputEndpointLeaseAcquireStatus::kInvalidArgument);

  const auto acquired = owner->Acquire(resources);
  assert(acquired.status == WindowInputEndpointLeaseAcquireStatus::kAcquired);
  assert(acquired.token != 0 && owner->size() == 1);
  assert(owner->PublishWindow(acquired.token, 0, 0, 320, 240, true) ==
         InputTransportStatus::kAccepted);
  assert(owner->PublishFocus(acquired.token, 0, true) == InputTransportStatus::kTerminal);
  assert(owner->PublishFocus(acquired.token, 1, true) == InputTransportStatus::kAccepted);

  // Publish pins the original shared resource before releasing the owner lock.
  // Release and Close therefore cannot destroy the endpoint while this call is
  // blocked in the test peer's real socket send.
  g_send_entered.store(false, std::memory_order_release);
  g_block_send.store(true, std::memory_order_release);
  InputTransportStatus in_flight_status = InputTransportStatus::kTerminal;
  std::thread in_flight([&] {
    in_flight_status = owner->PublishWindow(acquired.token, 1, 2, 321, 242, true);
  });
  while (!g_send_entered.load(std::memory_order_acquire)) std::this_thread::yield();
  const std::weak_ptr<InputChannelResources> original = resources;
  resources.reset();
  assert(owner->Release(acquired.token));
  assert(!original.expired());
  assert(owner->size() == 0);
  assert(owner->PublishWindow(acquired.token, 0, 0, 1, 1, true) ==
         InputTransportStatus::kTerminal);
  g_block_send.store(false, std::memory_order_release);
  in_flight.join();
  assert(in_flight_status == InputTransportStatus::kAccepted);
  assert(original.expired());

  auto replacement = std::make_shared<InputChannelResources>(
      "replacement", darwin_art::input::ChannelEndpoint::CreateLocal());
  const auto next = owner->Acquire(replacement);
  assert(next.status == WindowInputEndpointLeaseAcquireStatus::kAcquired);
  assert(next.token != acquired.token);
  g_send_entered.store(false, std::memory_order_release);
  g_block_send.store(true, std::memory_order_release);
  std::thread closing_call([&] {
    in_flight_status = owner->PublishFocus(next.token, 2, false);
  });
  while (!g_send_entered.load(std::memory_order_acquire)) std::this_thread::yield();
  const std::weak_ptr<InputChannelResources> closing = replacement;
  replacement.reset();
  owner->Close();
  assert(!closing.expired());
  g_block_send.store(false, std::memory_order_release);
  closing_call.join();
  assert(in_flight_status == InputTransportStatus::kAccepted && closing.expired());
  assert(owner->closed() && owner->size() == 0);
  assert(owner->Release(next.token) == false);
  assert(owner->PublishFocus(next.token, 2, false) == InputTransportStatus::kTerminal);
  auto after_close = std::make_shared<InputChannelResources>("after-close", endpoint);
  assert(owner->Acquire(after_close).status ==
         WindowInputEndpointLeaseAcquireStatus::kClosed);
  std::puts("WMS original endpoint lease: retention, stale tokens and in-flight retirement PASS");
  alarm(0);
  return 0;
}
