#include "eventfd_owner.h"

#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

namespace {

constexpr int kAndroidEagain = 11;
constexpr int kAndroidEnobufs = 105;

struct SignalScript {
  int calls = 0;
  int failures = 0;
  int failure_errno = EAGAIN;
  bool eintr_once = false;
};

ssize_t ScriptSignal(int fd, const void *bytes, size_t count, int flags,
                     void *opaque) {
  auto *script = static_cast<SignalScript *>(opaque);
  ++script->calls;
  if (script->eintr_once) {
    script->eintr_once = false;
    errno = EINTR;
    return -1;
  }
  if (script->failures != 0) {
    --script->failures;
    errno = script->failure_errno;
    return -1;
  }
  return send(fd, bytes, count, flags);
}

void MakeSocketPair(int fds[2]) {
  assert(socketpair(AF_UNIX, SOCK_DGRAM, 0, fds) == 0);
  assert(fcntl(fds[0], F_SETFL, fcntl(fds[0], F_GETFL) | O_NONBLOCK) == 0);
  assert(fcntl(fds[1], F_SETFL, fcntl(fds[1], F_GETFL) | O_NONBLOCK) == 0);
}

int Ready(int fd) {
  pollfd descriptor{fd, POLLIN, 0};
  assert(poll(&descriptor, 1, 0) >= 0);
  return descriptor.revents & POLLIN;
}

intptr_t Write(const std::shared_ptr<darwin_art::eventfd::State> &state,
               uint64_t value, int *error) {
  return darwin_art::eventfd::Write(state, &value, sizeof(value), error);
}

uint64_t Read(const std::shared_ptr<darwin_art::eventfd::State> &state, int fd,
              int *error) {
  uint64_t value = 0;
  assert(darwin_art::eventfd::Read(state, fd, &value, sizeof(value), error) ==
         static_cast<intptr_t>(sizeof(value)));
  return value;
}

void TestCounterAndZero() {
  int fds[2];
  MakeSocketPair(fds);
  auto state = darwin_art::eventfd::Create(0, false, fds[1]);
  int error = 0;
  assert(Ready(fds[0]) == 0);
  assert(Write(state, 0, &error) == 8 && error == 0);
  assert(Ready(fds[0]) == 0);
  assert(Write(state, 2, &error) == 8 && error == 0);
  assert(Ready(fds[0]) != 0);
  assert(Read(state, fds[0], &error) == 2 && error == 0);
  assert(Ready(fds[0]) == 0);
  close(fds[0]);
  close(fds[1]);
}

void TestSemaphoreAndDup() {
  int fds[2];
  MakeSocketPair(fds);
  auto state = darwin_art::eventfd::Create(2, true, fds[1]);
  int error = 0;
  assert(darwin_art::eventfd::EnsureSignaled(state, &error) == 0);
  const int duplicate = dup(fds[0]);
  assert(duplicate >= 0);
  assert(Read(state, duplicate, &error) == 1 && error == 0);
  // Closing the original descriptor must not retire the shared open
  // description; a broker duplicate remains usable until its final close.
  close(fds[0]);
  assert(Ready(duplicate) != 0);
  assert(Read(state, duplicate, &error) == 1 && error == 0);
  assert(Ready(duplicate) == 0);
  close(duplicate);
  close(fds[1]);
}

void TestOverflow() {
  int fds[2];
  MakeSocketPair(fds);
  assert(darwin_art::eventfd::Create(UINT64_MAX, false, fds[1]) == nullptr);
  auto state = darwin_art::eventfd::Create(UINT64_MAX - 1, false, fds[1]);
  int error = 0;
  assert(darwin_art::eventfd::EnsureSignaled(state, &error) == 0);
  assert(Write(state, 1, &error) == -1 && error == kAndroidEagain);
  uint64_t ignored = 0;
  assert(darwin_art::eventfd::Read(state, fds[0], &ignored, sizeof(ignored),
                                   &error) == 8);
  assert(ignored == UINT64_MAX - 1);
  close(fds[0]);
  close(fds[1]);
}

void TestEINTRRetry() {
  int fds[2];
  MakeSocketPair(fds);
  SignalScript script;
  script.eintr_once = true;
  auto state = darwin_art::eventfd::Create(0, false, fds[1], ScriptSignal,
                                           &script);
  int error = 0;
  assert(Write(state, 1, &error) == 8 && error == 0);
  assert(script.calls == 2);
  assert(Read(state, fds[0], &error) == 1 && error == 0);
  close(fds[0]);
  close(fds[1]);
}

void TestFailedSignalRollsBackAndRetries() {
  int fds[2];
  MakeSocketPair(fds);
  SignalScript script;
  script.failures = 1;
  script.failure_errno = ENOBUFS;
  auto state = darwin_art::eventfd::Create(0, false, fds[1], ScriptSignal,
                                           &script);
  int error = 0;
  assert(Write(state, 9, &error) == -1 && error == kAndroidEnobufs);
  assert(Ready(fds[0]) == 0);
  uint64_t value = 0;
  assert(darwin_art::eventfd::Read(state, fds[0], &value, sizeof(value),
                                   &error) == -1 &&
         error == kAndroidEagain);
  assert(Write(state, 9, &error) == 8 && error == 0);
  assert(Read(state, fds[0], &error) == 9 && error == 0);
  close(fds[0]);
  close(fds[1]);
}

void TestSemaphoreReadKeepsReadinessOnSignalFailure() {
  int fds[2];
  MakeSocketPair(fds);
  SignalScript script;
  auto state = darwin_art::eventfd::Create(2, true, fds[1], ScriptSignal,
                                           &script);
  int error = 0;
  assert(darwin_art::eventfd::EnsureSignaled(state, &error) == 0);
  script.failures = 1;
  assert(Read(state, fds[0], &error) == 1 && error == 0);
  assert(script.calls == 1);
  assert(Ready(fds[0]) != 0);
  assert(Read(state, fds[0], &error) == 1 && error == 0);
  assert(Ready(fds[0]) == 0);
  close(fds[0]);
  close(fds[1]);
}

}  // namespace

int main() {
  TestCounterAndZero();
  TestSemaphoreAndDup();
  TestOverflow();
  TestEINTRRetry();
  TestFailedSignalRollsBackAndRetries();
  TestSemaphoreReadKeepsReadinessOnSignalFailure();
  std::puts("eventfd-owner: PASS real-socket counter/semaphore/zero/overflow/dup/"
            "poll EINTR and signal-failure contracts");
  return 0;
}
