#pragma once

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

int aim_binder_platform_open(const char *path, int flags, ...);
int aim_binder_platform_access(const char *path, int mode);
ssize_t aim_binder_platform_read(int fd, void *buffer, size_t count);
int aim_binder_platform_close(int fd);
int aim_binder_platform_ioctl(int fd, unsigned long request,
                                     void *argument);
void *aim_binder_platform_mmap(void *address, size_t length,
                                      int protection, int flags, int fd,
                                      off_t offset);
int aim_binder_platform_munmap(void *address, size_t length);
// The process's Android uid (IPCThreadState's own calling uid).
uid_t aim_binder_platform_getuid(void);

#ifdef __cplusplus
}
#endif

// AOSP libbinder owns Binder protocol and thread semantics. These aliases only
// replace the POSIX device edge when the same sources are compiled as Mach-O;
// they must be included after the source's host system headers.
#if defined(AIM_BINDER_PROCESS_STATE_SYSCALLS)
#define open aim_binder_platform_open
#define access aim_binder_platform_access
#define read aim_binder_platform_read
#define close aim_binder_platform_close
#define ioctl aim_binder_platform_ioctl
#define mmap aim_binder_platform_mmap
#define munmap aim_binder_platform_munmap
#elif defined(AIM_BINDER_IPC_THREAD_STATE_SYSCALLS)
#define close aim_binder_platform_close
#define ioctl aim_binder_platform_ioctl
#define getuid aim_binder_platform_getuid
#endif
