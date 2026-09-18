#include "darwin_art_bionic_socket_broker.h"
#include "darwin_android_platform.h"
#include "looper/android_looper_owner.h"

#include <android/choreographer.h>
#include <android/looper.h>

#include <cassert>
#include <cerrno>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <map>
#include <mutex>
#include <poll.h>
#include <stdexcept>
#include <thread>
#include <time.h>
#include <unistd.h>
#include <vector>

namespace {

std::mutex g_event_mutex;
std::map<int, int> g_event_writers;

struct CallbackState {
  std::thread::id owner_thread;
  int frame64 = 0;
  int vsync = 0;
  int refresh = 0;
  bool wrong_thread = false;
  int64_t frame_time = 0;
  int64_t expected_time = 0;
  int64_t deadline = 0;
  AVsyncId vsync_id = 0;
  AChoreographer* choreographer = nullptr;
};

struct OrderedTask {
  std::vector<int>* order;
  int value;
};

void OrderedTaskCallback(void* opaque) {
  auto* task = static_cast<OrderedTask*>(opaque);
  task->order->push_back(task->value);
}

void OrderedTaskRelease(void* opaque) { delete static_cast<OrderedTask*>(opaque); }

void IncrementTask(void* opaque) { ++*static_cast<int*>(opaque); }
void NoopRelease(void*) {}
void ThrowingTask(void*) { throw std::runtime_error("test callback failure"); }

int64_t MonotonicNanos() {
  timespec value{};
  assert(clock_gettime(CLOCK_MONOTONIC, &value) == 0);
  return static_cast<int64_t>(value.tv_sec) * 1'000'000'000 + value.tv_nsec;
}

void CheckOwnerThread(CallbackState* state) {
  std::lock_guard<std::mutex> lock(g_event_mutex);
  if (std::this_thread::get_id() != state->owner_thread)
    state->wrong_thread = true;
}

void Frame64(int64_t frame_time, void* opaque) {
  auto* state = static_cast<CallbackState*>(opaque);
  CheckOwnerThread(state);
  bool repost = false;
  {
    std::lock_guard<std::mutex> lock(g_event_mutex);
    ++state->frame64;
    state->frame_time = frame_time;
    repost = state->frame64 == 1;
  }
  if (repost) {
    AChoreographer_postFrameCallbackDelayed64(state->choreographer, Frame64,
                                              state, 1);
  }
}

void Vsync(const AChoreographerFrameCallbackData* data, void* opaque) {
  auto* state = static_cast<CallbackState*>(opaque);
  CheckOwnerThread(state);
  std::lock_guard<std::mutex> lock(g_event_mutex);
  ++state->vsync;
  state->vsync_id =
      AChoreographerFrameCallbackData_getFrameTimelineVsyncId(data, 0);
  state->expected_time =
      AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos(
          data, 0);
  state->deadline =
      AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos(data, 0);
}

void Refresh(int64_t, void* opaque) {
  auto* state = static_cast<CallbackState*>(opaque);
  CheckOwnerThread(state);
  std::lock_guard<std::mutex> lock(g_event_mutex);
  ++state->refresh;
}

int FdCallback(int, int, void* opaque) {
  auto* count = static_cast<int*>(opaque);
  ++*count;
  return 0;
}

int PersistentFdCallback(int, int, void* opaque) {
  auto* count = static_cast<int*>(opaque);
  ++*count;
  return 1;
}

void OwnerRelease(void* opaque) { ++*static_cast<int*>(opaque); }

struct ReentrantReleaseState {
  void* looper = nullptr;
  int other_fd = -1;
  int* other_data = nullptr;
  int release_count = 0;
  int nested_remove_result = -1;
};

void ReentrantOwnerRelease(void* opaque) {
  auto* state = static_cast<ReentrantReleaseState*>(opaque);
  ++state->release_count;
  state->nested_remove_result = darwin_art::looper::RemoveFdIfOwned(
      state->looper, state->other_fd, PersistentFdCallback,
      state->other_data);
}

}  // namespace

