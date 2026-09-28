#include <gui/BufferReleaseChannel.h>
#include <array>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>

int main() {
    using namespace android;
    using Message = gui::BufferReleaseChannel::Message;
    Message source{ReleaseCallbackId{17, 23}, Fence::NO_FENCE, 5};
    std::array<uint8_t, 64> bytes{};
    void* output = bytes.data();
    size_t available = bytes.size();
    int* outputFds = nullptr;
    size_t fdCount = 0;
    assert(source.flatten(output, available, outputFds, fdCount) == OK);
    const size_t wireSize = bytes.size() - available;
    assert(wireSize == source.getFlattenedSize());
    for (size_t length = 0; length < wireSize; ++length) {
        Message decoded;
        const void* input = bytes.data();
        size_t remaining = length;
        const int* inputFds = nullptr;
        size_t inputFdCount = 0;
        if (decoded.unflatten(input, remaining, inputFds, inputFdCount) == OK) {
            std::fprintf(stderr, "release message accepted truncated frame: %zu/%zu\n",
                         length, wireSize);
            return 1;
        }
    }
    Message decoded;
    const void* input = bytes.data();
    size_t remaining = wireSize;
    const int* inputFds = nullptr;
    assert(decoded.unflatten(input, remaining, inputFds, fdCount) == OK);
    assert(decoded.releaseCallbackId == source.releaseCallbackId);
    assert(decoded.maxAcquiredBufferCount == 5 && remaining == 0);
    int pipeFds[2];
    assert(pipe(pipeFds) == 0);
    const uint32_t oneFd = 1;
    std::memcpy(bytes.data(), &oneFd, sizeof(oneFd));
    for (size_t length = 0; length < wireSize; ++length) {
        {
            Message rejected;
            input = bytes.data();
            remaining = length;
            inputFds = &pipeFds[0];
            fdCount = 1;
            assert(rejected.unflatten(input, remaining, inputFds, fdCount) != OK);
            assert(inputFds == &pipeFds[0] && fdCount == 1);
        }
        assert(fcntl(pipeFds[0], F_GETFD) != -1);
    }
    close(pipeFds[0]);
    close(pipeFds[1]);
    std::puts("original release message: every truncated length rejected; complete frame PASS");
}
