#include "darwin_art_bionic_socket_broker.h"
#include "window/surface_scanout_owner.h"

#include <notify.h>

#include <array>
#include <atomic>
#include <cassert>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <map>
#include <mutex>
#include <poll.h>
#include <signal.h>
#include <string>
#include <thread>
#include <unistd.h>

namespace {

struct CallbackState final {
  std::mutex mutex;
  std::condition_variable condition;
  darwin_art::window::SurfaceScanoutOwner* owner = nullptr;
  uint64_t calls = 0;
  uint64_t generation = 0;
  bool entered = false;
  bool delay = false;
  bool release = false;
  bool reenter_owner = false;
  bool reentered = false;
  bool reentry_closed = false;
};

std::mutex g_close_mutex;
std::map<int, unsigned> g_close_counts;
std::atomic<darwin_art::window::SurfaceScanoutOwner*> g_reentry_owner{
    nullptr};
std::atomic<int> g_reentry_fd{-1};
std::atomic<bool> g_reentry_observed{false};

void FenceReady(void* context, uint64_t generation) noexcept {
  auto* state = static_cast<CallbackState*>(context);
  std::unique_lock<std::mutex> lock(state->mutex);
  ++state->calls;
  state->generation = generation;
  state->entered = true;
  state->condition.notify_all();
  const bool reenter_owner = state->reenter_owner;
  auto* owner = state->owner;
  if (state->delay) {
    state->condition.wait(lock, [state] { return state->release; });
  }
  lock.unlock();
  if (reenter_owner && owner != nullptr) {
    const auto result = owner->Request(0, true);
    lock.lock();
    state->reentered = true;
    state->reentry_closed =
        result == darwin_art::window::SurfaceScanoutRequestResult::kClosed;
    lock.unlock();
  }
}

unsigned ClosedCount(int fd) {
  std::lock_guard<std::mutex> lock(g_close_mutex);
  const auto found = g_close_counts.find(fd);
  return found == g_close_counts.end() ? 0u : found->second;
}

void ResetCloseCounts() {
  std::lock_guard<std::mutex> lock(g_close_mutex);
  g_close_counts.clear();
}

constexpr size_t kMaximumPollDescriptors = 32;

uint32_t NotifySurfaceId(uint32_t salt) {
  return 0x6d000000u ^ (static_cast<uint32_t>(getpid()) << 8) ^ salt;
}

std::string NotifyName(uint32_t surface_id) {
  return "dev.darwinart.surface." + std::to_string(surface_id) +
         ".composition";
}

bool WaitForCallback(CallbackState* state) {
  std::unique_lock<std::mutex> lock(state->mutex);
  return state->condition.wait_for(lock, std::chrono::seconds(2),
                                   [state] { return state->entered; });
}

void SignalPipe(int fd) {
  const uint8_t marker = 1;
  assert(::write(fd, &marker, sizeof(marker)) == sizeof(marker));
}

void ClearNotificationBaseline(
    darwin_art::window::SurfaceScanoutOwner* owner) {
  using Request = darwin_art::window::SurfaceScanoutRequestResult;
  const Request request = owner->Request(0, false);
  (void)owner->TakeReimportRequest();
  if (request == Request::kWakeMain) {
    uint64_t generation = 0;
    assert(owner->BeginDrain(&generation));
    assert(!owner->CompleteDrain(generation, 0));
  }
}

}  // namespace

extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd* descriptors, size_t count, int timeout_ms) {
  if (descriptors == nullptr || count > kMaximumPollDescriptors) {
    errno = EINVAL;
    return -1;
  }
  std::array<struct pollfd, kMaximumPollDescriptors> native{};
  for (size_t i = 0; i < count; ++i) {
    native[i].fd = descriptors[i].fd;
    native[i].events = descriptors[i].events;
  }
  const int result = ::poll(native.data(), count, timeout_ms);
  for (size_t i = 0; i < count; ++i) descriptors[i].revents = native[i].revents;
  return result;
}

extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  auto* owner = g_reentry_owner.load(std::memory_order_acquire);
  if (owner != nullptr && fd == g_reentry_fd.load(std::memory_order_acquire)) {
    const auto result = owner->Request(0, true);
    g_reentry_observed.store(
        result == darwin_art::window::SurfaceScanoutRequestResult::kClosed,
        std::memory_order_release);
  }
  {
    std::lock_guard<std::mutex> lock(g_close_mutex);
    ++g_close_counts[fd];
  }
  return ::close(fd);
}

namespace {

void TestNotifyPrecedesUnreadyFence() {
  ResetCloseCounts();
  CallbackState callback;
  auto owner = darwin_art::window::SurfaceScanoutOwner::Create(&FenceReady,
                                                                &callback);
  assert(owner != nullptr);
  int pipe_fds[2] = {-1, -1};
  assert(::pipe(pipe_fds) == 0);
  const uint32_t surface_id = NotifySurfaceId(1);
  assert(owner->RebindBacking(surface_id));
  ClearNotificationBaseline(owner.get());
  assert(owner->TrackCompositionFence(pipe_fds[0]));
  // An embedded frame cannot bypass a newer pending composition fence.
  using Request = darwin_art::window::SurfaceScanoutRequestResult;
  assert(owner->Request(1, false) == Request::kGated);
  assert(!owner->TakeReimportRequest());
  assert(notify_post(NotifyName(surface_id).c_str()) == NOTIFY_STATUS_OK);

  Request request = Request::kGated;
  bool reimport = false;
  for (int attempt = 0; attempt != 200; ++attempt) {
    request = owner->Request(0, false);
    reimport = owner->TakeReimportRequest();
    if (reimport) break;
    usleep(1000);
  }
  assert(reimport);
  assert(request == Request::kWakeMain);
  uint64_t generation = 0;
  assert(owner->BeginDrain(&generation));
  assert(!owner->CompleteDrain(generation, 0));
  SignalPipe(pipe_fds[1]);
  assert(WaitForCallback(&callback));
  assert(callback.generation == 1);
  ::close(pipe_fds[1]);
  assert(ClosedCount(pipe_fds[0]) == 1);
}

void TestEmbeddedDirtyAndRebindNotification() {
  ResetCloseCounts();
  CallbackState callback;
  auto owner = darwin_art::window::SurfaceScanoutOwner::Create(&FenceReady,
                                                                &callback);
  assert(owner != nullptr);
  using Request = darwin_art::window::SurfaceScanoutRequestResult;
  assert(owner->Request(7, false) == Request::kWakeMain);
  uint64_t generation = 0;
  assert(owner->BeginDrain(&generation));
  assert(!owner->CompleteDrain(generation, 0));
  assert(owner->Request(7, false) == Request::kNoWork);

  const uint32_t first = NotifySurfaceId(2);
  const uint32_t second = NotifySurfaceId(3);
  assert(owner->RebindBacking(first));
  ClearNotificationBaseline(owner.get());
  assert(!owner->TakeReimportRequest());
  assert(notify_post(NotifyName(first).c_str()) == NOTIFY_STATUS_OK);
  Request request = Request::kNoWork;
  bool reimport = false;
  for (int attempt = 0; attempt != 200; ++attempt) {
    request = owner->Request(0, false);
    reimport = owner->TakeReimportRequest();
    if (reimport) break;
    usleep(1000);
  }
  assert(reimport && request == Request::kWakeMain);
  assert(owner->BeginDrain(&generation));
  assert(!owner->CompleteDrain(generation, 0));

  assert(owner->RebindBacking(second));
  ClearNotificationBaseline(owner.get());
  assert(!owner->TakeReimportRequest());
  assert(owner->Request(7, false) == Request::kWakeMain);
  assert(owner->BeginDrain(&generation));
  assert(!owner->CompleteDrain(generation, 0));
  // The retired backing's notify token is cancelled; posting its old name
  // cannot make the current backing dirty.
  assert(notify_post(NotifyName(first).c_str()) == NOTIFY_STATUS_OK);
  for (int attempt = 0; attempt != 50; ++attempt) {
    assert(owner->Request(0, false) == Request::kNoWork);
    assert(!owner->TakeReimportRequest());
    usleep(1000);
  }
  assert(owner->RebindBacking(0));
  assert(!owner->TakeReimportRequest());
}

void TestFenceClosureAndDelayedStop() {
  ResetCloseCounts();
  CallbackState callback;
  callback.delay = true;
  callback.reenter_owner = true;
  auto owner = darwin_art::window::SurfaceScanoutOwner::Create(&FenceReady,
                                                                &callback);
  assert(owner != nullptr);
  callback.owner = owner.get();
  int pipe_fds[2] = {-1, -1};
  int pending_fds[2] = {-1, -1};
  assert(::pipe(pipe_fds) == 0);
  assert(::pipe(pending_fds) == 0);
  assert(owner->TrackCompositionFence(pipe_fds[0]));
  assert(owner->TrackCompositionFence(pending_fds[0]));
  SignalPipe(pipe_fds[1]);
  assert(WaitForCallback(&callback));
  // Establish the close cutoff while the callback is deliberately parked;
  // StopAndJoin must wait for its reentrant tail before returning.
  assert(owner->BeginClose());

  std::atomic<bool> stopped{false};
  std::thread stopper([&] {
    owner->StopAndJoin();
    stopped.store(true, std::memory_order_release);
  });
  usleep(20000);
  assert(!stopped.load(std::memory_order_acquire));
  {
    std::lock_guard<std::mutex> lock(callback.mutex);
    callback.release = true;
  }
  callback.condition.notify_all();
  stopper.join();
  assert(stopped.load(std::memory_order_acquire));
  assert(callback.calls == 1);
  {
    std::lock_guard<std::mutex> lock(callback.mutex);
    assert(callback.reentered && callback.reentry_closed);
  }
  assert(ClosedCount(pipe_fds[0]) == 1);
  assert(ClosedCount(pending_fds[0]) == 1);
  ::close(pipe_fds[1]);
  // The second fence was pending at the close cutoff. Its producer write is
  // rejected after StopAndJoin and cannot trigger a late callback.
  errno = 0;
  assert(::write(pending_fds[1], "x", 1) == -1 && errno == EPIPE);
  ::close(pending_fds[1]);
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  assert(callback.calls == 1);
  owner->StopAndJoin();
  assert(ClosedCount(pipe_fds[0]) == 1);
}

void TestRejectedFenceClosesOutsideOwnerLock() {
  ResetCloseCounts();
  CallbackState callback;
  auto owner = darwin_art::window::SurfaceScanoutOwner::Create(&FenceReady,
                                                                &callback);
  assert(owner != nullptr);
  assert(owner->BeginClose());
  int pipe_fds[2] = {-1, -1};
  assert(::pipe(pipe_fds) == 0);
  g_reentry_fd.store(pipe_fds[0], std::memory_order_release);
  g_reentry_owner.store(owner.get(), std::memory_order_release);
  assert(!owner->TrackCompositionFence(pipe_fds[0]));
  g_reentry_owner.store(nullptr, std::memory_order_release);
  assert(g_reentry_observed.load(std::memory_order_acquire));
  assert(ClosedCount(pipe_fds[0]) == 1);
  ::close(pipe_fds[1]);
}

}  // namespace

int main() {
  ::signal(SIGPIPE, SIG_IGN);
  TestNotifyPrecedesUnreadyFence();
  TestEmbeddedDirtyAndRebindNotification();
  TestFenceClosureAndDelayedStop();
  TestRejectedFenceClosesOutsideOwnerLock();
  std::puts("surface scanout owner: notify/fence/embedded/rebind/close PASS");
  return 0;
}
