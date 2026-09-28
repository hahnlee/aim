#pragma once
#include "readiness.h"
#include <memory>
#include <mutex>

namespace aim::looper {
// Host resource lifetime only. The Android Looper retains its callback registry
// and supplies the previous/next native filter state for each transition.
class ReadinessQueue final {
public:
    ReadinessQueue() = default;
    ~ReadinessQueue() = default; // Caller must quiesce method calls first.
    ReadinessQueue(const ReadinessQueue&) = delete;
    ReadinessQueue& operator=(const ReadinessQueue&) = delete;
    bool reset(); // New empty kernel set; caller repopulates Android registrations.
    bool valid() const;
    AimLooperTransitionResult transition(int fd,
        AimLooperRegistration previous, AimLooperRegistration next);
    int wait(AimLooperReadyEvent* events, uint32_t capacity, int timeoutMs);
private:
    using Owner = std::shared_ptr<AimLooperQueue>;
    mutable std::mutex mutex_;
    Owner owner_;
    bool poisoned_ = false;
};
}  // namespace aim::looper
