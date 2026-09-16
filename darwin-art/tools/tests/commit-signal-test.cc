#include "surfaceflinger/commit_signal.h"
#include <cassert>
#include <future>
#include <memory>
#include <thread>
#include <cstdio>
using darwin_art::surfaceflinger::CommitSignal;
using namespace std::chrono_literals;
int main() {
    CommitSignal queued;
    queued.signal(); queued.signal();
    assert(queued.waitUntil(CommitSignal::Clock::now()));
    assert(queued.waitUntil(CommitSignal::Clock::now()));
    assert(!queued.waitUntil(CommitSignal::Clock::now()));
    const auto deadline = CommitSignal::Clock::now() + 20ms;
    assert(!queued.waitUntil(deadline));
    assert(CommitSignal::Clock::now() >= deadline);
    // A callback may arrive after its caller timed out. It still owns the
    // context, and signaling it must neither fabricate an earlier completion
    // nor access an already-destroyed wait primitive.
    auto owner = std::make_shared<CommitSignal>();
    std::weak_ptr<CommitSignal> weak = owner;
    std::promise<void> allow;
    auto gate = allow.get_future();
    std::thread callback([context = owner, gate = std::move(gate)]() mutable {
        gate.wait();
        context->signal();
    });
    assert(!owner->waitUntil(CommitSignal::Clock::now()));
    owner.reset(); assert(!weak.expired());
    allow.set_value(); callback.join(); assert(weak.expired());
    for (int i = 0; i < 50; ++i) {
        CommitSignal signal;
        std::thread worker([&] { signal.signal(); });
        assert(signal.waitUntil(CommitSignal::Clock::now() + 2s));
        worker.join();
        assert(!signal.waitUntil(CommitSignal::Clock::now()));
    }
    std::puts("commit signal: counted early/async callback, monotonic timeout, late callback lifetime PASS");
}
