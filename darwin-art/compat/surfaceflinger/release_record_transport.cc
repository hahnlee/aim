#include "release_record_transport.h"

#include <array>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

namespace darwin_art::release_record {

int open(int endpoints[2]) {
    endpoints[0] = endpoints[1] = -1;
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, pair) != 0) return -errno;
    android::base::unique_fd first(pair[0]), second(pair[1]);
    for (int fd : pair) {
        const int flags = fcntl(fd, F_GETFD);
        if (flags == -1 || fcntl(fd, F_SETFD, flags | FD_CLOEXEC) == -1) return -errno;
        const int yes = 1;
        if (setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, sizeof(yes)) == -1) return -errno;
    }
    endpoints[0] = first.release();
    endpoints[1] = second.release();
    return 0;
}

int send(int socket, const void* bytes, size_t size, int descriptor) {
    if (!bytes || size == 0) return -EINVAL;
    size_t sent = 0;
    while (sent < size) {
        iovec io{const_cast<unsigned char*>(static_cast<const unsigned char*>(bytes)) + sent,
                 size - sent};
        alignas(cmsghdr) std::array<unsigned char, CMSG_SPACE(sizeof(int))> control{};
        msghdr message{};
        message.msg_iov = &io;
        message.msg_iovlen = 1;
        if (sent == 0 && descriptor >= 0) {
            message.msg_control = control.data();
            message.msg_controllen = control.size();
            cmsghdr* header = CMSG_FIRSTHDR(&message);
            header->cmsg_level = SOL_SOCKET;
            header->cmsg_type = SCM_RIGHTS;
            header->cmsg_len = CMSG_LEN(sizeof(int));
            std::memcpy(CMSG_DATA(header), &descriptor, sizeof(descriptor));
        }
        const ssize_t result = sendmsg(socket, &message, 0);
        if (result < 0 && errno == EINTR) continue;
        if (result <= 0) {
            const int error = result == 0 ? EPIPE : errno;
            if (sent != 0) shutdown(socket, SHUT_RDWR);
            return -error;
        }
        sent += static_cast<size_t>(result);
    }
    return 0;
}

int Receiver::fail(int error) {
    descriptor_.reset();
    used_ = 0;
    failed_ = true;
    return -error;
}

int Receiver::receive(int socket, void* bytes, size_t size,
                      android::base::unique_fd& descriptor) {
    if (failed_) return -EPROTO;
    if (!bytes || size == 0 || size != pending_.size()) return -EINVAL;
    while (used_ < size) {
        iovec io{pending_.data() + used_, size - used_};
        // Room for unexpected rights so every delivered descriptor is closed on
        // rejection. The kernel disposes of rights omitted by MSG_CTRUNC.
        alignas(cmsghdr) std::array<unsigned char, CMSG_SPACE(16 * sizeof(int))> control{};
        msghdr message{};
        message.msg_iov = &io;
        message.msg_iovlen = 1;
        message.msg_control = control.data();
        message.msg_controllen = control.size();
        const ssize_t result = recvmsg(socket, &message, MSG_DONTWAIT);
        if (result < 0) {
            if (errno == EINTR) continue;
            if (errno == EAGAIN || errno == EWOULDBLOCK) return -EAGAIN;
            return fail(errno);
        }
        bool invalid = (message.msg_flags & (MSG_TRUNC | MSG_CTRUNC)) != 0;
        for (cmsghdr* header = CMSG_FIRSTHDR(&message); header;
             header = CMSG_NXTHDR(&message, header)) {
            if (header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS ||
                header->cmsg_len < CMSG_LEN(0)) {
                invalid = true;
                continue;
            }
            const size_t payload = header->cmsg_len - CMSG_LEN(0);
            if (payload % sizeof(int) != 0) invalid = true;
            for (size_t offset = 0; offset + sizeof(int) <= payload; offset += sizeof(int)) {
                int fd;
                std::memcpy(&fd, CMSG_DATA(header) + offset, sizeof(fd));
                android::base::unique_fd incoming(fd);
                if (descriptor_.ok()) {
                    invalid = true;
                } else {
                    const int flags = fcntl(fd, F_GETFD);
                    if (flags == -1 || fcntl(fd, F_SETFD, flags | FD_CLOEXEC) == -1) invalid = true;
                    descriptor_ = std::move(incoming);
                }
            }
        }
        if (invalid) return fail(EPROTO);
        if (result == 0) return fail(used_ == 0 ? ECONNRESET : EPROTO);
        used_ += static_cast<size_t>(result);
    }
    std::memcpy(bytes, pending_.data(), size);
    descriptor = std::move(descriptor_);
    used_ = 0;
    return 0;
}

} // namespace darwin_art::release_record
