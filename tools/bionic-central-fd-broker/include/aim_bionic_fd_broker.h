#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimFdBroker AimFdBroker;
typedef uint64_t AimFdOwnerHandle;
typedef struct AimFdDescriptionPin AimFdDescriptionPin;

typedef struct AimFdPublishBatchEntryV1 {
  AimFdOwnerHandle owner;
  uint64_t object;
  int32_t status_flags;
  int32_t descriptor_flags;
} AimFdPublishBatchEntryV1;

typedef enum AimFdKind {
  AIM_FD_FS_FILE = 1,
  AIM_FD_FS_RANDOM = 2,
  AIM_FD_STDIO = 3,
  AIM_FD_SOCKET = 4,
  AIM_FD_EPOLL = 5,
  AIM_FD_PIPE = 6,
  AIM_FD_BINDER = 7,
} AimFdKind;

typedef enum AimFdBrokerStatus {
  AIM_FD_BROKER_OK = 0,
  AIM_FD_BROKER_INVALID_ARGUMENT = 1,
  AIM_FD_BROKER_STALE = 2,
  AIM_FD_BROKER_WRONG_OWNER = 3,
  AIM_FD_BROKER_WRONG_KIND = 4,
  AIM_FD_BROKER_BUSY = 5,
  AIM_FD_BROKER_DRAINING = 6,
  AIM_FD_BROKER_EXHAUSTED = 7,
  AIM_FD_BROKER_ALREADY_EXISTS = 8,
  AIM_FD_BROKER_UNSUPPORTED = 9,
} AimFdBrokerStatus;

typedef struct AimFdIoResult {
  intptr_t value;
  int android_errno;
} AimFdIoResult;

typedef struct AimFdDescriptionSnapshotV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  uint64_t object;
  AimFdOwnerHandle owner;
  uint32_t kind;
  int32_t status_flags;
} AimFdDescriptionSnapshotV1;

typedef intptr_t (*AimFdDescriptionOperationV1)(
    void *context, const AimFdDescriptionSnapshotV1 *snapshot,
    int *android_errno);
// Retained-export operation: the broker resolves and retains one exact
// Description, invokes its owner export callback on that Description object,
// then supplies the duplicated host descriptor together with the same
// immutable snapshot. The caller owns the host descriptor on success and the
// returned pin keeps the Description alive until explicit release.
// The operation borrows the FD until a nonnegative return and must not close
// it on failure. Required provider cleanup runs once on export/operation
// failure, outside the broker mutex and before the Description pin is released.
typedef intptr_t (*AimFdRetainedExportOperationV1)(
    void *context, const AimFdDescriptionSnapshotV1 *snapshot,
    int host_fd, int *android_errno);
typedef void (*AimFdRetainedExportReleaseV1)(void *context, int host_fd);

typedef struct AimFdPollEntry {
  int fd;
  int16_t events;
  int16_t revents;
} AimFdPollEntry;

typedef struct AimFdEpollEvent {
  uint32_t events;
  uint64_t data;
} AimFdEpollEvent;

typedef enum AimFdSocketOperation {
  AIM_FD_SOCKET_BIND = 1,
  AIM_FD_SOCKET_CONNECT = 2,
  AIM_FD_SOCKET_LISTEN = 3,
  AIM_FD_SOCKET_SHUTDOWN = 4,
  AIM_FD_SOCKET_SEND = 5,
  AIM_FD_SOCKET_RECV = 6,
  AIM_FD_SOCKET_SENDTO = 7,
  AIM_FD_SOCKET_RECVFROM = 8,
  AIM_FD_SOCKET_GETSOCKOPT = 9,
  AIM_FD_SOCKET_SETSOCKOPT = 10,
  AIM_FD_SOCKET_GETPEERNAME = 11,
  AIM_FD_SOCKET_GETSOCKNAME = 12,
  AIM_FD_SOCKET_ACCEPT4 = 13,
} AimFdSocketOperation;

typedef struct AimFdSocketRequestV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  uint32_t operation;
  int32_t flags;
  const void *address;
  uint32_t address_length;
  void *output_address;
  uint32_t output_address_capacity;
  uint32_t *output_address_length;
  const void *input_bytes;
  void *output_bytes;
  size_t byte_count;
  const void *option_input;
  uint32_t option_input_length;
  void *option_output;
  uint32_t option_output_capacity;
  uint32_t *option_output_length;
  int32_t level;
  int32_t option;
  int32_t argument;
} AimFdSocketRequestV1;

typedef struct AimFdSocketAcceptResultV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  uint64_t object;
  uint32_t kind;
  int32_t status_flags;
  int32_t descriptor_flags;
} AimFdSocketAcceptResultV1;

