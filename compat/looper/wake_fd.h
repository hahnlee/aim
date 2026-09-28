#pragma once
#include "wake.h"
#include <cerrno>
#include <cstdint>
#include <sys/types.h>

namespace aim::looper {
// A resource owner only. Android Looper remains the message/thread scheduler.
class WakeFd final {
public:
    WakeFd() = default;
    ~WakeFd() { aim_looper_wake_destroy(owner_); }
    WakeFd(const WakeFd&) = delete;
    WakeFd& operator=(const WakeFd&) = delete;
    WakeFd(WakeFd&&) = delete;
    WakeFd& operator=(WakeFd&&) = delete;

    bool initialize() {
        if (owner_) { errno = EALREADY; return false; }
        int error = aim_looper_wake_create(&owner_);
        if (error) errno = error;
        return error == 0;
    }
    int get() const { return aim_looper_wake_fd(owner_); }
    ssize_t signal(uint64_t increment) const {
        return result(aim_looper_wake_signal(owner_, increment));
    }
    ssize_t drain(uint64_t* out) const {
        return result(aim_looper_wake_drain(owner_, out));
    }
private:
    static ssize_t result(int error) {
        if (error) { errno = error; return -1; }
        return sizeof(uint64_t);
    }
    AimLooperWake* owner_ = nullptr;
};
}  // namespace aim::looper
