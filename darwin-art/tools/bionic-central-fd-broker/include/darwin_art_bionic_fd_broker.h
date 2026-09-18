#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DarwinArtFdBroker DarwinArtFdBroker;
typedef uint64_t DarwinArtFdOwnerHandle;
typedef struct DarwinArtFdDescriptionPin DarwinArtFdDescriptionPin;

typedef enum DarwinArtFdKind {
  DARWIN_ART_FD_FS_FILE = 1,
  DARWIN_ART_FD_FS_RANDOM = 2,
  DARWIN_ART_FD_STDIO = 3,
  DARWIN_ART_FD_SOCKET = 4,
  DARWIN_ART_FD_EPOLL = 5,
  DARWIN_ART_FD_PIPE = 6,
  DARWIN_ART_FD_BINDER = 7,
} DarwinArtFdKind;

typedef enum DarwinArtFdBrokerStatus {
  DARWIN_ART_FD_BROKER_OK = 0,
  DARWIN_ART_FD_BROKER_INVALID_ARGUMENT = 1,
  DARWIN_ART_FD_BROKER_STALE = 2,
  DARWIN_ART_FD_BROKER_WRONG_OWNER = 3,
  DARWIN_ART_FD_BROKER_WRONG_KIND = 4,
  DARWIN_ART_FD_BROKER_BUSY = 5,
  DARWIN_ART_FD_BROKER_DRAINING = 6,
  DARWIN_ART_FD_BROKER_EXHAUSTED = 7,
  DARWIN_ART_FD_BROKER_ALREADY_EXISTS = 8,
  DARWIN_ART_FD_BROKER_UNSUPPORTED = 9,
} DarwinArtFdBrokerStatus;

typedef struct DarwinArtFdIoResult {
  intptr_t value;
  int android_errno;
} DarwinArtFdIoResult;

typedef struct DarwinArtFdDescriptionSnapshotV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  uint64_t object;
  DarwinArtFdOwnerHandle owner;
  uint32_t kind;
  int32_t status_flags;
} DarwinArtFdDescriptionSnapshotV1;

typedef intptr_t (*DarwinArtFdDescriptionOperationV1)(
    void *context, const DarwinArtFdDescriptionSnapshotV1 *snapshot,
    int *android_errno);
// Retained-export operation: the broker resolves and retains one exact
// Description, invokes its owner export callback on that Description object,
// then supplies the duplicated host descriptor together with the same
// immutable snapshot. The caller owns the host descriptor on success and the
// returned pin keeps the Description alive until explicit release.
// The operation borrows the FD until a nonnegative return and must not close
// it on failure. Required provider cleanup runs once on export/operation
// failure, outside the broker mutex and before the Description pin is released.
typedef intptr_t (*DarwinArtFdRetainedExportOperationV1)(
    void *context, const DarwinArtFdDescriptionSnapshotV1 *snapshot,
    int host_fd, int *android_errno);
typedef void (*DarwinArtFdRetainedExportReleaseV1)(void *context, int host_fd);

typedef struct DarwinArtFdPollEntry {
  int fd;
  int16_t events;
  int16_t revents;
} DarwinArtFdPollEntry;

typedef struct DarwinArtFdEpollEvent {
  uint32_t events;
  uint64_t data;
} DarwinArtFdEpollEvent;

typedef enum DarwinArtFdSocketOperation {
  DARWIN_ART_FD_SOCKET_BIND = 1,
  DARWIN_ART_FD_SOCKET_CONNECT = 2,
  DARWIN_ART_FD_SOCKET_LISTEN = 3,
  DARWIN_ART_FD_SOCKET_SHUTDOWN = 4,
  DARWIN_ART_FD_SOCKET_SEND = 5,
  DARWIN_ART_FD_SOCKET_RECV = 6,
  DARWIN_ART_FD_SOCKET_SENDTO = 7,
  DARWIN_ART_FD_SOCKET_RECVFROM = 8,
  DARWIN_ART_FD_SOCKET_GETSOCKOPT = 9,
  DARWIN_ART_FD_SOCKET_SETSOCKOPT = 10,
  DARWIN_ART_FD_SOCKET_GETPEERNAME = 11,
  DARWIN_ART_FD_SOCKET_GETSOCKNAME = 12,
  DARWIN_ART_FD_SOCKET_ACCEPT4 = 13,
} DarwinArtFdSocketOperation;

