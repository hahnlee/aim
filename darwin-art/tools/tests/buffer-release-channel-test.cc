#include <gui/BufferReleaseChannel.h>
#include <cassert>
#include <cstdio>
#include <binder/Parcel.h>
#include <binder/RpcSession.h>
#include <fcntl.h>
#include <unistd.h>

int main() {
    using namespace android;
    std::unique_ptr<gui::BufferReleaseChannel::ConsumerEndpoint> consumer;
    std::shared_ptr<gui::BufferReleaseChannel::ProducerEndpoint> producer;
    const auto status = gui::BufferReleaseChannel::open("contract-test", consumer, producer);
    if (status != OK) {
        std::fprintf(stderr, "original BufferReleaseChannel::open failed: %d\n", status);
        return 1;
    }
    ReleaseCallbackId actual;
    sp<Fence> fence;
    uint32_t count = 0;
    assert(consumer->readReleaseFence(actual, fence, count) == WOULD_BLOCK);
    // The real producer endpoint must survive original Parcel FD duplication.
    auto session = RpcSession::make();
    session->setFileDescriptorTransportMode(RpcSession::FileDescriptorTransportMode::UNIX);
    auto restored = std::make_shared<gui::BufferReleaseChannel::ProducerEndpoint>();
    {
        Parcel parcel;
        parcel.markForRpc(session);
        assert(producer->writeToParcel(&parcel) == OK);
        producer.reset();
        parcel.setDataPosition(0);
        assert(restored->readFromParcel(&parcel) == OK);
    }
    producer = std::move(restored);
    int pipeFds[2];
    assert(pipe(pipeFds) == 0);
    auto sourceFence = sp<Fence>::make(pipeFds[0]);
    for (uint64_t i = 0; i < 64; ++i) {
        assert(producer->writeReleaseFence(ReleaseCallbackId{i, i + 1}, sourceFence, i + 2) == OK);
    }
    sourceFence.clear();
    for (uint64_t i = 0; i < 64; ++i) {
        assert(consumer->readReleaseFence(actual, fence, count) == OK);
        assert(actual == ReleaseCallbackId(i, i + 1) && count == i + 2);
        assert(fence && fence->isValid());
        assert(fcntl(fence->get(), F_GETFD) & FD_CLOEXEC);
    }
    assert(write(pipeFds[1], "R", 1) == 1);
    char value;
    assert(read(fence->get(), &value, 1) == 1 && value == 'R');
    close(pipeFds[1]);
    const ReleaseCallbackId expected{0x100000002ULL, 0x300000004ULL};
    assert(producer->writeReleaseFence(expected, Fence::NO_FENCE, 3) == OK);
    assert(consumer->readReleaseFence(actual, fence, count) == OK);
    assert(actual == expected && count == 3 && fence && !fence->isValid());
    assert(consumer->readReleaseFence(actual, fence, count) == WOULD_BLOCK);
    std::puts("original BufferReleaseChannel open/Parcel ownership/64 FD records/empty queue PASS");
}
