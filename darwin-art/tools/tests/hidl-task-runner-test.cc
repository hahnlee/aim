#include <hidl/TaskRunner.h>

#include <cassert>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <mutex>
#include <thread>
#include <vector>

// Exercise the upstream executor, not a replacement queue or Binder transport.
int main() {
    using namespace std::chrono_literals;
    android::hardware::details::TaskRunner runner;
    assert(!runner.push([] {}));
    runner.start(2);
    assert(!runner.push(nullptr));
    std::mutex mutex;
    std::condition_variable changed;
    bool entered = false;
    bool release = false;
    std::vector<int> order;
    const auto caller = std::this_thread::get_id();
    assert(runner.push([&] {
        assert(std::this_thread::get_id() != caller);
        std::unique_lock lock(mutex);
        entered = true;
        changed.notify_all();
        changed.wait(lock, [&] { return release; });
        order.push_back(0);
    }));
    {
        std::unique_lock lock(mutex);
        assert(changed.wait_for(lock, 5s, [&] { return entered; }));
    }
    for (int i = 1; i <= 2; ++i) {
        assert(runner.push([&, i] {
            std::lock_guard lock(mutex);
            order.push_back(i);
            changed.notify_all();
        }));
    }
    // The worker is blocked in task 0: both pending slots are occupied.
    assert(!runner.push([] { assert(false); }));
    {
        std::unique_lock lock(mutex);
        release = true;
        changed.notify_all();
        assert(changed.wait_for(lock, 5s, [&] { return order.size() == 3; }));
        assert((order == std::vector<int>{0, 1, 2}));
    }
    // Drain before destruction; full-queue shutdown is not covered by this test.
    std::puts("original HIDL TaskRunner: worker thread, FIFO, bounded rejection PASS");
}
