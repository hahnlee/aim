#pragma once
#include <stdint.h>
// AOSP Looper's internal event representation; no Linux syscall exports.
enum { EPOLLIN = 1, EPOLLOUT = 4, EPOLLERR = 8, EPOLLHUP = 16 };
enum { EPOLL_CTL_ADD = 1, EPOLL_CTL_DEL = 2, EPOLL_CTL_MOD = 3 };
struct epoll_event {
    uint32_t events;
    union { uint64_t u64; } data;
};
