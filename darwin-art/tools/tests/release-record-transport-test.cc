#include "surfaceflinger/release_record_transport.h"
#include <array>
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <sys/event.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

namespace rr = darwin_art::release_record;
using android::base::unique_fd;
using Record = std::array<unsigned char, 24>;

int main() {
    int pair[2];
    assert(rr::open(pair) == 0);
    unique_fd reader(pair[0]), writer(pair[1]);
    rr::Receiver receiver(24);
    Record expected{}, actual{};
    for (size_t i = 0; i < expected.size(); ++i) expected[i] = i + 1;
    unique_fd received;
    assert(receiver.receive(reader.get(), actual.data(), actual.size(), received) == -EAGAIN);
    // Force a partial record: draining it must leave no readable bytes/spin.
    assert(::write(writer.get(), expected.data(), 7) == 7);
    assert(receiver.receive(reader.get(), actual.data(), actual.size(), received) == -EAGAIN);
    unique_fd queue(kqueue());
    assert(queue.ok());
    struct kevent registration, event;
    EV_SET(&registration, reader.get(), EVFILT_READ, EV_ADD, 0, 0, nullptr);
    timespec immediate{};
    assert(kevent(queue.get(), &registration, 1, &event, 1, &immediate) == 0);
    assert(::write(writer.get(), expected.data() + 7, 17) == 17);
    assert(kevent(queue.get(), nullptr, 0, &event, 1, &immediate) == 1);
    assert(receiver.receive(reader.get(), actual.data(), actual.size(), received) == 0);
    assert(actual == expected && !received.ok());

    // Real cross-process SCM_RIGHTS, then close the original before receiving.
    int pipeFds[2];
    assert(pipe(pipeFds) == 0);
    unique_fd pipeReader(pipeFds[0]), pipeWriter(pipeFds[1]);
    const pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        reader.reset();
        pipeWriter.reset();
        const int result = rr::send(writer.get(), expected.data(), expected.size(), pipeReader.get());
        _exit(result == 0 ? 0 : 1);
    }
    pipeReader.reset();
    int status;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    assert(receiver.receive(reader.get(), actual.data(), actual.size(), received) == 0);
    assert(actual == expected && received.ok());
    assert(fcntl(received.get(), F_GETFD) & FD_CLOEXEC);
    assert(::write(pipeWriter.get(), "F", 1) == 1);
    char byte;
    assert(::read(received.get(), &byte, 1) == 1 && byte == 'F');
    received.reset();
    for (unsigned char i = 0; i < 64; ++i) {
        expected[0] = i;
        assert(rr::send(writer.get(), expected.data(), expected.size(), -1) == 0);
    }
    for (unsigned char i = 0; i < 64; ++i) {
        assert(receiver.receive(reader.get(), actual.data(), actual.size(), received) == 0);
        assert(actual[0] == i && !received.ok());
    }
    writer.reset();
    assert(kevent(queue.get(), nullptr, 0, &event, 1, &immediate) == 1);
    assert(event.flags & EV_EOF);
    assert(receiver.receive(reader.get(), actual.data(), actual.size(), received) == -ECONNRESET);

    // EOF in a partial frame must not produce a successful record.
    assert(rr::open(pair) == 0);
    reader.reset(pair[0]); writer.reset(pair[1]);
    rr::Receiver partial(24);
    assert(::write(writer.get(), expected.data(), 3) == 3);
    writer.reset();
    assert(partial.receive(reader.get(), actual.data(), actual.size(), received) == -EPROTO);

    // Reject excess rights and close *all* received rights on the error path.
    assert(rr::open(pair) == 0);
    reader.reset(pair[0]); writer.reset(pair[1]);
    assert(pipe(pipeFds) == 0);
    pipeReader.reset(pipeFds[0]); pipeWriter.reset(pipeFds[1]);
    int rights[2] = {pipeWriter.get(), pipeWriter.get()};
    alignas(cmsghdr) unsigned char control[CMSG_SPACE(sizeof(rights))]{};
    iovec io{expected.data(), expected.size()};
    msghdr message{};
    message.msg_iov = &io; message.msg_iovlen = 1;
    message.msg_control = control; message.msg_controllen = sizeof(control);
    auto* header = CMSG_FIRSTHDR(&message);
    header->cmsg_level = SOL_SOCKET; header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(rights));
    std::memcpy(CMSG_DATA(header), rights, sizeof(rights));
    assert(sendmsg(writer.get(), &message, 0) == 24);
    pipeWriter.reset();
    rr::Receiver malformed(24);
    assert(malformed.receive(reader.get(), actual.data(), actual.size(), received) == -EPROTO);
    assert(fcntl(pipeReader.get(), F_SETFL, O_NONBLOCK) == 0);
    assert(::read(pipeReader.get(), &byte, 1) == 0);
    // Saturation must report failure, never silently discard a complete record.
    assert(rr::open(pair) == 0);
    reader.reset(pair[0]); writer.reset(pair[1]);
    assert(fcntl(writer.get(), F_SETFL, O_NONBLOCK) == 0);
    int bufferSize = 4096;
    assert(setsockopt(writer.get(), SOL_SOCKET, SO_SNDBUF, &bufferSize, sizeof(bufferSize)) == 0);
    size_t accepted = 0;
    int sendStatus = 0;
    for (; accepted < 10000; ++accepted) {
        sendStatus = rr::send(writer.get(), expected.data(), expected.size(), -1);
        if (sendStatus != 0) break;
    }
    assert(accepted > 0 && accepted < 10000 && sendStatus == -EAGAIN);
    rr::Receiver saturated(24);
    for (size_t i = 0; i < accepted; ++i) {
        assert(saturated.receive(reader.get(), actual.data(), actual.size(), received) == 0);
        assert(actual == expected);
    }
    assert(saturated.receive(reader.get(), actual.data(), actual.size(), received) != 0);
    std::puts("release transport: fragmented records/readiness, 64 ordered records, cross-process FD/CLOEXEC, EOF, excess-FD cleanup PASS");
}
