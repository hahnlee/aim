#include "readiness_queue.h"
#include "wake_fd.h"
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <thread>

int main() {
    using namespace darwin_art::looper;
    ReadinessQueue queue;
    WakeFd wake;
    assert(wake.initialize());
    DarwinArtLooperReadyEvent events[16]{};
    assert(!queue.valid());
    assert(queue.wait(events, 16, 0) == -1 && errno == EBADF);
    assert(queue.reset() && queue.valid());
    auto result = queue.transition(wake.get(), {0, 0, 0}, {7, 1, 0});
    assert(!result.operation_error && !result.rollback_error);
    std::thread sender([&] { assert(wake.signal(1) == 8); });
    assert(queue.wait(events, 16, 2000) == 1);
    sender.join();
    assert(events[0].token == 7 && events[0].flags == 1);
    result = queue.transition(wake.get(), {7, 1, 0}, {8, 8, 0});
    assert(result.operation_error == EINVAL && !result.rollback_error && queue.valid());
    assert(queue.wait(events, 16, 0) == 1 && events[0].token == 7);
    // Explicit reset discards registrations, not caller-owned descriptors.
    assert(queue.reset());
    assert(queue.wait(events, 16, 0) == 0);
    result = queue.transition(wake.get(), {0, 0, 0}, {9, 1, 0});
    assert(!result.operation_error && !result.rollback_error);
    assert(queue.wait(events, 16, 0) == 1 && events[0].token == 9);
    uint64_t counter;
    assert(wake.drain(&counter) == 8 && counter == 1);
    result = queue.transition(wake.get(), {9, 1, 0}, {0, 0, 0});
    assert(!result.operation_error && !result.rollback_error);
    assert(wake.signal(1) == 8);
    assert(queue.wait(events, 16, 0) == 0);
    puts("Looper C++ queue owner: initialization, wait/wake, failure preservation, reset/repopulate PASS");
}
