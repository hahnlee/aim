#ifndef AIM_BIONIC_SOCKET_H_
#define AIM_BIONIC_SOCKET_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t AimAndroidSocklen;
typedef void (*AimBionicSocketFunction)(void);

typedef struct AimAndroidPollfd {
  int32_t fd;
  int16_t events;
  int16_t revents;
} AimAndroidPollfd;

typedef struct AimAndroidTimespec {
  int64_t tv_sec;
  int64_t tv_nsec;
} AimAndroidTimespec;

typedef struct AimAndroidIovec {
  void* iov_base;
  uint64_t iov_len;
} AimAndroidIovec;

typedef struct AimAndroidMsghdr {
  void* msg_name;
  uint32_t msg_namelen;
  uint32_t reserved_name_padding;
  AimAndroidIovec* msg_iov;
  uint64_t msg_iovlen;
  void* msg_control;
  uint64_t msg_controllen;
  int32_t msg_flags;
  uint32_t reserved_flags_padding;
} AimAndroidMsghdr;

int aim_bionic_socket_socket(int domain, int type, int protocol);
int aim_bionic_socket_socketpair(int domain, int type, int protocol,
                                        int32_t sockets[2]);
int aim_bionic_socket_close(int fd);
int aim_bionic_socket_bind(int fd, const void* address,
                                  AimAndroidSocklen length);
int aim_bionic_socket_connect(int fd, const void* address,
                                     AimAndroidSocklen length);
int aim_bionic_socket_listen(int fd, int backlog);
int aim_bionic_socket_accept4(int fd, void* address,
                                     AimAndroidSocklen* length,
                                     int flags);
int aim_bionic_socket_shutdown(int fd, int how);
int aim_bionic_socket_getsockname(int fd, void* address,
                                         AimAndroidSocklen* length);
int aim_bionic_socket_getpeername(int fd, void* address,
                                         AimAndroidSocklen* length);
int aim_bionic_socket_getsockopt(int fd, int level, int option,
                                        void* value,
                                        AimAndroidSocklen* length);
int aim_bionic_socket_setsockopt(int fd, int level, int option,
                                        const void* value,
                                        AimAndroidSocklen length);
intptr_t aim_bionic_socket_send(int fd, const void* buffer,
                                       size_t length, int flags);
intptr_t aim_bionic_socket_recv(int fd, void* buffer, size_t length,
                                       int flags);
intptr_t aim_bionic_socket_sendto(int fd, const void* buffer,
                                         size_t length, int flags,
                                         const void* address,
                                         AimAndroidSocklen address_length);
intptr_t aim_bionic_socket_recvfrom(
    int fd, void* buffer, size_t length, int flags, void* address,
    AimAndroidSocklen* address_length);
int aim_bionic_socket_poll(AimAndroidPollfd* descriptors,
                                  uint32_t count, int timeout_ms);
int aim_bionic_socket_ppoll(
    AimAndroidPollfd* descriptors, uint32_t count,
    const AimAndroidTimespec* timeout, const void* android_sigset);
intptr_t aim_bionic_socket_sendmsg(
    int fd, const AimAndroidMsghdr* message, int flags);
intptr_t aim_bionic_socket_recvmsg(
    int fd, AimAndroidMsghdr* message, int flags);

AimBionicSocketFunction aim_bionic_socket_resolve(
    const char* soname, const char* symbol, const char* version);
const char* aim_bionic_socket_capability(const char* capability);

/* Standalone-owner lifecycle. Reset closes every token and waits only for the
 * table lock; operations borrow a duplicate, so an already-admitted call may
 * finish safely after reset or close. */
void aim_bionic_socket_reset_for_test(void);
size_t aim_bionic_socket_live_tokens_for_test(void);
size_t aim_bionic_socket_borrowed_descriptors_for_test(void);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // AIM_BIONIC_SOCKET_H_
