#include <utils/Looper.h>
#include <utils/Timers.h>
#include <cassert>
#include <cstdio>
#include <thread>
#include <unistd.h>

using namespace android;
struct Handler final : MessageHandler {
    int received = 0;
    void handleMessage(const Message& message) override { received = message.what; }
};
int main() {
    alarm(10); // Isolated test bound, not a runtime shutdown timer.
    assert(!Looper::getForThread());
    auto looper = Looper::prepare(Looper::PREPARE_ALLOW_NON_CALLBACKS);
    assert(looper == Looper::getForThread());
    assert(looper == Looper::prepare(Looper::PREPARE_ALLOW_NON_CALLBACKS));
    std::thread isolated([&] {
        assert(!Looper::getForThread());
        auto other = Looper::prepare(0);
        assert(other != looper);
        Looper::setForThread(nullptr);
    });
    isolated.join();
    auto handler = sp<Handler>::make();
    const auto deadline = systemTime(SYSTEM_TIME_MONOTONIC) + 10'000'000;
    looper->sendMessageAtTime(deadline, handler, Message(42));
    for (int i = 0; i < 4 && handler->received == 0; ++i) looper->pollOnce(1000);
    assert(handler->received == 42);
    assert(systemTime(SYSTEM_TIME_MONOTONIC) >= deadline);
    int pipeFds[2];
    assert(pipe(pipeFds) == 0);
    int callbacks = 0;
    auto callback = [](int fd, int events, void* data) -> int {
        assert(events & Looper::EVENT_INPUT);
        char byte;
        assert(read(fd, &byte, 1) == 1 && byte == 'x');
        ++*static_cast<int*>(data);
        return 0; // Original Looper removes this callback.
    };
    assert(looper->addFd(pipeFds[0], 0, Looper::EVENT_INPUT, callback, &callbacks) == 1);
    assert(write(pipeFds[1], "x", 1) == 1);
    for (int i = 0; i < 4 && callbacks == 0; ++i) looper->pollOnce(1000);
    assert(callbacks == 1);
    assert(looper->removeFd(pipeFds[0]) == 0);
    assert(close(pipeFds[0]) == 0 && close(pipeFds[1]) == 0);
    assert(pipe(pipeFds) == 0);
    int closedCallbacks = 0;
    auto closeCallback = [](int fd, int events, void* data) -> int {
        assert(events & Looper::EVENT_INPUT);
        assert(close(fd) == 0);
        ++*static_cast<int*>(data);
        return 0; // AOSP must remove the now-closed FD and schedule rebuild.
    };
    assert(looper->addFd(pipeFds[0], 0, Looper::EVENT_INPUT, closeCallback, &closedCallbacks) == 1);
    assert(write(pipeFds[1], "x", 1) == 1);
    for (int i = 0; i < 4 && closedCallbacks == 0; ++i) looper->pollOnce(1000);
    assert(closedCallbacks == 1);
    assert(looper->removeFd(pipeFds[0]) == 0);
    assert(close(pipeFds[1]) == 0);
    // Consume original Looper's scheduled rebuild/wake, then prove it still works.
    looper->pollOnce(0);
    assert(pipe(pipeFds) == 0);
    assert(looper->addFd(pipeFds[0], 123, Looper::EVENT_INPUT, nullptr, nullptr) == 1);
    assert(write(pipeFds[1], "x", 1) == 1);
    int delivered = Looper::POLL_WAKE;
    for (int i = 0; i < 4 && delivered != 123; ++i) delivered = looper->pollOnce(1000);
    assert(delivered == 123);
    assert(looper->removeFd(pipeFds[0]) == 1);
    assert(close(pipeFds[0]) == 0 && close(pipeFds[1]) == 0);
    assert(pipe(pipeFds) == 0);
    assert(looper->addFd(pipeFds[0], 124, 0, nullptr, nullptr) == 1);
    assert(write(pipeFds[1], "x", 1) == 1);
    assert(looper->pollOnce(10) == Looper::POLL_TIMEOUT);
    assert(close(pipeFds[1]) == 0);
    int outputEvents = 0;
    for (int i = 0; i < 2; ++i) {
        assert(looper->pollOnce(1000, nullptr, &outputEvents, nullptr) == 124);
        assert(outputEvents == Looper::EVENT_HANGUP);
    }
    // Original addFd replacement changes both native mode and AOSP sequence.
    assert(looper->addFd(pipeFds[0], 125, Looper::EVENT_INPUT, nullptr, nullptr) == 1);
    for (int i = 0; i < 2; ++i) {
        assert(looper->pollOnce(1000, nullptr, &outputEvents, nullptr) == 125);
        assert(outputEvents == (Looper::EVENT_INPUT | Looper::EVENT_HANGUP));
    }
    assert(looper->addFd(pipeFds[0], 126, 0, nullptr, nullptr) == 1);
    assert(looper->pollOnce(1000, nullptr, &outputEvents, nullptr) == 126);
    assert(outputEvents == Looper::EVENT_HANGUP);
    assert(looper->removeFd(pipeFds[0]) == 1);
    assert(close(pipeFds[0]) == 0);
    std::thread wake([&] { looper->wake(); });
    assert(looper->pollOnce(1000) == Looper::POLL_WAKE);
    wake.join();
    Looper::setForThread(nullptr);
    looper.clear();
    alarm(0);
    puts("Original AOSP Looper: thread-local prepare, timed message, FD callback/removal, wake/destruction PASS");
}