typedef struct DarwinArtFdSocketRequestV1 {
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
} DarwinArtFdSocketRequestV1;

typedef struct DarwinArtFdSocketAcceptResultV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  uint64_t object;
  uint32_t kind;
  int32_t status_flags;
  int32_t descriptor_flags;
} DarwinArtFdSocketAcceptResultV1;

typedef struct DarwinArtFdOwnerV1 {
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
                               const DarwinArtFdSocketRequestV1 *request,
                               DarwinArtFdSocketAcceptResultV1 *accepted,
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
} DarwinArtFdOwnerV1;

enum {
  DARWIN_ART_FD_OWNER_ABI_V1 = 1,
  DARWIN_ART_FD_OWNER_ABI_V2 = 2,
  DARWIN_ART_FD_OWNER_ABI_V3 = 3,
  DARWIN_ART_FD_OWNER_ABI_V4 = 4,
  DARWIN_ART_FD_OWNER_ABI_V5 = 5,
  DARWIN_ART_FD_OWNER_ABI_V6 = 6,
  DARWIN_ART_FD_OWNER_ABI_V7 = 7,
  DARWIN_ART_FD_DESCRIPTION_SNAPSHOT_ABI_V1 = 1,
  DARWIN_ART_FD_SOCKET_REQUEST_ABI_V1 = 1,
  DARWIN_ART_FD_SOCKET_ACCEPT_RESULT_ABI_V1 = 1,
  DARWIN_ART_FD_CLOEXEC = 1,
  DARWIN_ART_FD_STATUS_APPEND = 0x400,
  DARWIN_ART_FD_STATUS_NONBLOCK = 0x800,
  DARWIN_ART_EPOLL_CLOEXEC = 0x80000,
  DARWIN_ART_EPOLL_CTL_ADD = 1,
  DARWIN_ART_EPOLL_CTL_DEL = 2,
  DARWIN_ART_EPOLL_CTL_MOD = 3,
};

DarwinArtFdBroker *darwin_art_fd_broker_create(void);
DarwinArtFdBrokerStatus darwin_art_fd_broker_destroy(DarwinArtFdBroker *broker);

DarwinArtFdBrokerStatus darwin_art_fd_broker_install_owner(
    DarwinArtFdBroker *broker, DarwinArtFdKind kind,
    const DarwinArtFdOwnerV1 *callbacks, DarwinArtFdOwnerHandle *owner);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_uninstall_owner(DarwinArtFdBroker *broker,
                                     DarwinArtFdOwnerHandle owner);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_wait_owner_quiescent(DarwinArtFdBroker *broker,
                                          DarwinArtFdOwnerHandle owner);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_flush_deferred_closes(DarwinArtFdBroker *broker);

DarwinArtFdBrokerStatus
darwin_art_fd_broker_publish(DarwinArtFdBroker *broker,
                             DarwinArtFdOwnerHandle owner, uint64_t object,
                             int *guest_fd);
DarwinArtFdBrokerStatus darwin_art_fd_broker_publish_with_flags(
    DarwinArtFdBroker *broker, DarwinArtFdOwnerHandle owner, uint64_t object,
    int status_flags, int descriptor_flags, int *guest_fd);
DarwinArtFdBrokerStatus darwin_art_fd_broker_dup(DarwinArtFdBroker *broker,
                                                 int old_fd, int *new_fd);
