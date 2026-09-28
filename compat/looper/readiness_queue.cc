#include "readiness_queue.h"
#include <cerrno>

namespace aim::looper {
bool ReadinessQueue::reset() {
    AimLooperQueue* raw = nullptr;
    const int error = aim_looper_queue_create(&raw);
    if (error) { errno = error; return false; }
    Owner next(raw, aim_looper_queue_destroy);
    std::lock_guard lock(mutex_);
    owner_ = std::move(next);
    poisoned_ = false;
    return true;
}

bool ReadinessQueue::valid() const {
    std::lock_guard lock(mutex_);
    return owner_ && !poisoned_;
}

AimLooperTransitionResult ReadinessQueue::transition(int fd,
        AimLooperRegistration previous, AimLooperRegistration next) {
    std::lock_guard lock(mutex_);
    if (!owner_) return {EBADF, 0};
    if (poisoned_) return {EIO, EIO};
    const auto result = aim_looper_queue_transition(owner_.get(), fd, previous, next);
    if (result.rollback_error) poisoned_ = true;
    return result;
}

int ReadinessQueue::wait(AimLooperReadyEvent* events, uint32_t capacity, int timeoutMs) {
    Owner snapshot;
    {
        std::lock_guard lock(mutex_);
        if (!owner_ || poisoned_) { errno = owner_ ? EIO : EBADF; return -1; }
        snapshot = owner_;
    }
    uint32_t count = 0;
    const int error = aim_looper_queue_wait(snapshot.get(), events, capacity, timeoutMs, &count);
    // Hold the old kernel set alive during wait, but don't publish responses from
    // a set replaced or invalidated concurrently. Caller must retry/rebuild.
    std::lock_guard lock(mutex_);
    if (snapshot != owner_) { errno = ESTALE; return -1; }
    if (poisoned_) { errno = EIO; return -1; }
    if (error) { errno = error; return -1; }
    return static_cast<int>(count);
}
}  // namespace aim::looper