// This scoped broker mock uses host pipes, while the production result seam
// reads Android TLS. Translate the errors exercised by this mock explicitly.
extern "C" int darwin_art_bionic_errno_load() {
  if (errno == EAGAIN || errno == EWOULDBLOCK) return 11;
  if (errno == EINTR) return 4;
  return errno;
}

extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t initial_value,
                                                          int) {
  int descriptors[2] = {-1, -1};
  if (pipe(descriptors) != 0) return -1;
  std::lock_guard<std::mutex> lock(g_event_mutex);
  g_event_writers[descriptors[0]] = descriptors[1];
  if (initial_value != 0) {
    const uint64_t value = initial_value;
    if (write(descriptors[1], &value, sizeof(value)) != sizeof(value)) return -1;
  }
  return descriptors[0];
}

extern "C" intptr_t darwin_art_bionic_socket_broker_read(int fd, void* bytes,
                                                            size_t count) {
  return read(fd, bytes, count);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_write(int fd,
                                                             const void* bytes,
                                                             size_t count) {
  std::lock_guard<std::mutex> lock(g_event_mutex);
  const auto found = g_event_writers.find(fd);
  return found == g_event_writers.end() ? -1 : write(found->second, bytes, count);
}

extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd* descriptors, size_t count, int timeout_ms) {
  std::vector<struct pollfd> host_descriptors;
  host_descriptors.reserve(count);
  for (size_t index = 0; index < count; ++index) {
    descriptors[index].revents = 0;
    host_descriptors.push_back({descriptors[index].fd, descriptors[index].events, 0});
  }
  int result;
  do {
    result = ::poll(host_descriptors.data(), host_descriptors.size(), timeout_ms);
  } while (result < 0 && errno == EINTR);
  if (result <= 0) return result;
  int ready = 0;
  for (size_t index = 0; index < count; ++index) {
    descriptors[index].revents = host_descriptors[index].revents;
    if (descriptors[index].revents != 0) ++ready;
  }
  return ready;
}

extern "C" int darwin_art_bionic_socket_broker_fcntl(int fd, int command,
                                                       intptr_t argument) {
  return ::fcntl(fd, command, argument);
}

