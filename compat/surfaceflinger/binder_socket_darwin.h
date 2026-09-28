#pragma once

#include <sys/socket.h>

#if defined(__APPLE__)
#ifndef MSG_CMSG_CLOEXEC
#define MSG_CMSG_CLOEXEC 0
#endif
#ifndef SOCK_CLOEXEC
#define SOCK_CLOEXEC 0
#endif
#ifndef SOCK_NONBLOCK
#define SOCK_NONBLOCK 0
#endif

#ifdef __cplusplus
extern "C" {
#endif
int aim_accept4(int socket_fd, struct sockaddr* address,
                       socklen_t* address_length, int flags);
#ifdef __cplusplus
}
#endif

#define accept4 aim_accept4
#endif
