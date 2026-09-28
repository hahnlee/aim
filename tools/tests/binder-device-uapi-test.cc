#include <sys/ioctl.h>
#include <cstddef>
#include <cstdio>
#include <cstring>
#include <initializer_list>
constexpr auto hostWrite = _IOW('x', 1, int);
constexpr auto hostRead = _IOR('x', 1, int);
constexpr auto hostNone = _IO('x', 1);
#include "binder/device_uapi.h"

static_assert(_IOW('x', 1, int) == hostWrite);
static_assert(_IOR('x', 1, int) == hostRead);
static_assert(_IO('x', 1) == hostNone);
static_assert(sizeof(binder_uintptr_t) == 8);
static_assert(sizeof(binder_size_t) == 8);
static_assert(sizeof(binder_write_read) == 48);
static_assert(offsetof(binder_write_read, read_buffer) == 40);
static_assert(sizeof(binder_transaction_data) == 64);
static_assert(offsetof(binder_transaction_data, data_size) == 32);
static_assert(offsetof(binder_transaction_data, data) == 48);
static_assert(sizeof(flat_binder_object) == 24);
static_assert(BINDER_CURRENT_PROTOCOL_VERSION == 8);
static_assert(BINDER_WRITE_READ == 0xc0306201u);
static_assert(BINDER_VERSION == 0xc0046209u);
static_assert(BINDER_SET_MAX_THREADS == 0x40046205u);
static_assert(BC_TRANSACTION == 0x40406300u);
static_assert(BC_FREE_BUFFER == 0x40086303u);
static_assert(BR_TRANSACTION == 0x80407202u);
static_assert(BR_NOOP == 0x720cu);
int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--transactions") == 0) {
        for (unsigned command : {BC_TRANSACTION, BC_REPLY, BC_TRANSACTION_SG, BC_REPLY_SG}) {
            binder_transaction_data_sg payload;
            std::memset(&payload, 0xff, sizeof(payload));
            payload.transaction_data.target.handle = 0x87654321u;
            payload.transaction_data.cookie = 0x8877665544332211ULL;
            payload.transaction_data.code = 0x12345678;
            payload.transaction_data.flags = TF_ONE_WAY | TF_ACCEPT_FDS;
            payload.transaction_data.sender_pid = 123;
            payload.transaction_data.sender_euid = 456;
            payload.transaction_data.data_size = 0x123456789ULL;
            payload.transaction_data.offsets_size = 0x234567890ULL;
            payload.transaction_data.data.ptr.buffer = 0xabcdef0123456789ULL;
            payload.transaction_data.data.ptr.offsets = 0xfedcba9876543210ULL;
            payload.buffers_size = 0x345678908ULL;
            const bool sg = command == BC_TRANSACTION_SG || command == BC_REPLY_SG;
            const auto size = sg ? sizeof(payload) : sizeof(payload.transaction_data);
            if (std::fwrite(&command, sizeof(command), 1, stdout) != 1 ||
                std::fwrite(&payload, size, 1, stdout) != 1) return 1;
        }
        return 0;
    }
    if (argc == 2 && std::strcmp(argv[1], "--node-work") == 0) {
        binder_ptr_cookie payload{};
        payload.ptr = 0x1122334455667788ULL;
        payload.cookie = 0x8877665544332211ULL;
        for (unsigned command : {BR_INCREFS, BR_ACQUIRE, BR_RELEASE, BR_DECREFS}) {
            if (std::fwrite(&command, sizeof(command), 1, stdout) != 1 ||
                std::fwrite(&payload, sizeof(payload), 1, stdout) != 1) return 1;
        }
        return 0;
    }
    if (argc == 2 && std::strcmp(argv[1], "--object-fields") == 0) {
        for (unsigned tag : {BINDER_TYPE_BINDER, BINDER_TYPE_WEAK_BINDER,
                             BINDER_TYPE_HANDLE, BINDER_TYPE_WEAK_HANDLE}) {
            flat_binder_object object;
            std::memset(&object, 0xff, sizeof(object));
            object.hdr.type = tag;
            object.flags = 0x12345678;
            object.cookie = 0x8877665544332211ULL;
            if (tag == BINDER_TYPE_HANDLE || tag == BINDER_TYPE_WEAK_HANDLE)
                object.handle = 42;
            else object.binder = 0xabcdef0123456789ULL;
            if (std::fwrite(&object, sizeof(object), 1, stdout) != 1) return 1;
        }
        binder_fd_object fd;
        std::memset(&fd, 0xff, sizeof(fd));
        fd.hdr.type = BINDER_TYPE_FD;
        fd.fd = 42;
        fd.cookie = 0x8877665544332211ULL;
        if (std::fwrite(&fd, sizeof(fd), 1, stdout) != 1) return 1;
        binder_buffer_object buffer{};
        buffer.hdr.type = BINDER_TYPE_PTR;
        buffer.flags = 0x12345678;
        buffer.buffer = 0xabcdef0123456789ULL;
        buffer.length = 0x8877665544332211ULL;
        buffer.parent = 13;
        buffer.parent_offset = 29;
        if (std::fwrite(&buffer, sizeof(buffer), 1, stdout) != 1) return 1;
        binder_fd_array_object array{};
        array.hdr.type = BINDER_TYPE_FDA;
        array.num_fds = 0x8877665544332211ULL;
        array.parent = 13;
        array.parent_offset = 29;
        return std::fwrite(&array, sizeof(array), 1, stdout) == 1 ? 0 : 1;
    }
    if (argc == 2 && std::strcmp(argv[1], "--objects") == 0) {
        struct Entry { unsigned tag; size_t size; };
        const Entry entries[] = {
            {BINDER_TYPE_BINDER, sizeof(flat_binder_object)},
            {BINDER_TYPE_WEAK_BINDER, sizeof(flat_binder_object)},
            {BINDER_TYPE_HANDLE, sizeof(flat_binder_object)},
            {BINDER_TYPE_WEAK_HANDLE, sizeof(flat_binder_object)},
            {BINDER_TYPE_FD, sizeof(binder_fd_object)},
            {BINDER_TYPE_PTR, sizeof(binder_buffer_object)},
            {BINDER_TYPE_FDA, sizeof(binder_fd_array_object)},
        };
        for (const auto& entry : entries) std::printf("%08x %zu\n", entry.tag, entry.size);
        return 0;
    }
    if (argc == 2 && std::strcmp(argv[1], "--commands") == 0) {
        const unsigned commands[] = {
            BC_TRANSACTION, BC_REPLY, BC_ACQUIRE_RESULT, BC_FREE_BUFFER,
            BC_INCREFS, BC_ACQUIRE, BC_RELEASE, BC_DECREFS, BC_INCREFS_DONE,
            BC_ACQUIRE_DONE, BC_ATTEMPT_ACQUIRE, BC_REGISTER_LOOPER,
            BC_ENTER_LOOPER, BC_EXIT_LOOPER, BC_REQUEST_DEATH_NOTIFICATION,
            BC_CLEAR_DEATH_NOTIFICATION, BC_DEAD_BINDER_DONE, BC_TRANSACTION_SG,
            BC_REPLY_SG, BC_REQUEST_FREEZE_NOTIFICATION, BC_CLEAR_FREEZE_NOTIFICATION,
            BC_FREEZE_NOTIFICATION_DONE,
        };
        for (unsigned command : commands) std::printf("%08x\n", command);
        return 0;
    }
    puts("Binder UAPI: 64-bit layout, Linux command encoding, Darwin ioctl isolation PASS");
}
