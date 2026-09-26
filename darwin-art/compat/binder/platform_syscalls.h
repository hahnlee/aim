#pragma once

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

int darwin_art_binder_platform_open(const char *path, int flags, ...);
int darwin_art_binder_platform_access(const char *path, int mode);
ssize_t darwin_art_binder_platform_read(int fd, void *buffer, size_t count);
int darwin_art_binder_platform_close(int fd);
int darwin_art_binder_platform_ioctl(int fd, unsigned long request,
                                     void *argument);
void *darwin_art_binder_platform_mmap(void *address, size_t length,
                                      int protection, int flags, int fd,
                                      off_t offset);
int darwin_art_binder_platform_munmap(void *address, size_t length);
// The process's Android uid (IPCThreadState's own calling uid).
uid_t darwin_art_binder_platform_getuid(void);

#ifdef __cplusplus
}
#endif

// AOSP libbinder owns Binder protocol and thread semantics. These aliases only
// replace the POSIX device edge when the same sources are compiled as Mach-O;
// they must be included after the source's host system headers.
#if defined(DARWIN_ART_BINDER_PROCESS_STATE_SYSCALLS)
#define open darwin_art_binder_platform_open
#define access darwin_art_binder_platform_access
#define read darwin_art_binder_platform_read
#define close darwin_art_binder_platform_close
#define ioctl darwin_art_binder_platform_ioctl
#define mmap darwin_art_binder_platform_mmap
#define munmap darwin_art_binder_platform_munmap
#elif defined(DARWIN_ART_BINDER_IPC_THREAD_STATE_SYSCALLS)
#define close darwin_art_binder_platform_close
#define ioctl darwin_art_binder_platform_ioctl
#define getuid darwin_art_binder_platform_getuid
#endif
