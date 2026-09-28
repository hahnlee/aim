#include "aim_bionic_fd_broker.h"

_Static_assert(AIM_FD_OWNER_ABI_V1 == 1 &&
                   AIM_FD_OWNER_ABI_V2 == 2 &&
                   AIM_FD_OWNER_ABI_V3 == 3 &&
                   AIM_FD_OWNER_ABI_V4 == 4 &&
                   AIM_FD_OWNER_ABI_V5 == 5 &&
                   AIM_FD_OWNER_ABI_V6 == 6 &&
                   AIM_FD_OWNER_ABI_V7 == 7,
               "owner ABI drift");
_Static_assert(AIM_FD_DESCRIPTION_SNAPSHOT_ABI_V1 == 1 &&
                   sizeof(AimFdDescriptionSnapshotV1) == 32 &&
                   offsetof(AimFdDescriptionSnapshotV1, object) == 8 &&
                   offsetof(AimFdDescriptionSnapshotV1, owner) == 16 &&
                   offsetof(AimFdDescriptionSnapshotV1, kind) == 24 &&
                   offsetof(AimFdDescriptionSnapshotV1, status_flags) ==
                       28,
               "description snapshot ABI drift");
_Static_assert(AIM_FD_FS_FILE == 1, "file kind drift");
_Static_assert(AIM_FD_FS_RANDOM == 2, "random kind drift");
_Static_assert(AIM_FD_STDIO == 3, "stdio kind drift");
_Static_assert(AIM_FD_SOCKET == 4, "socket kind drift");
_Static_assert(AIM_FD_EPOLL == 5, "epoll kind drift");
_Static_assert(AIM_FD_PIPE == 6, "pipe kind drift");
_Static_assert(AIM_FD_BINDER == 7, "binder kind drift");
_Static_assert(sizeof(((AimFdPollEntry *)0)->fd) == sizeof(int),
               "guest descriptor width drift");
_Static_assert(offsetof(AimFdOwnerV1, read_at) == 56,
               "owner v1 prefix drift");
_Static_assert(offsetof(AimFdOwnerV1, socket_operation) == 72,
               "owner v2 prefix drift");
_Static_assert(offsetof(AimFdOwnerV1, poll_many) == 80,
               "owner v3 prefix drift");
_Static_assert(offsetof(AimFdOwnerV1, set_status_flags) == 88,
               "owner v4 prefix drift");
_Static_assert(offsetof(AimFdOwnerV1, create_poll_wake) == 96,
               "owner v5 size drift");
_Static_assert(offsetof(AimFdOwnerV1, export_host_fd) == 128,
               "owner v6 size drift");
_Static_assert(sizeof(AimFdOwnerV1) == 136, "owner v7 size drift");
_Static_assert(sizeof(AimFdSocketRequestV1) == 136,
               "socket request size drift");
_Static_assert(offsetof(AimFdSocketRequestV1, address) == 16 &&
                   offsetof(AimFdSocketRequestV1, output_address) == 32 &&
                   offsetof(AimFdSocketRequestV1, input_bytes) == 56 &&
                   offsetof(AimFdSocketRequestV1, option_input) == 80 &&
                   offsetof(AimFdSocketRequestV1, level) == 120,
               "socket request layout drift");
_Static_assert(sizeof(AimFdSocketAcceptResultV1) == 32 &&
                   offsetof(AimFdSocketAcceptResultV1, object) == 8 &&
                   offsetof(AimFdSocketAcceptResultV1, kind) == 16 &&
                   offsetof(AimFdSocketAcceptResultV1,
                            descriptor_flags) == 24,
               "socket accept result layout drift");
_Static_assert(AIM_FD_SOCKET_BIND == 1 &&
                   AIM_FD_SOCKET_ACCEPT4 == 13,
               "socket operation drift");

static AimFdBrokerStatus (*const publish_signature)(
    AimFdBroker *, AimFdOwnerHandle, uint64_t,
    int *) = aim_fd_broker_publish;
static AimFdBrokerStatus (*const sendfile_signature)(
    AimFdBroker *, int, int, size_t,
    AimFdIoResult *) = aim_fd_broker_sendfile;
static AimFdBrokerStatus (*const duplicate_with_flags_signature)(
    AimFdBroker *, int, int,
    int *) = aim_fd_broker_duplicate_with_flags;
static AimFdBrokerStatus (*const socket_operation_signature)(
    AimFdBroker *, AimFdOwnerHandle, int,
    const AimFdSocketRequestV1 *,
    AimFdIoResult *) = aim_fd_broker_socket_operation;
static AimFdBrokerStatus (*const poll_wait_signature)(
    AimFdBroker *, AimFdPollEntry *, size_t, int,
    AimFdIoResult *) = aim_fd_broker_poll_wait;
static AimFdBrokerStatus (*const retain_description_signature)(
    AimFdBroker *, int, AimFdOwnerHandle,
    AimFdDescriptionOperationV1, void *, AimFdDescriptionPin **,
    AimFdIoResult *) = aim_fd_broker_retain_description;
static AimFdBrokerStatus (*const release_description_signature)(
    AimFdBroker *,
    AimFdDescriptionPin *) = aim_fd_broker_release_description;

int main(void) {
  return publish_signature == 0 || sendfile_signature == 0 ||
         duplicate_with_flags_signature == 0 ||
         socket_operation_signature == 0 || poll_wait_signature == 0 ||
         retain_description_signature == 0 ||
         release_description_signature == 0;
}