typedef struct AimFdOwnerV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void *context;
  intptr_t (*read)(void *context, uint64_t object, void *bytes, size_t count,
                   int *android_errno);
  intptr_t (*write)(void *context, uint64_t object, const void *bytes,
                    size_t count, int *android_errno);
  int (*poll)(void *context, uint64_t object, int16_t events, int16_t *revents,
              int *android_errno);
  int (*ioctl)(void *context, uint64_t object, uint64_t request, void *argument,
               int *android_errno);
  int (*close)(void *context, uint64_t object, int *android_errno);
  intptr_t (*read_at)(void *context, uint64_t object, int64_t *offset,
                      void *bytes, size_t count, int *android_errno);
  intptr_t (*write_at)(void *context, uint64_t object, int64_t *offset,
                       const void *bytes, size_t count, int *android_errno);
  intptr_t (*socket_operation)(void *context, uint64_t object,
                               const AimFdSocketRequestV1 *request,
                               AimFdSocketAcceptResultV1 *accepted,
                               int *android_errno);
  int (*poll_many)(void *context, const uint64_t *objects,
                   const int16_t *events, int16_t *revents, size_t count,
                   int timeout_ms, int *android_errno);
  int (*set_status_flags)(void *context, uint64_t object, int flags,
                          int *android_errno);
  uint64_t (*create_poll_wake)(void *context, int *android_errno);
  int (*signal_poll_wake)(void *context, uint64_t wake, int *android_errno);
  int (*close_poll_wake)(void *context, uint64_t wake, int *android_errno);
  int (*poll_many_with_wake)(void *context, const uint64_t *objects,
                             const int16_t *events, int16_t *revents,
                             size_t count, int timeout_ms, uint64_t wake,
                             int *woke, int *android_errno);
  int (*export_host_fd)(void *context, uint64_t object, int *host_fd,
                        int *android_errno);
} AimFdOwnerV1;

enum {
  AIM_FD_OWNER_ABI_V1 = 1,
  AIM_FD_OWNER_ABI_V2 = 2,
  AIM_FD_OWNER_ABI_V3 = 3,
  AIM_FD_OWNER_ABI_V4 = 4,
  AIM_FD_OWNER_ABI_V5 = 5,
  AIM_FD_OWNER_ABI_V6 = 6,
  AIM_FD_OWNER_ABI_V7 = 7,
  AIM_FD_DESCRIPTION_SNAPSHOT_ABI_V1 = 1,
  AIM_FD_PUBLISH_BATCH_ABI_V1 = 1,
  AIM_FD_BROKER_MAX_PUBLISH_BATCH = 16,
  AIM_FD_SOCKET_REQUEST_ABI_V1 = 1,
  AIM_FD_SOCKET_ACCEPT_RESULT_ABI_V1 = 1,
  AIM_FD_CLOEXEC = 1,
  AIM_FD_STATUS_APPEND = 0x400,
  AIM_FD_STATUS_NONBLOCK = 0x800,
  AIM_EPOLL_CLOEXEC = 0x80000,
  AIM_EPOLL_CTL_ADD = 1,
  AIM_EPOLL_CTL_DEL = 2,
  AIM_EPOLL_CTL_MOD = 3,
};

AimFdBroker *aim_fd_broker_create(void);
AimFdBrokerStatus aim_fd_broker_destroy(AimFdBroker *broker);

AimFdBrokerStatus aim_fd_broker_install_owner(
    AimFdBroker *broker, AimFdKind kind,
    const AimFdOwnerV1 *callbacks, AimFdOwnerHandle *owner);
AimFdBrokerStatus
aim_fd_broker_uninstall_owner(AimFdBroker *broker,
                                     AimFdOwnerHandle owner);
AimFdBrokerStatus
aim_fd_broker_wait_owner_quiescent(AimFdBroker *broker,
                                          AimFdOwnerHandle owner);
AimFdBrokerStatus
aim_fd_broker_flush_deferred_closes(AimFdBroker *broker);

AimFdBrokerStatus
aim_fd_broker_publish(AimFdBroker *broker,
                             AimFdOwnerHandle owner, uint64_t object,
                             int *guest_fd);
AimFdBrokerStatus aim_fd_broker_publish_with_flags(
    AimFdBroker *broker, AimFdOwnerHandle owner, uint64_t object,
    int status_flags, int descriptor_flags, int *guest_fd);
// Publishes all entries as one namespace transaction. On failure no entry is
// installed and |guest_fds| is left untouched; opaque objects remain owned by
// their callers. The bounded count keeps the reservation transaction local.
AimFdBrokerStatus aim_fd_broker_publish_batch_with_flags(
    AimFdBroker *broker, const AimFdPublishBatchEntryV1 *entries,
    size_t count, int *guest_fds);
// Convenience form for the common socketpair case. It has the same
// all-or-none and caller-owned-object failure contract as the batch API.
AimFdBrokerStatus aim_fd_broker_publish_pair_with_flags(
    AimFdBroker *broker, AimFdOwnerHandle owner,
    const uint64_t objects[2], const int status_flags[2],
    const int descriptor_flags[2], int guest_fds[2]);
