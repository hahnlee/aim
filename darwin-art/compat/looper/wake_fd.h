#pragma once
#include "wake.h"
#include <cerrno>
#include <cstdint>
#include <sys/types.h>

namespace darwin_art::looper {
// A resource owner only. Android Looper remains the message/thread scheduler.
class WakeFd final {
public:
    WakeFd() = default;
    ~WakeFd() { darwin_art_looper_wake_destroy(owner_); }
    WakeFd(const WakeFd&) = delete;
    WakeFd& operator=(const WakeFd&) = delete;
    WakeFd(WakeFd&&) = delete;
    WakeFd& operator=(WakeFd&&) = delete;

    bool initialize() {
        if (owner_) { errno = EALREADY; return false; }
        int error = darwin_art_looper_wake_create(&owner_);
        if (error) errno = error;
        return error == 0;
    }
    int get() const { return darwin_art_looper_wake_fd(owner_); }
    ssize_t signal(uint64_t increment) const {
        return result(darwin_art_looper_wake_signal(owner_, increment));
    }
    ssize_t drain(uint64_t* out) const {
        return result(darwin_art_looper_wake_drain(owner_, out));
    }
private:
    static ssize_t result(int error) {
        if (error) { errno = error; return -1; }
        return sizeof(uint64_t);
    }
    DarwinArtLooperWake* owner_ = nullptr;
};
}  // namespace darwin_art::looper
