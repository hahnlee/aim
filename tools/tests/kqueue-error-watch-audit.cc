// Read-only platform mechanism audit: no runtime policy or substitute watcher.
#include <sys/event.h>
#include <sys/socket.h>
#include <poll.h>
#include <unistd.h>
#include <cassert>
#include <cerrno>
#include <cstdio>

static void inspect(const char* kind, bool socket) {
    int fds[2];
    assert((socket ? socketpair(AF_UNIX, SOCK_STREAM, 0, fds) : pipe(fds)) == 0);
    const int queue = kqueue();
    assert(queue >= 0);
    kevent64_s change{};
    change.ident = fds[0];
    change.filter = EVFILT_EXCEPT;
    change.flags = EV_ADD | EV_ENABLE;
    change.fflags = NOTE_OOB;
    const int attached = kevent64(queue, &change, 1, nullptr, 0, 0, nullptr);
    printf("%s except attach=%d errno=%d\n", kind, attached, attached < 0 ? errno : 0);
    const int edgeQueue = kqueue();
    assert(edgeQueue >= 0);
    change.filter = EVFILT_READ;
    change.flags = EV_ADD | EV_ENABLE | EV_CLEAR;
    change.fflags = 0;
    assert(kevent64(edgeQueue, &change, 1, nullptr, 0, 0, nullptr) == 0);
    assert(write(fds[1], "x", 1) == 1);
    for (int phase = 0; phase < 2; ++phase) {
        kevent64_s event{};
        timespec zero{};
        const int count = kevent64(queue, nullptr, 0, &event, 1, 0, &zero);
        pollfd descriptor{fds[0], POLLPRI, 0};
        const int polled = poll(&descriptor, 1, 0);
        printf("%s %s kqueue=%d flags=%u fflags=%u pollpri=%d revents=%d\n",
               kind, phase == 0 ? "pending-data" : "peer-closed", count,
               event.flags, event.fflags, polled, descriptor.revents);
        for (int repeat = 0; repeat < 2; ++repeat) {
            kevent64_s edge{};
            const int n = kevent64(edgeQueue, nullptr, 0, &edge, 1, 0, &zero);
            printf("%s %s edge-repeat=%d count=%d flags=%u\n", kind,
                   phase == 0 ? "pending-data" : "peer-closed", repeat, n, edge.flags);
            assert(n == (repeat == 0 ? 1 : 0));
            if (repeat == 0) assert(bool(edge.flags & EV_EOF) == bool(phase));
        }
        if (phase == 0) assert(close(fds[1]) == 0);
    }
    assert(close(fds[0]) == 0 && close(queue) == 0 && close(edgeQueue) == 0);
}
int main() { inspect("socket", true); inspect("pipe", false); }
