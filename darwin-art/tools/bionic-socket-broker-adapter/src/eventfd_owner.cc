#include "eventfd_owner.h"

#include "darwin_art_bionic_errno.h"

#include <cerrno>
#include <cstring>
#include <mutex>
#include <new>
#include <sys/socket.h>

namespace darwin_art::eventfd {
namespace {

constexpr uint64_t kMaximumCounter = UINT64_MAX - 1;
constexpr size_t kEventFdWordSize = sizeof(uint64_t);

// Keep the Android-facing owner on the repository's name-derived errno
// provider. Unmapped host values are a genuine I/O failure, not a Darwin code
// that can be exposed to the guest.
int AndroidErrnoForHost(int host_errno) {
  int32_t android_errno = 5;  // Android EIO fallback for an unmapped host code.
  if (darwin_art_bionic_errno_from_darwin(host_errno, &android_errno) != 0)
    return android_errno;
  return 5;
}

ssize_t DefaultSignal(int fd, const void *bytes, size_t count, int flags,
                      void *) {
  return send(fd, bytes, count, flags);
}

int SignalLocked(State &state);

}  // namespace

struct State {
  explicit State(uint64_t initial_value, bool semaphore_value, int signal,
                 SignalFunction signal_function, void *signal_context_value)
      : counter(initial_value),
        semaphore(semaphore_value),
        signal_fd(signal),
        signal_function(signal_function != nullptr ? signal_function
                                                     : DefaultSignal),
        signal_context(signal_context_value) {}

  std::mutex mutex;
  uint64_t counter;
  bool semaphore;
  bool signaled = false;
  int signal_fd;
  SignalFunction signal_function;
  void *signal_context;
};

std::shared_ptr<State> Create(uint64_t initial_value, bool semaphore,
                              int signal_fd, SignalFunction signal,
                              void *signal_context) {
  if (initial_value > kMaximumCounter)
    return nullptr;
  try {
    return std::make_shared<State>(initial_value, semaphore, signal_fd, signal,
                                   signal_context);
  } catch (const std::bad_alloc &) {
    return nullptr;
  }
}

namespace {

int SignalLocked(State &state) {
  if (state.counter == 0 || state.signaled)
    return 0;
  const uint64_t token = 1;
  for (;;) {
    const ssize_t result = state.signal_function(
        state.signal_fd, &token, sizeof(token), MSG_DONTWAIT,
        state.signal_context);
    if (result == static_cast<ssize_t>(sizeof(token))) {
      state.signaled = true;
      return 0;
    }
    if (result < 0 && errno == EINTR)
      continue;
    if (result < 0)
      return errno != 0 ? errno : EIO;
    return EIO;
  }
}

int ReceiveToken(int read_fd) {
  uint64_t token = 0;
  for (;;) {
    const ssize_t result = recv(read_fd, &token, sizeof(token), MSG_DONTWAIT);
    if (result == static_cast<ssize_t>(sizeof(token)))
      return 0;
    if (result < 0 && errno == EINTR)
      continue;
    if (result < 0)
      return errno != 0 ? errno : EIO;
    return EIO;
  }
}

}  // namespace

int EnsureSignaled(const std::shared_ptr<State> &state, int *host_errno) {
  if (host_errno == nullptr)
    return -1;
  *host_errno = 0;
  if (state == nullptr)
    return 0;
  std::lock_guard lock(state->mutex);
  const int error = SignalLocked(*state);
  if (error != 0) {
    *host_errno = error;
    return -1;
  }
  return 0;
}

intptr_t Read(const std::shared_ptr<State> &state, int read_fd, void *bytes,
              size_t count, int *android_errno) {
  if (android_errno == nullptr)
    return -1;
  if (state == nullptr) {
    *android_errno = 22;
    return -1;
  }
  if (count < kEventFdWordSize || bytes == nullptr) {
    *android_errno = 22;
    return -1;
  }

  std::lock_guard lock(state->mutex);
  if (state->counter == 0) {
    *android_errno = 11;
    return -1;
  }

  const uint64_t value = state->semaphore ? 1 : state->counter;

  // eventfd readiness is level-triggered. A semaphore read with a remaining
  // count must leave the single queued token in place; draining and trying to
  // enqueue another token creates an avoidable failure window and can strand
  // a positive counter from a blocked poll.
  if (!(state->semaphore && state->counter > 1)) {
    if (!state->signaled) {
      const int signal_error = SignalLocked(*state);
      if (signal_error != 0) {
        *android_errno = AndroidErrnoForHost(signal_error);
        return -1;
      }
    }
    // Keep the Android counter untouched if the readiness token disappeared;
    // this is recoverable by a later EnsureSignaled call.
    const int receive_error = ReceiveToken(read_fd);
    if (receive_error != 0) {
      *android_errno = AndroidErrnoForHost(receive_error);
      return -1;
    }
    state->signaled = false;
  }

  state->counter -= value;
  std::memcpy(bytes, &value, kEventFdWordSize);
  *android_errno = 0;
  return static_cast<intptr_t>(kEventFdWordSize);
}

intptr_t Write(const std::shared_ptr<State> &state, const void *bytes,
               size_t count, int *android_errno) {
  if (android_errno == nullptr)
    return -1;
  if (state == nullptr) {
    *android_errno = 22;
    return -1;
  }
  if (count != kEventFdWordSize || bytes == nullptr) {
    *android_errno = 22;
    return -1;
  }
  uint64_t value = 0;
  std::memcpy(&value, bytes, kEventFdWordSize);
  if (value == UINT64_MAX) {
    *android_errno = 22;
    return -1;
  }

  std::lock_guard lock(state->mutex);
  if (value > kMaximumCounter - state->counter) {
    *android_errno = 11;
    return -1;
  }
  const uint64_t previous = state->counter;
  state->counter += value;

  // Retry a lost token even when this write did not transition from empty.
  // Roll back the addition on failure so a caller retry cannot double-count.
  const int signal_error = SignalLocked(*state);
  if (signal_error != 0) {
    state->counter = previous;
    *android_errno = AndroidErrnoForHost(signal_error);
    return -1;
  }
  *android_errno = 0;
  return static_cast<intptr_t>(kEventFdWordSize);
}

void Retire(const std::shared_ptr<State> &state) {
  if (state == nullptr)
    return;
  std::lock_guard lock(state->mutex);
  state->counter = 0;
  state->signaled = false;
  state->signal_fd = -1;
}

}  // namespace darwin_art::eventfd
