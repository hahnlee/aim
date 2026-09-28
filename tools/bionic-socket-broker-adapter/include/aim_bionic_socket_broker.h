#ifndef AIM_BIONIC_SOCKET_BROKER_H_
#define AIM_BIONIC_SOCKET_BROKER_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*AimBionicSocketBrokerFunction)(void);

typedef struct AimBionicPollFd {
  int32_t fd;
  int16_t events;
  int16_t revents;
} AimBionicPollFd;

/* Android arm64 mmsghdr ABI. The explicit padding keeps the 64-bit guest
 * stride (sizeof(mmsghdr)==64) independent of the host's socket headers. */
typedef struct AimAndroidIovec {
  void* base;
  uint64_t length;
} AimAndroidIovec;

typedef struct AimAndroidMsghdr {
  void* name;
  uint32_t name_length;
  uint32_t padding;
  AimAndroidIovec* vectors;
  uint64_t vector_count;
  void* control;
  uint64_t control_length;
  int32_t flags;
  uint32_t tail_padding;
} AimAndroidMsghdr;

typedef struct AimAndroidMmsghdr {
  AimAndroidMsghdr msg_hdr;
  uint32_t msg_len;
  uint32_t padding;
} AimAndroidMmsghdr;

int aim_bionic_socket_broker_activate(void);
int aim_bionic_socket_broker_deactivate(void);
/* Trusted host installation API, not an Android exported symbol. The runtime
 * owns and protects host_path; service permissions remain service-owned. */
int aim_bionic_socket_broker_install_unix_endpoint(
    const void* android_address, uint32_t length, const char* host_path);

int aim_bionic_socket_broker_socket(int domain, int type, int protocol);
int aim_bionic_socket_broker_pipe(int32_t descriptors[2]);
int aim_bionic_socket_broker_pipe2(int32_t descriptors[2], int flags);
int aim_bionic_socket_broker_eventfd(uint32_t initial_value, int flags);
int aim_bionic_socket_broker_timerfd_create(int clock_id, int flags);
int aim_bionic_socket_broker_epoll_create(int size);
int aim_bionic_socket_broker_epoll_create1(int flags);
int aim_bionic_socket_broker_epoll_ctl(int epoll_fd, int operation,
                                               int target_fd,
                                               const void* event);
int aim_bionic_socket_broker_epoll_wait(int epoll_fd, void* events,
                                                int capacity, int timeout_ms);
intptr_t aim_bionic_socket_broker_readv(int fd, const void *vectors,
                                               int count);
intptr_t aim_bionic_socket_broker_writev(int fd, const void *vectors,
                                                int count);
intptr_t aim_bionic_socket_broker_read(int fd, void *bytes,
                                              size_t count);
intptr_t aim_bionic_socket_broker_write(int fd, const void *bytes,
                                               size_t count);
int aim_bionic_socket_broker_poll(AimBionicPollFd *descriptors,
                                         size_t count, int timeout_ms);
int aim_bionic_socket_broker_connect(int fd, const void *address,
                                            uint32_t length);
int aim_bionic_socket_broker_bind(int fd, const void *address,
                                         uint32_t length);
int aim_bionic_socket_broker_listen(int fd, int backlog);
int aim_bionic_socket_broker_accept4(int fd, void *address,
                                            uint32_t *length, int flags);
int aim_bionic_socket_broker_accept(int fd, void *address,
                                           uint32_t *length);
int aim_bionic_socket_broker_getsockname(int fd, void *address,
                                                uint32_t *length);
int aim_bionic_socket_broker_getpeername(int fd, void *address,
                                                uint32_t *length);
int aim_bionic_socket_broker_socketpair(int domain, int type,
                                               int protocol,
                                               int32_t descriptors[2]);
intptr_t aim_bionic_socket_broker_send(int fd, const void *bytes,
                                              size_t count, int flags);
intptr_t aim_bionic_socket_broker_recv(int fd, void *bytes, size_t count,
                                              int flags);
intptr_t aim_bionic_socket_broker_sendto(int fd, const void *bytes,
                                                size_t count, int flags,
                                                const void *address,
                                                uint32_t address_length);
intptr_t aim_bionic_socket_broker_recvfrom(int fd, void *bytes,
                                                  size_t count, int flags,
                                                  void *address,
                                                  uint32_t *address_length);
int aim_bionic_socket_broker_sendmmsg(
    int fd, AimAndroidMmsghdr* messages, uint32_t count, int flags);
int aim_bionic_socket_broker_getsockopt(int fd, int level, int option,
                                               void *value, uint32_t *length);
int aim_bionic_socket_broker_setsockopt(int fd, int level, int option,
                                               const void *value,
                                               uint32_t length);
int aim_bionic_socket_broker_shutdown(int fd, int how);
int aim_bionic_socket_broker_dup(int fd);
int aim_bionic_socket_broker_dup2(int old_fd, int new_fd);
int aim_bionic_socket_broker_close(int fd);
int aim_bionic_socket_broker_fcntl(int fd, int command,
                                          intptr_t argument);
int aim_bionic_fd_export_for_scm(int guest_fd);
int aim_bionic_fd_import_from_scm(int host_fd);
int aim_bionic_socket_broker_res_nquery(uint64_t network,
                                               const char* name,
                                               int ns_class, int ns_type,
                                               uint32_t flags);
int aim_bionic_socket_broker_res_nresult(int fd, int* rcode,
                                                uint8_t* answer,
                                                size_t answer_length);
/* VM facade resolver: transfers one duplicate for any central-broker,
 * filesystem, or Android shared-memory descriptor. */
int aim_bionic_fd_dup_host_fd_core(int guest_fd, int* host_fd);
/* Returns zero after reporting whether a central-broker descriptor handled the
 * ioctl. This is the device-owner seam used by the Bionic ioctl facade. */
int aim_bionic_fd_broker_ioctl_dispatch(
    int fd, uint32_t request, void* argument, int* handled, int* result,
    int* android_errno);
/* Compatibility name for old runtime archives. New callers use the central-FD
 * name because device ownership is no longer socket-specific. */
int aim_bionic_socket_broker_ioctl_dispatch(
    int fd, uint32_t request, void* argument, int* handled, int* result,
    int* android_errno);

AimBionicSocketBrokerFunction
aim_bionic_socket_broker_resolve(const char *soname, const char *symbol,
                                        const char *version);
uintptr_t aim_bionic_socket_broker_data_resolve(
    const char *soname, const char *symbol, const char *version);
AimBionicSocketBrokerFunction aim_bionic_socket_broker_dns_resolve(
    const char *soname, const char *symbol, const char *version);
size_t aim_bionic_socket_broker_live_objects(void);
int aim_bionic_socket_broker_is_active(void);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AIM_BIONIC_SOCKET_BROKER_H_