AimFdBrokerStatus aim_fd_broker_dup(AimFdBroker *broker,
                                                 int old_fd, int *new_fd);
AimFdBrokerStatus aim_fd_broker_dup2(AimFdBroker *broker,
                                                  int old_fd, int new_fd,
                                                  AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_duplicate_with_flags(AimFdBroker *broker, int old_fd,
                                          int flags, int *new_fd);
AimFdBrokerStatus
aim_fd_broker_fcntl_dupfd_cloexec(AimFdBroker *broker, int old_fd,
                                         int minimum_fd, int *new_fd);
AimFdBrokerStatus
aim_fd_broker_get_descriptor_flags(AimFdBroker *broker,
                                          int guest_fd, int *flags);
AimFdBrokerStatus
aim_fd_broker_set_descriptor_flags(AimFdBroker *broker,
                                          int guest_fd, int flags);
AimFdBrokerStatus
aim_fd_broker_get_status_flags(AimFdBroker *broker, int guest_fd,
                                      int *flags);
AimFdBrokerStatus
aim_fd_broker_set_status_flags(AimFdBroker *broker, int guest_fd,
                                      int flags);
AimFdBrokerStatus
aim_fd_broker_set_status_flags_io(AimFdBroker *broker,
                                         int guest_fd, int flags,
                                         AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_get_offset(AimFdBroker *broker, int guest_fd,
                                int64_t *offset);
AimFdBrokerStatus aim_fd_broker_get_kind(AimFdBroker *broker,
                                                      int guest_fd,
                                                      AimFdKind *kind);
AimFdBrokerStatus
aim_fd_broker_export_host_fd(AimFdBroker *broker, int guest_fd,
                                    int *host_fd, AimFdIoResult *result);
// Acquires the exact open-file description once, invokes |operation| with a
// copied snapshot without the broker mutex, and returns an opaque pin only
// when the operation succeeds. The pin keeps the description active across
// logical close/dup2 until release; expected_owner must match exactly.
AimFdBrokerStatus aim_fd_broker_retain_description(
    AimFdBroker *broker, int guest_fd,
    AimFdOwnerHandle expected_owner,
    AimFdDescriptionOperationV1 operation, void *operation_context,
    AimFdDescriptionPin **pin, AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_release_description(AimFdBroker *broker,
                                         AimFdDescriptionPin *pin);
AimFdBrokerStatus aim_fd_broker_retain_exported_description(
    AimFdBroker *broker, int guest_fd,
    AimFdRetainedExportOperationV1 operation,
    AimFdRetainedExportReleaseV1 release_export, void *operation_context,
    AimFdDescriptionPin **pin, AimFdIoResult *result);
AimFdBrokerStatus aim_fd_broker_close(AimFdBroker *broker,
                                                   int guest_fd,
                                                   AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_close_owned(AimFdBroker *broker,
                                 AimFdOwnerHandle owner, int guest_fd,
                                 AimFdIoResult *result);

AimFdBrokerStatus aim_fd_broker_read(AimFdBroker *broker,
                                                  int guest_fd, void *bytes,
                                                  size_t count,
                                                  AimFdIoResult *result);
AimFdBrokerStatus aim_fd_broker_write(AimFdBroker *broker,
                                                   int guest_fd,
                                                   const void *bytes,
                                                   size_t count,
                                                   AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_ioctl(AimFdBroker *broker, int guest_fd,
                           AimFdKind required_kind, uint64_t request,
                           void *argument, AimFdIoResult *result);
AimFdBrokerStatus aim_fd_broker_poll(AimFdBroker *broker,
                                                  AimFdPollEntry *entries,
                                                  size_t count,
                                                  AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_poll_wait(AimFdBroker *broker,
                               AimFdPollEntry *entries, size_t count,
                               int timeout_ms, AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_sendfile(AimFdBroker *broker, int output_fd,
                              int input_fd, size_t count,
                              AimFdIoResult *result);
AimFdBrokerStatus aim_fd_broker_socket_operation(
    AimFdBroker *broker, AimFdOwnerHandle owner, int guest_fd,
    const AimFdSocketRequestV1 *request, AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_epoll_create1(AimFdBroker *broker, int flags,
                                   int *epoll_fd);
AimFdBrokerStatus aim_fd_broker_epoll_ctl(
    AimFdBroker *broker, int epoll_fd, int operation, int target_fd,
    const AimFdEpollEvent *event, AimFdIoResult *result);
AimFdBrokerStatus
aim_fd_broker_epoll_wait(AimFdBroker *broker, int epoll_fd,
                                AimFdEpollEvent *events, size_t capacity,
                                int timeout_ms, AimFdIoResult *result);

#ifdef __cplusplus
}
#endif
