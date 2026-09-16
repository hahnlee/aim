#include "darwin_art_bionic_fd_broker.h"

_Static_assert(DARWIN_ART_FD_OWNER_ABI_V1 == 1 &&
                   DARWIN_ART_FD_OWNER_ABI_V2 == 2 &&
                   DARWIN_ART_FD_OWNER_ABI_V3 == 3 &&
                   DARWIN_ART_FD_OWNER_ABI_V4 == 4 &&
                   DARWIN_ART_FD_OWNER_ABI_V5 == 5 &&
                   DARWIN_ART_FD_OWNER_ABI_V6 == 6 &&
                   DARWIN_ART_FD_OWNER_ABI_V7 == 7,
               "owner ABI drift");
_Static_assert(DARWIN_ART_FD_FS_FILE == 1, "file kind drift");
_Static_assert(DARWIN_ART_FD_FS_RANDOM == 2, "random kind drift");
_Static_assert(DARWIN_ART_FD_STDIO == 3, "stdio kind drift");
_Static_assert(DARWIN_ART_FD_SOCKET == 4, "socket kind drift");
_Static_assert(DARWIN_ART_FD_EPOLL == 5, "epoll kind drift");
_Static_assert(DARWIN_ART_FD_PIPE == 6, "pipe kind drift");
_Static_assert(DARWIN_ART_FD_BINDER == 7, "binder kind drift");
_Static_assert(sizeof(((DarwinArtFdPollEntry *)0)->fd) == sizeof(int),
               "guest descriptor width drift");
_Static_assert(offsetof(DarwinArtFdOwnerV1, read_at) == 56,
               "owner v1 prefix drift");
_Static_assert(offsetof(DarwinArtFdOwnerV1, socket_operation) == 72,
               "owner v2 prefix drift");
_Static_assert(offsetof(DarwinArtFdOwnerV1, poll_many) == 80,
               "owner v3 prefix drift");
_Static_assert(offsetof(DarwinArtFdOwnerV1, set_status_flags) == 88,
               "owner v4 prefix drift");
_Static_assert(offsetof(DarwinArtFdOwnerV1, create_poll_wake) == 96,
               "owner v5 size drift");
_Static_assert(offsetof(DarwinArtFdOwnerV1, export_host_fd) == 128,
               "owner v6 size drift");
_Static_assert(sizeof(DarwinArtFdOwnerV1) == 136, "owner v7 size drift");
_Static_assert(sizeof(DarwinArtFdSocketRequestV1) == 136,
               "socket request size drift");
_Static_assert(offsetof(DarwinArtFdSocketRequestV1, address) == 16 &&
                   offsetof(DarwinArtFdSocketRequestV1, output_address) == 32 &&
                   offsetof(DarwinArtFdSocketRequestV1, input_bytes) == 56 &&
                   offsetof(DarwinArtFdSocketRequestV1, option_input) == 80 &&
                   offsetof(DarwinArtFdSocketRequestV1, level) == 120,
               "socket request layout drift");
_Static_assert(sizeof(DarwinArtFdSocketAcceptResultV1) == 32 &&
                   offsetof(DarwinArtFdSocketAcceptResultV1, object) == 8 &&
                   offsetof(DarwinArtFdSocketAcceptResultV1, kind) == 16 &&
                   offsetof(DarwinArtFdSocketAcceptResultV1,
                            descriptor_flags) == 24,
               "socket accept result layout drift");
_Static_assert(DARWIN_ART_FD_SOCKET_BIND == 1 &&
                   DARWIN_ART_FD_SOCKET_ACCEPT4 == 13,
               "socket operation drift");

static DarwinArtFdBrokerStatus (*const publish_signature)(
    DarwinArtFdBroker *, DarwinArtFdOwnerHandle, uint64_t,
    int *) = darwin_art_fd_broker_publish;
static DarwinArtFdBrokerStatus (*const sendfile_signature)(
    DarwinArtFdBroker *, int, int, size_t,
    DarwinArtFdIoResult *) = darwin_art_fd_broker_sendfile;
static DarwinArtFdBrokerStatus (*const duplicate_with_flags_signature)(
    DarwinArtFdBroker *, int, int,
    int *) = darwin_art_fd_broker_duplicate_with_flags;
static DarwinArtFdBrokerStatus (*const socket_operation_signature)(
    DarwinArtFdBroker *, DarwinArtFdOwnerHandle, int,
    const DarwinArtFdSocketRequestV1 *,
    DarwinArtFdIoResult *) = darwin_art_fd_broker_socket_operation;
static DarwinArtFdBrokerStatus (*const poll_wait_signature)(
    DarwinArtFdBroker *, DarwinArtFdPollEntry *, size_t, int,
    DarwinArtFdIoResult *) = darwin_art_fd_broker_poll_wait;

int main(void) {
  return publish_signature == 0 || sendfile_signature == 0 ||
         duplicate_with_flags_signature == 0 ||
         socket_operation_signature == 0 || poll_wait_signature == 0;
}