int main() {
  void* looper = darwin_art::looper::PrepareCurrent();
  assert(looper != nullptr);
  assert(darwin_art::looper::Current() == looper);
  AChoreographer* choreographer = AChoreographer_getInstance();
  assert(choreographer != nullptr);

  int fd_pair[2] = {-1, -1};
  assert(pipe(fd_pair) == 0);
  int old_callback_count = 0;
  int new_callback_count = 0;
  assert(ALooper_addFd(static_cast<ALooper*>(looper), fd_pair[0], 7,
                       ALOOPER_EVENT_INPUT, FdCallback, &old_callback_count) ==
         1);
  const char old_byte = 'a';
  assert(write(fd_pair[1], &old_byte, sizeof(old_byte)) == sizeof(old_byte));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(old_callback_count == 1);
  assert(ALooper_addFd(static_cast<ALooper*>(looper), fd_pair[0], 8,
                       ALOOPER_EVENT_INPUT, FdCallback, &new_callback_count) ==
         1);
  const char new_byte = 'b';
  assert(write(fd_pair[1], &new_byte, sizeof(new_byte)) == sizeof(new_byte));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(new_callback_count == 1);
  close(fd_pair[0]);
  close(fd_pair[1]);

  int owned_fd_pair[2] = {-1, -1};
  assert(pipe(owned_fd_pair) == 0);
  int owned_callback_count = 0;
  int owner_release_count = 0;
  assert(darwin_art_android_platform_add_fd_owned(
             looper, owned_fd_pair[0], 9, ALOOPER_EVENT_INPUT, FdCallback,
             &owned_callback_count, &owner_release_count, OwnerRelease) == 1);
  const char owned_byte = 'c';
  assert(write(owned_fd_pair[1], &owned_byte, sizeof(owned_byte)) ==
         sizeof(owned_byte));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(owned_callback_count == 1 && owner_release_count == 1);
  close(owned_fd_pair[0]);
  close(owned_fd_pair[1]);

  int owned_replace_fd_pair[2] = {-1, -1};
  assert(pipe(owned_replace_fd_pair) == 0);
  int stale_callback_count = 0;
  int current_callback_count = 0;
  int stale_release_count = 0;
  int current_release_count = 0;
  assert(darwin_art_android_platform_add_fd_owned(
             looper, owned_replace_fd_pair[0], 10, ALOOPER_EVENT_INPUT,
             PersistentFdCallback, &stale_callback_count, &stale_release_count,
             OwnerRelease) == 1);
  assert(darwin_art_android_platform_add_fd_owned(
             looper, owned_replace_fd_pair[0], 11, ALOOPER_EVENT_INPUT,
             PersistentFdCallback, &current_callback_count,
             &current_release_count, OwnerRelease) == 1);
  // The old disposer must not remove a replacement that reused the same fd.
  assert(darwin_art::looper::RemoveFdIfOwned(
             looper, owned_replace_fd_pair[0], PersistentFdCallback,
             &stale_callback_count) == 0);
  const char replacement_byte = 'd';
  assert(write(owned_replace_fd_pair[1], &replacement_byte,
               sizeof(replacement_byte)) == sizeof(replacement_byte));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(stale_callback_count == 0 && current_callback_count == 1);
  assert(stale_release_count == 1 && current_release_count == 0);
  assert(darwin_art::looper::RemoveFdIfOwned(
             looper, owned_replace_fd_pair[0], PersistentFdCallback,
             &current_callback_count) == 1);
  assert(current_release_count == 1);
  assert(darwin_art::looper::RemoveFdIfOwned(
             looper, owned_replace_fd_pair[0], PersistentFdCallback,
             &current_callback_count) == 0);
  close(owned_replace_fd_pair[0]);
  close(owned_replace_fd_pair[1]);

  int generation_fd_pair[2] = {-1, -1};
  assert(pipe(generation_fd_pair) == 0);
  struct GenerationReplacementState {
    void* looper;
    int fd;
    int old_callback_count;
    int new_callback_count;
  } generation_state{looper, generation_fd_pair[0], 0, 0};
  auto generation_replacement = [](int, int, void* opaque) {
    auto* state = static_cast<GenerationReplacementState*>(opaque);
    ++state->old_callback_count;
    assert(darwin_art::looper::AddFd(
               state->looper, state->fd, 13, ALOOPER_EVENT_INPUT,
               [](int, int, void* data) {
                 ++*static_cast<int*>(data);
                 return 0;
               },
               &state->new_callback_count) == 1);
    return 0;
  };
  assert(darwin_art::looper::AddFd(
             looper, generation_fd_pair[0], 12, ALOOPER_EVENT_INPUT,
             generation_replacement, &generation_state) == 1);
  const char generation_byte = 'e';
  assert(write(generation_fd_pair[1], &generation_byte,
               sizeof(generation_byte)) == sizeof(generation_byte));
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  // pollOnce's old generation cleanup must not remove the replacement.
  assert(generation_state.old_callback_count == 1 &&
         generation_state.new_callback_count == 0);
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(generation_state.new_callback_count == 1);
  close(generation_fd_pair[0]);
  close(generation_fd_pair[1]);

  int reentrant_fd_pair[2] = {-1, -1};
  int other_fd_pair[2] = {-1, -1};
  assert(pipe(reentrant_fd_pair) == 0);
  assert(pipe(other_fd_pair) == 0);
  int reentrant_callback_count = 0;
  int other_callback_count = 0;
  assert(darwin_art::looper::AddFd(
             looper, other_fd_pair[0], 15, ALOOPER_EVENT_INPUT,
             PersistentFdCallback, &other_callback_count) == 1);
  ReentrantReleaseState reentrant_release{looper, other_fd_pair[0],
                                           &other_callback_count};
  assert(darwin_art_android_platform_add_fd_owned(
             looper, reentrant_fd_pair[0], 14, ALOOPER_EVENT_INPUT,
             PersistentFdCallback, &reentrant_callback_count,
             &reentrant_release, ReentrantOwnerRelease) == 1);
  assert(darwin_art::looper::RemoveFdIfOwned(
             looper, reentrant_fd_pair[0], PersistentFdCallback,
             &reentrant_callback_count) == 1);
  assert(reentrant_release.release_count == 1);
  assert(reentrant_release.nested_remove_result == 1);
  close(reentrant_fd_pair[0]);
  close(reentrant_fd_pair[1]);
  close(other_fd_pair[0]);
  close(other_fd_pair[1]);

  assert(darwin_art::looper::RemoveFdIfOwned(
             nullptr, 1, PersistentFdCallback, nullptr) == -1);
  assert(darwin_art::looper::RemoveFdIfOwned(
             looper, -1, PersistentFdCallback, nullptr) == -1);
  assert(darwin_art::looper::RemoveFdIfOwned(looper, 1, nullptr, nullptr) ==
         -1);

  CallbackState state;
  state.owner_thread = std::this_thread::get_id();
  state.choreographer = choreographer;
  std::thread poster([&] {
    AChoreographer_postFrameCallbackDelayed64(choreographer, Frame64, &state, 1);
    AChoreographer_postVsyncCallback(choreographer, Vsync, &state);
    AChoreographer_registerRefreshRateCallback(choreographer, Refresh, &state);
  });
  poster.join();

  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::milliseconds(500);
  for (;;) {
    (void)darwin_art::looper::PollCurrent(50);
    std::lock_guard<std::mutex> lock(g_event_mutex);
    if (state.frame64 == 2 && state.vsync == 1 && state.refresh == 1) break;
    if (std::chrono::steady_clock::now() >= deadline) return 2;
  }
  assert(state.frame_time > 0);
  assert(state.vsync_id > 0);
  assert(state.expected_time > state.frame_time);
  assert(state.deadline > state.frame_time);
  assert(!state.wrong_thread);

  std::vector<int> order;
  const auto now_nanos = MonotonicNanos();
  // Deliberately enqueue in reverse order; the owner must dispatch by the
  // absolute deadline, not insertion order.
  auto* later = new OrderedTask{&order, 2};
  auto* earlier = new OrderedTask{&order, 1};
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             looper, now_nanos + 5'000'000, OrderedTaskCallback, later,
             OrderedTaskRelease) == 1);
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             looper, now_nanos + 1'000'000, OrderedTaskCallback, earlier,
             OrderedTaskRelease) == 1);
  const auto order_deadline = std::chrono::steady_clock::now() +
                              std::chrono::milliseconds(100);
  while (order.size() < 2 && std::chrono::steady_clock::now() < order_deadline)
    (void)darwin_art::looper::PollCurrent(20);
  assert(order.size() == 2 && order[0] == 1 && order[1] == 2);

  int public_poll_count = 0;
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             looper, MonotonicNanos(), IncrementTask, &public_poll_count,
             NoopRelease) == 1);
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(public_poll_count == 1);
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             looper, MonotonicNanos(), IncrementTask, &public_poll_count,
             NoopRelease) == 1);
  assert(darwin_art_android_platform_poll_current_looper() >= 1);
  assert(public_poll_count == 2);
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             nullptr, MonotonicNanos(), IncrementTask, &public_poll_count,
             NoopRelease) == 0);

  int exception_released = 0;
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             looper, MonotonicNanos(), ThrowingTask, &exception_released,
             [](void* opaque) { ++*static_cast<int*>(opaque); }) == 1);
  // The callback exception is intentionally propagated, but the detached
  // payload must still be released by the actual owner before unwinding.
  bool caught = false;
  try {
    (void)ALooper_pollOnce(0, nullptr, nullptr, nullptr);
  } catch (const std::runtime_error&) {
    caught = true;
  }
  assert(caught && exception_released == 1);
  AChoreographer_unregisterRefreshRateCallback(choreographer, Refresh, &state);
  std::puts("actual Looper owner: FD generation/ownership, owner-thread timed callbacks, frame64/vsync/refresh PASS");
  return 0;
}
