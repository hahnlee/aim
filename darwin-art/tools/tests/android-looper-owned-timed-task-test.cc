#include "darwin_art_bionic_socket_broker.h"
#include "looper/android_looper_owner.h"

#include <android/looper.h>

#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <fcntl.h>
#include <map>
#include <mutex>
#include <poll.h>
#include <unistd.h>
#include <vector>

namespace {

std::mutex g_wake_mutex;
std::map<int, int> g_wake_writers;

struct TaskState {
  void* looper = nullptr;
  uint64_t token = 0;
  uint64_t nested_token = 0;
  int callback_count = 0;
  int release_count = 0;
  int nested_release_count = 0;
  int callback_cancel_result = -1;
  int nested_schedule_result = 0;
  bool schedule_nested_on_release = false;
};

void NoopCallback(void*) {}

void NestedRelease(void* opaque) {
  ++static_cast<TaskState*>(opaque)->nested_release_count;
}

void ReentrantRelease(void* opaque) {
  auto* state = static_cast<TaskState*>(opaque);
  ++state->release_count;
  if (!state->schedule_nested_on_release) return;
  state->schedule_nested_on_release = false;
  state->nested_schedule_result =
      darwin_art::looper::ScheduleTimedTaskAtOwned(
          state->looper, INT64_MAX, NoopCallback, state, NestedRelease,
          &state->nested_token);
}

void CancelFromCallback(void* opaque) {
  auto* state = static_cast<TaskState*>(opaque);
  ++state->callback_count;
  // Dispatch detaches the task before invoking this callback. Cancellation
  // therefore returns zero without claiming that the callback has quiesced.
  state->callback_cancel_result =
      darwin_art::looper::CancelTimedTaskIfOwned(state->looper, state->token);
}

void CountRelease(void* opaque) {
  ++static_cast<TaskState*>(opaque)->release_count;
}

}  // namespace

extern "C" int darwin_art_bionic_errno_load() {
  if (errno == EAGAIN || errno == EWOULDBLOCK) return 11;
  if (errno == EINTR) return 4;
  return errno;
}

extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t initial_value,
                                                          int) {
  int descriptors[2] = {-1, -1};
  if (pipe(descriptors) != 0) return -1;
  {
    std::lock_guard<std::mutex> lock(g_wake_mutex);
    g_wake_writers.emplace(descriptors[0], descriptors[1]);
  }
  if (initial_value != 0) {
    const uint64_t value = initial_value;
    if (write(descriptors[1], &value, sizeof(value)) != sizeof(value))
      return -1;
  }
  return descriptors[0];
}

extern "C" intptr_t darwin_art_bionic_socket_broker_read(int fd, void* bytes,
                                                            size_t count) {
  return read(fd, bytes, count);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, size_t count) {
  std::lock_guard<std::mutex> lock(g_wake_mutex);
  const auto found = g_wake_writers.find(fd);
  return found == g_wake_writers.end() ? -1 : write(found->second, bytes, count);
}

extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd* descriptors, size_t count, int timeout_ms) {
  std::vector<struct pollfd> host_descriptors;
  host_descriptors.reserve(count);
  for (size_t index = 0; index < count; ++index) {
    descriptors[index].revents = 0;
    host_descriptors.push_back(
        {descriptors[index].fd, descriptors[index].events, 0});
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

  TaskState canceled{looper};
  canceled.schedule_nested_on_release = true;
  assert(darwin_art::looper::ScheduleTimedTaskAtOwned(
             looper, INT64_MAX, NoopCallback, &canceled, ReentrantRelease,
             &canceled.token) == 1);
  assert(canceled.token != 0);
  assert(darwin_art::looper::CancelTimedTaskIfOwned(looper, canceled.token) ==
         1);
  assert(canceled.release_count == 1);
  assert(canceled.nested_schedule_result == 1 && canceled.nested_token >
         canceled.token);
  assert(darwin_art::looper::CancelTimedTaskIfOwned(looper,
                                                    canceled.nested_token) == 1);
  assert(canceled.nested_release_count == 1);
  assert(darwin_art::looper::CancelTimedTaskIfOwned(looper, canceled.token) ==
         0);

  TaskState in_flight{looper};
  assert(darwin_art::looper::ScheduleTimedTaskAtOwned(
             looper, 0, CancelFromCallback, &in_flight, CountRelease,
             &in_flight.token) == 1);
  assert(in_flight.token > canceled.nested_token);
  assert(darwin_art::looper::PollCurrent(0) == ALOOPER_POLL_CALLBACK);
  assert(in_flight.callback_count == 1 && in_flight.callback_cancel_result == 0);
  assert(in_flight.release_count == 1);
  assert(darwin_art::looper::CancelTimedTaskIfOwned(looper, in_flight.token) ==
         0);

  TaskState rejected{looper};
  uint64_t rejected_token = 123;
  assert(darwin_art::looper::ScheduleTimedTaskAtOwned(
             nullptr, 0, NoopCallback, &rejected, CountRelease,
             &rejected_token) == 0);
  assert(rejected_token == 0 && rejected.release_count == 1);
  errno = 0;
  assert(darwin_art::looper::ScheduleTimedTaskAtOwned(
             looper, 0, NoopCallback, &rejected, CountRelease, nullptr) == 0);
  assert(rejected.release_count == 2);
  assert(darwin_art::looper::CancelTimedTaskIfOwned(looper, 0) == 0);

  std::puts("owned timed task: monotonic token/cancel, detached callback, reentrant release PASS");
  return 0;
}
