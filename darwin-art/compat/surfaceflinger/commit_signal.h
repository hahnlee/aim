#pragma once
#include <chrono>
#include <condition_variable>
#include <cstdlib>
#include <limits>
#include <mutex>

namespace darwin_art::surfaceflinger {
// Darwin wait primitive only. The original transaction callback owns this
// object's lifetime and chooses the commit deadline; this does not submit,
// acknowledge or synthesize a SurfaceFlinger transaction.
class CommitSignal {
public:
    using Clock = std::chrono::steady_clock;
    void signal() {
        {
            std::lock_guard lock(mutex_);
            if (permits_ == std::numeric_limits<int>::max()) std::abort();
            ++permits_;
        }
        ready_.notify_one();
    }
    bool waitUntil(Clock::time_point deadline) {
        std::unique_lock lock(mutex_);
        if (!ready_.wait_until(lock, deadline, [this] { return permits_ != 0; })) return false;
        --permits_;
        return true;
    }
private:
    std::mutex mutex_;
    std::condition_variable ready_;
    int permits_ = 0;
};
}
