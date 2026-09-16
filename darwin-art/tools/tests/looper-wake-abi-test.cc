#include "wake_fd.h"
#include <cassert>
#include <cstdio>
#include <poll.h>
#include <thread>
#include <unistd.h>
#include <type_traits>

using darwin_art::looper::WakeFd;
static_assert(!std::is_copy_constructible_v<WakeFd>);
static_assert(!std::is_move_constructible_v<WakeFd>);
int main() {
    assert(darwin_art_looper_wake_create(nullptr) == EINVAL);
    darwin_art_looper_wake_destroy(nullptr);
    int observer = -1;
    {
        WakeFd wake;
        assert(wake.get() == -1);
        assert(wake.initialize());
        assert(!wake.initialize() && errno == EALREADY);
        observer = dup(wake.get());
        assert(observer >= 0);
        uint64_t counter = 42;
        assert(wake.drain(&counter) == -1 && errno == EAGAIN && counter == 42);
        assert(wake.signal(UINT64_MAX) == -1 && errno == EINVAL);
        std::thread sender([&] {
            for (int i = 0; i < 10000; ++i) assert(wake.signal(1) == 8);
        });
        uint64_t total = 0;
        while (total < 10000) {
            pollfd event{wake.get(), POLLIN, 0};
            assert(poll(&event, 1, 2000) == 1);
            assert(event.revents & POLLIN);
            assert(wake.drain(&counter) == 8);
            total += counter;
        }
        sender.join();
        assert(total == 10000);
        pollfd event{wake.get(), POLLIN, 0};
        assert(poll(&event, 1, 0) == 0);
    }
    // Peer EOF proves Rust owner destruction without checking reused FD numbers.
    char byte;
    assert(read(observer, &byte, 1) == 0);
    assert(close(observer) == 0);
    puts("Looper wake C++/Rust ABI: errno, 10000 signals, poll, RAII destruction PASS");
}
