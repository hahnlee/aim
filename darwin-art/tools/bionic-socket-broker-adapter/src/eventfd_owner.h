#ifndef DARWIN_ART_BIONIC_EVENTFD_OWNER_H_
#define DARWIN_ART_BIONIC_EVENTFD_OWNER_H_

#include <cstddef>
#include <cstdint>
#include <memory>
#include <sys/types.h>

namespace darwin_art::eventfd {

struct State;

// The readiness token transport is deliberately injectable for owner tests.
// The production default calls send(2) on the socketpair endpoint.
using SignalFunction = ssize_t (*)(int fd, const void *bytes, size_t count,
                                   int flags, void *context);

std::shared_ptr<State> Create(uint64_t initial_value, bool semaphore,
                              int signal_fd, SignalFunction signal = nullptr,
                              void *signal_context = nullptr);

// Returns 0 when the counter is empty, already represented by a token, or a
// token was sent. Returns -1 and stores the host errno on a genuine transport
// failure. EAGAIN is not success: callers must retry rather than block.
int EnsureSignaled(const std::shared_ptr<State> &state, int *host_errno);

// These functions implement the Android eventfd read/write contract over a
// host socketpair. android_errno is written with an Android-compatible errno.
intptr_t Read(const std::shared_ptr<State> &state, int read_fd, void *bytes,
              size_t count, int *android_errno);
intptr_t Write(const std::shared_ptr<State> &state, const void *bytes,
               size_t count, int *android_errno);

// Retire the state when the broker closes the final open description.
// Descriptor reference counting and broker ownership remain outside this
// owner; duplicate descriptors continue to use the shared state until then.
void Retire(const std::shared_ptr<State> &state);

}  // namespace darwin_art::eventfd

#endif  // DARWIN_ART_BIONIC_EVENTFD_OWNER_H_
