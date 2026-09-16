#include "readiness.h"
#include "wake_fd.h"
#include <cassert>
#include <cstddef>
#include <climits>
#include <cstdio>
#include <memory>
#include <thread>
#include <sys/socket.h>
#include <unistd.h>
static_assert(sizeof(DarwinArtLooperReadyEvent) == 16);
static_assert(offsetof(DarwinArtLooperReadyEvent, flags) == 8);
static_assert(sizeof(DarwinArtLooperRegistration) == 16);
static_assert(offsetof(DarwinArtLooperRegistration, mask) == 8);
static_assert(sizeof(DarwinArtLooperTransitionResult) == 8);
int main() {
    DarwinArtLooperQueue* raw = nullptr;
    assert(darwin_art_looper_queue_create(&raw) == 0);
    std::unique_ptr<DarwinArtLooperQueue, decltype(&darwin_art_looper_queue_destroy)>
        queue(raw, darwin_art_looper_queue_destroy);
    const int invalidChange = darwin_art_looper_queue_change(raw, INT_MAX, 1, 2, 1);
    assert(invalidChange == EBADF);
    const auto invalidFd = darwin_art_looper_queue_transition(raw, INT_MAX, {1, 1, 0}, {0, 0, 0});
    assert(invalidFd.operation_error == EBADF && invalidFd.rollback_error == 0);
    darwin_art::looper::WakeFd wake;
    assert(wake.initialize());
    assert(darwin_art_looper_queue_change(raw, wake.get(), 3, 0, 1) == EINVAL);
    assert(darwin_art_looper_queue_change(raw, wake.get(), 1, 2, 1) == ENOENT);
    assert(darwin_art_looper_queue_change(raw, wake.get(), 1, 0, UINT64_MAX) == 0);
    DarwinArtLooperReadyEvent events[16]{};
    uint32_t count = 99;
    assert(darwin_art_looper_queue_wait(raw, events, 16, 0, &count) == 0 && count == 0);
    std::thread writer([&] { assert(wake.signal(1) == 8); });
    assert(darwin_art_looper_queue_wait(raw, events, 16, 2000, &count) == 0);
    writer.join();
    assert(count == 1 && events[0].token == UINT64_MAX && events[0].flags == 1);
    assert(events[0].reserved == 0);
    assert(darwin_art_looper_queue_change(raw, wake.get(), 1, 2, 42) == 0);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 0, &count) == 0);
    assert(count == 1 && events[0].token == 42);
    uint64_t value;
    assert(wake.drain(&value) == 8 && value == 1);
    assert(darwin_art_looper_queue_change(raw, wake.get(), 1, 1, 0) == 0);
    assert(darwin_art_looper_queue_change(raw, wake.get(), 1, 1, 0) == ENOENT);
    count = 99;
    assert(darwin_art_looper_queue_wait(raw, events, 16, -2, &count) == EINVAL && count == 0);
    int pair[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    assert(darwin_art_looper_queue_change(raw, pair[0], 1, 0, 77) == 0);
    assert(darwin_art_looper_queue_change(raw, pair[0], 2, 0, 77) == 0);
    assert(write(pair[1], "x", 1) == 1);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 1000, &count) == 0);
    assert(count == 1 && events[0].token == 77 && events[0].flags == 3);
    // A different sequence must not be collapsed into the old response.
    assert(darwin_art_looper_queue_change(raw, pair[0], 2, 2, 78) == 0);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 1000, &count) == 0);
    assert(count == 2 && events[0].token != events[1].token);
    assert(darwin_art_looper_queue_change(raw, pair[0], 2, 1, 0) == 0);
    char byte;
    assert(read(pair[0], &byte, 1) == 1);
    assert(close(pair[1]) == 0);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 1000, &count) == 0);
    assert(count == 1 && events[0].token == 77 && events[0].flags == 5);
    assert(close(pair[0]) == 0);
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    auto transition = [&](DarwinArtLooperRegistration oldState, DarwinArtLooperRegistration newState) {
        return darwin_art_looper_queue_transition(raw, pair[0], oldState, newState);
    };
    auto result = transition({0, 0, 0}, {88, 3, 0});
    assert(result.operation_error == 0 && result.rollback_error == 0);
    result = transition({88, 3, 0}, {89, 5, 0});
    assert(result.operation_error == EINVAL && result.rollback_error == 0);
    // Invalid masks must leave original kernel registration untouched.
    assert(write(pair[1], "z", 1) == 1);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 1000, &count) == 0);
    assert(count == 1 && events[0].token == 88 && events[0].flags == 3);
    result = transition({88, 3, 0}, {89, 1, 0});
    assert(result.operation_error == 0 && result.rollback_error == 0);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 1000, &count) == 0);
    assert(count == 1 && events[0].token == 89 && events[0].flags == 1);
    result = transition({89, 1, 0}, {0, 0, 0});
    assert(result.operation_error == 0 && result.rollback_error == 0);
    result = transition({89, 1, 0}, {90, 3, 0});
    assert(result.operation_error == ENOENT && result.rollback_error == 0);
    assert(darwin_art_looper_queue_wait(raw, events, 16, 0, &count) == 0 && count == 0);
    result = transition({0, 0, 0}, {90, 4, 0});
    assert(result.operation_error == 0 && result.rollback_error == 0);
    // Unread data does not leak READ through the error-only ABI mask.
    assert(darwin_art_looper_queue_wait(raw, events, 16, 0, &count) == 0 && count == 0);
    assert(close(pair[1]) == 0);
    for (int i = 0; i < 2; ++i) {
        assert(darwin_art_looper_queue_wait(raw, events, 16, 1000, &count) == 0);
        assert(count == 1 && events[0].token == 90 && events[0].flags == 4);
    }
    result = transition({90, 4, 0}, {91, 1, 0});
    assert(result.operation_error == 0 && result.rollback_error == 0);
    for (int i = 0; i < 2; ++i) {
        assert(darwin_art_looper_queue_wait(raw, events, 16, 0, &count) == 0);
        assert(count == 1 && events[0].token == 91 && events[0].flags == 5);
    }
    result = transition({91, 1, 0}, {0, 0, 0});
    assert(result.operation_error == 0 && result.rollback_error == 0);
    assert(close(pair[0]) == 0);
    queue.reset();
    assert(wake.signal(2) == 8 && wake.drain(&value) == 8 && value == 2);
    puts("Looper readiness C++/Rust ABI: layout, token, wake, timeout, removal, merged masks/EOF, ownership PASS");
}