DarwinArtFdBrokerStatus darwin_art_fd_broker_dup2(DarwinArtFdBroker *broker,
                                                  int old_fd, int new_fd,
                                                  DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_duplicate_with_flags(DarwinArtFdBroker *broker, int old_fd,
                                          int flags, int *new_fd);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_fcntl_dupfd_cloexec(DarwinArtFdBroker *broker, int old_fd,
                                         int minimum_fd, int *new_fd);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_descriptor_flags(DarwinArtFdBroker *broker,
                                          int guest_fd, int *flags);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_set_descriptor_flags(DarwinArtFdBroker *broker,
                                          int guest_fd, int flags);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_status_flags(DarwinArtFdBroker *broker, int guest_fd,
                                      int *flags);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_set_status_flags(DarwinArtFdBroker *broker, int guest_fd,
                                      int flags);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_set_status_flags_io(DarwinArtFdBroker *broker,
                                         int guest_fd, int flags,
                                         DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_get_offset(DarwinArtFdBroker *broker, int guest_fd,
                                int64_t *offset);
DarwinArtFdBrokerStatus darwin_art_fd_broker_get_kind(DarwinArtFdBroker *broker,
                                                      int guest_fd,
                                                      DarwinArtFdKind *kind);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_export_host_fd(DarwinArtFdBroker *broker, int guest_fd,
                                    int *host_fd, DarwinArtFdIoResult *result);
// Acquires the exact open-file description once, invokes |operation| with a
// copied snapshot without the broker mutex, and returns an opaque pin only
// when the operation succeeds. The pin keeps the description active across
// logical close/dup2 until release; expected_owner must match exactly.
DarwinArtFdBrokerStatus darwin_art_fd_broker_retain_description(
    DarwinArtFdBroker *broker, int guest_fd,
    DarwinArtFdOwnerHandle expected_owner,
    DarwinArtFdDescriptionOperationV1 operation, void *operation_context,
    DarwinArtFdDescriptionPin **pin, DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_release_description(DarwinArtFdBroker *broker,
                                         DarwinArtFdDescriptionPin *pin);
DarwinArtFdBrokerStatus darwin_art_fd_broker_retain_exported_description(
    DarwinArtFdBroker *broker, int guest_fd,
    DarwinArtFdRetainedExportOperationV1 operation,
    DarwinArtFdRetainedExportReleaseV1 release_export, void *operation_context,
    DarwinArtFdDescriptionPin **pin, DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus darwin_art_fd_broker_close(DarwinArtFdBroker *broker,
                                                   int guest_fd,
                                                   DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_close_owned(DarwinArtFdBroker *broker,
                                 DarwinArtFdOwnerHandle owner, int guest_fd,
                                 DarwinArtFdIoResult *result);

DarwinArtFdBrokerStatus darwin_art_fd_broker_read(DarwinArtFdBroker *broker,
                                                  int guest_fd, void *bytes,
                                                  size_t count,
                                                  DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus darwin_art_fd_broker_write(DarwinArtFdBroker *broker,
                                                   int guest_fd,
                                                   const void *bytes,
                                                   size_t count,
                                                   DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_ioctl(DarwinArtFdBroker *broker, int guest_fd,
                           DarwinArtFdKind required_kind, uint64_t request,
                           void *argument, DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus darwin_art_fd_broker_poll(DarwinArtFdBroker *broker,
                                                  DarwinArtFdPollEntry *entries,
                                                  size_t count,
                                                  DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_poll_wait(DarwinArtFdBroker *broker,
                               DarwinArtFdPollEntry *entries, size_t count,
                               int timeout_ms, DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_sendfile(DarwinArtFdBroker *broker, int output_fd,
                              int input_fd, size_t count,
                              DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus darwin_art_fd_broker_socket_operation(
    DarwinArtFdBroker *broker, DarwinArtFdOwnerHandle owner, int guest_fd,
    const DarwinArtFdSocketRequestV1 *request, DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_epoll_create1(DarwinArtFdBroker *broker, int flags,
                                   int *epoll_fd);
DarwinArtFdBrokerStatus darwin_art_fd_broker_epoll_ctl(
    DarwinArtFdBroker *broker, int epoll_fd, int operation, int target_fd,
    const DarwinArtFdEpollEvent *event, DarwinArtFdIoResult *result);
DarwinArtFdBrokerStatus
darwin_art_fd_broker_epoll_wait(DarwinArtFdBroker *broker, int epoll_fd,
                                DarwinArtFdEpollEvent *events, size_t capacity,
                                int timeout_ms, DarwinArtFdIoResult *result);

#ifdef __cplusplus
}
#endif
