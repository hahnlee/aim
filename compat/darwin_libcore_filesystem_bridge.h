#ifndef AIM_LIBCORE_FILESYSTEM_BRIDGE_H_
#define AIM_LIBCORE_FILESYSTEM_BRIDGE_H_

#include <sys/stat.h>
#include <sys/mount.h>
#include <dirent.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include "../tools/bionic-fs-facade/include/aim_bionic_stat.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef int (*AimLibcoreStatProvider)(const char*,
                                            AimAndroidStat*);
typedef int (*AimLibcoreModeProvider)(const char*, uint32_t);
typedef int32_t (*AimLibcoreErrnoProvider)(void);

void aim_libcore_install_filesystem_provider(
    AimLibcoreStatProvider stat_provider,
    AimLibcoreModeProvider mkdir_provider,
    AimLibcoreModeProvider chmod_provider,
    AimLibcoreErrnoProvider errno_provider);

int aim_libcore_stat(const char* path, struct stat* status);
int aim_libcore_fstat(int fd, struct stat* status);
int aim_libcore_open(const char* path, int flags, ...);
int aim_libcore_close(int fd);
int aim_libcore_mkdir(const char* path, mode_t mode);
int aim_libcore_chmod(const char* path, mode_t mode);
int aim_libcore_statfs(const char* path, struct statfs* status);
DIR* aim_libcore_opendir(const char* path);
struct dirent* aim_libcore_readdir(DIR* directory);
int aim_libcore_closedir(DIR* directory);
char* aim_libcore_realpath(const char* path, char* resolved);

#ifdef __cplusplus
}
#endif

// Install redirects only after the Darwin declarations have been parsed.
// Command-line -Dstatfs rewrites the declaration but retains its asm("statfs")
// symbol alias, silently binding the supposed bridge back to the host syscall.
#ifdef AIM_LIBCORE_REDIRECT_FILESYSTEM
#define stat(path, status) aim_libcore_stat(path, status)
#define fstat(fd, status) aim_libcore_fstat(fd, status)
#define open(path, flags, ...) \
  aim_libcore_open(path, flags, ##__VA_ARGS__)
#define close(fd) aim_libcore_close(fd)
#define statfs(path, status) aim_libcore_statfs(path, status)
#define mkdir(path, mode) aim_libcore_mkdir(path, mode)
#define chmod(path, mode) aim_libcore_chmod(path, mode)
#define opendir(path) aim_libcore_opendir(path)
// UnixFileSystem_md.c aliases readdir64 to readdir on Darwin. Redirect the
// underlying spelling so that alias remains source-compatible without a
// conflicting readdir64 macro definition.
#define readdir(directory) aim_libcore_readdir(directory)
#define closedir(directory) aim_libcore_closedir(directory)
#define realpath(path, resolved) aim_libcore_realpath(path, resolved)
#endif

#endif  // AIM_LIBCORE_FILESYSTEM_BRIDGE_H_
