#ifndef AIM_BIONIC_FS_H_
#define AIM_BIONIC_FS_H_

#include <stddef.h>
#include <stdint.h>
#include <sys/uio.h>
#include "aim_bionic_stat.h"

#include "aim_bionic_ioctl.h"

#ifdef __cplusplus
extern "C" {
#endif

enum {
  AIM_ANDROID_AT_FDCWD = -100,
  AIM_ANDROID_O_RDONLY = 0,
  AIM_ANDROID_O_WRONLY = 1,
  AIM_ANDROID_O_RDWR = 2,
  AIM_ANDROID_O_NONBLOCK = 2048,
  AIM_ANDROID_O_DIRECTORY = 16384,
  AIM_ANDROID_O_NOFOLLOW = 32768,
  AIM_ANDROID_O_LARGEFILE = 131072,
  AIM_ANDROID_O_CLOEXEC = 524288,
  AIM_ANDROID_AT_SYMLINK_NOFOLLOW = 0x100,
  AIM_ANDROID_AT_REMOVEDIR = 0x200,
};

typedef struct AimAndroidDirent {
  uint64_t d_ino;
  int64_t d_off;
  uint16_t d_reclen;
  uint8_t d_type;
  char d_name[256];
} AimAndroidDirent;

typedef struct AimAndroidStatvfs {
  uint64_t f_bsize;
  uint64_t f_frsize;
  uint64_t f_blocks;
  uint64_t f_bfree;
  uint64_t f_bavail;
  uint64_t f_files;
  uint64_t f_ffree;
  uint64_t f_favail;
  uint64_t f_fsid;
  uint64_t f_flag;
  uint64_t f_namemax;
  uint32_t __f_reserved[6];
} AimAndroidStatvfs;

typedef struct AimHostDirent {
  uint64_t d_ino;
  uint16_t d_name_length;
  uint8_t d_type;
  uint8_t d_name[256];
} AimHostDirent;

typedef struct AimHostStatvfs {
  uint64_t f_bsize;
  uint64_t f_frsize;
  uint64_t f_blocks;
  uint64_t f_bfree;
  uint64_t f_bavail;
  uint64_t f_files;
  uint64_t f_ffree;
  uint64_t f_favail;
  uint64_t f_fsid;
  uint64_t f_flag;
  uint64_t f_namemax;
} AimHostStatvfs;

typedef void (*AimBionicFsFunction)(void);
typedef int (*AimBionicFsHostDescriptorResolver)(int guest_fd,
                                                        int* host_fd);

typedef enum AimBionicFsProcessOwnerStatus {
  AIM_BIONIC_FS_PROCESS_OWNER_OK = 0,
  AIM_BIONIC_FS_PROCESS_OWNER_INVALID_ARGUMENT = 1,
  AIM_BIONIC_FS_PROCESS_OWNER_ALREADY_INSTALLED = 2,
  AIM_BIONIC_FS_PROCESS_OWNER_CREATE_FAILED = 3,
  AIM_BIONIC_FS_PROCESS_OWNER_NOT_INSTALLED = 4,
  AIM_BIONIC_FS_PROCESS_OWNER_BUSY = 5,
} AimBionicFsProcessOwnerStatus;

/* Installs one process-wide owner from an already-authorized Darwin directory
 * fd. The fd is duplicated; caller ownership is unchanged. Byte paths are not
 * C strings. A second live owner is rejected rather than aliased. */
AimBionicFsProcessOwnerStatus aim_bionic_fs_process_install(
    int root_fd, const uint8_t* guest_mount, size_t guest_mount_length,
    const uint8_t* cwd, size_t cwd_length);
/* Stops admission and waits for all in-flight filesystem and ioctl-lookup
 * leases before destroying the owner. */
AimBionicFsProcessOwnerStatus
aim_bionic_fs_process_uninstall(void);
/* Returns one/zero for the live owner and -1 when no owner is available. */
int aim_bionic_fs_process_has_capability_failure(void);
/* Seeds one process-authorized app directory hierarchy in the private /data
 * overlay. Immutable mounts are rejected. */
int aim_bionic_fs_seed_private_directory(const char* path);
/* Resolves only an authorized Android /data path to its host backing path.
 * A null/zero output buffer queries the required byte length (excluding NUL).
 * Immutable mounts and traversal outside the private overlay are rejected. */
intptr_t aim_bionic_fs_resolve_private_host_path(
    const char* path, char* output, size_t capacity);
/* As resolve_private_host_path, for any writable mount: private data,
 * shared storage and the system server's /data/app. */
intptr_t aim_bionic_fs_resolve_writable_host_path(const char* path, char* output,
                                                         size_t capacity);

/* Fixed-register forms intentionally capture the Android AAPCS64 mode slot.
 * Calls without O_CREAT leave mode unspecified and the implementation ignores it. */
int aim_bionic_open(const char* path, int flags, uint32_t mode);
int aim_bionic_openat(int directory_fd, const char* path, int flags,
                             uint32_t mode);
intptr_t aim_bionic_read(int fd, void* buffer, size_t count);
intptr_t aim_bionic_pread(int fd, void* buffer, size_t count,
                                 int64_t offset);
intptr_t aim_bionic_pwrite(int fd, const void* buffer, size_t count,
                                  int64_t offset);
intptr_t aim_bionic_write(int fd, const void* buffer, size_t count);
/* Vector operations retain one readv/writev operation at the virtual-FD
 * boundary; callers must not emulate them by issuing one syscall per iovec. */
intptr_t aim_bionic_readv(int fd, const struct iovec* vectors, int count);
intptr_t aim_bionic_writev(int fd, const struct iovec* vectors, int count);
intptr_t aim_bionic___write_chk(int fd, const void* buffer,
                                       size_t count, size_t buffer_size);
int64_t aim_bionic_lseek(int fd, int64_t offset, int whence);
int aim_bionic_close(int fd);
int aim_bionic_access(const char* path, int mode);
int aim_bionic_fstat(int fd, AimAndroidStat* status);
int aim_bionic_fstatat(int fd, const char* path,
                            AimAndroidStat* status, int flags);
int aim_bionic_fdatasync(int fd);
int aim_bionic_fsync(int fd);
int aim_bionic_fs_fsync_core(int fd);
int aim_bionic_stat(const char* path, AimAndroidStat* status);
int aim_bionic_lstat(const char* path, AimAndroidStat* status);
intptr_t aim_bionic_readlink(const char* path, char* buffer,
                                    size_t size);
char* aim_bionic_getcwd(char* buffer, size_t size);
int aim_bionic_chdir(const char* path);
int aim_bionic_fchdir(int fd);
int aim_bionic_chmod(const char* path, uint32_t mode);
void* aim_bionic_opendir(const char* path);
void* aim_bionic_fdopendir(int fd);
AimAndroidDirent* aim_bionic_readdir(void* directory);
void aim_bionic_rewinddir(void* directory);
int aim_bionic_closedir(void* directory);
int aim_bionic_dirfd(void* directory);
int aim_bionic_fchmod(int fd, uint32_t mode);
int aim_bionic_fchown(int fd, uint32_t owner, uint32_t group);
int aim_bionic_fchmodat(int directory_fd, const char* path,
                               uint32_t mode, int flags);
int aim_bionic_ftruncate(int fd, int64_t length);
int aim_bionic_posix_fallocate(int fd, int64_t offset, int64_t length);
int aim_bionic_isatty(int fd);
int aim_bionic_link(const char* old_path, const char* new_path);
int aim_bionic_mkdir(const char* path, uint32_t mode);
int aim_bionic_mkdirat(int directory_fd, const char* path, uint32_t mode);
int aim_bionic_mkstemp(char* path_template);
int aim_bionic_mkstemp64(char* path_template);
int64_t aim_bionic_pathconf(const char* path, int name);
char* aim_bionic_realpath(const char* path, char* resolved);
int aim_bionic_remove(const char* path);
int aim_bionic_rename(const char* old_path, const char* new_path);
int aim_bionic_statvfs(const char* path,
                              AimAndroidStatvfs* status);
int aim_bionic_symlink(const char* target, const char* link_path);
int aim_bionic_truncate(const char* path, int64_t length);
int aim_bionic_unlinkat(int directory_fd, const char* path, int flags);
int aim_bionic_utimensat(int directory_fd, const char* path,
                                const AimAndroidTimespec times[2],
                                int flags);
int aim_bionic_futimens(int fd,
                               const AimAndroidTimespec times[2]);

AimBionicFsFunction aim_bionic_fs_resolve(const char* import_name);

/* Exact callback for aim_bionic_ioctl_activate. Context is unused;
 * production lookup uses the process owner; a test-only pthread override wins
 * when present. */
AimBionicIoctlFdLookupStatus aim_bionic_fs_ioctl_fd_lookup(
    void* context, int32_t fd, AimBionicIoctlFdInfo* info);

/* Returns 1 and transfers a duplicate Darwin descriptor to the caller when
 * `fd` names a host-backed file, 0 when this facade does not own `fd`, and -1
 * with Android errno set when an owned descriptor cannot be represented. */
int aim_bionic_fs_dup_host_fd_core(int fd, int* host_fd);

/* Consumes one host descriptor returned by an in-process Android platform
 * component and publishes it in the same virtual file table used by Bionic.
 * The descriptor is closed on every failure path. */
int aim_bionic_fs_adopt_host_fd_core(int host_fd);
/* Returns one when `fd` belongs to this process filesystem table and zero for
 * host descriptors or descriptors owned by another central-broker provider. */
int aim_bionic_fs_owns_fd_core(int fd);

/* Rust implementation boundary called only by the errno-preserving shims. */
int aim_bionic_fs_open_core(const char* path, int flags, uint32_t mode);
int aim_bionic_fs_openat_core(int directory_fd, const char* path,
                                     int flags, uint32_t mode);

typedef int (*AimBionicSpecialDeviceOpen)(int flags, uint32_t mode,
                                                int* android_errno);
/* Process-global VFS device hook. Only the exact absolute /dev/binder path is
 * delegated; normal filesystem resolution remains owned by the facade. */
int aim_bionic_fs_bind_binder_device_open(
    AimBionicSpecialDeviceOpen callback);
/* Binds the process descriptor broker used only after the filesystem owner
 * does not recognize an fd. A successful callback transfers one host dup to
 * the filesystem facade, which retains Android stat layout ownership. */
int aim_bionic_fs_bind_host_descriptor_resolver(
    AimBionicFsHostDescriptorResolver callback);
intptr_t aim_bionic_fs_read_core(int fd, void* buffer, size_t count);
intptr_t aim_bionic_fs_pread_core(int fd, void* buffer, size_t count,
                                         int64_t offset);
intptr_t aim_bionic_fs_pwrite_core(int fd, const void* buffer,
                                          size_t count, int64_t offset);
intptr_t aim_bionic_fs_write_core(int fd, const void* buffer,
                                         size_t count);
intptr_t aim_bionic_fs_readv_core(int fd, const struct iovec* vectors,
                                         int count);
intptr_t aim_bionic_fs_writev_core(int fd, const struct iovec* vectors,
                                          int count);
int64_t aim_bionic_fs_lseek_core(int fd, int64_t offset, int whence);
int aim_bionic_fs_close_core(int fd);
int aim_bionic_fs_flock_core(int fd, int operation);
int aim_bionic_fs_fcntl_core(int fd, int command, intptr_t argument);
int aim_bionic_fs_fstat_core(int fd, AimAndroidStat* status);
int aim_bionic_fs_fchdir_core(int fd);
int aim_bionic_fs_fstatat_core(int fd, const char* path,
                                    AimAndroidStat* status, int flags);
int aim_bionic_fs_stat_core(const char* path,
                                   AimAndroidStat* status);
int aim_bionic_fs_lstat_core(const char* path,
                                    AimAndroidStat* status);
intptr_t aim_bionic_fs_readlink_core(const char* path, char* buffer,
                                            size_t size);
char* aim_bionic_fs_getcwd_core(char* buffer, size_t size);
int aim_bionic_fs_chdir_core(const char* path);
int aim_bionic_fs_chmod_core(const char* path, uint32_t mode);
// Linux getxattr/setxattr/removexattr/listxattr (l* variants with no_follow).
ssize_t aim_bionic_fs_getxattr_core(const char* path, const char* name, void* value,
                                           size_t size, int no_follow);
int aim_bionic_fs_setxattr_core(const char* path, const char* name, const void* value,
                                       size_t size, int flags, int no_follow);
int aim_bionic_fs_removexattr_core(const char* path, const char* name, int no_follow);
ssize_t aim_bionic_fs_listxattr_core(const char* path, char* list, size_t size,
                                            int no_follow);
void* aim_bionic_fs_opendir_core(const char* path);
void* aim_bionic_fs_fdopendir_core(int fd);
AimAndroidDirent* aim_bionic_fs_readdir_core(void* directory);
void aim_bionic_fs_rewinddir_core(void* directory);
int aim_bionic_fs_closedir_core(void* directory);
int aim_bionic_fs_dirfd_core(void* directory);
int aim_bionic_fs_fchmod_core(int fd, uint32_t mode);
int aim_bionic_fs_fchown_core(int fd, uint32_t owner, uint32_t group);
int aim_bionic_fs_fchmodat_core(int directory_fd, const char* path,
                                      uint32_t mode, int flags);
int aim_bionic_fs_ftruncate_core(int fd, int64_t length);
int aim_bionic_fs_posix_fallocate_core(int fd, int64_t offset,
                                              int64_t length);
int aim_bionic_fs_isatty_core(int fd);
int aim_bionic_fs_link_core(const char* old_path,
                                   const char* new_path);
int aim_bionic_fs_mkdir_core(const char* path, uint32_t mode);
int aim_bionic_fs_mkdirat_core(int directory_fd, const char* path,
                                     uint32_t mode);
int64_t aim_bionic_fs_pathconf_core(const char* path, int name);
char* aim_bionic_fs_realpath_core(const char* path, char* resolved);
int aim_bionic_fs_remove_core(const char* path);
int aim_bionic_fs_rename_core(const char* old_path,
                                     const char* new_path);
int aim_bionic_fs_statvfs_core(const char* path,
                                      AimAndroidStatvfs* status);
int aim_bionic_fs_symlink_core(const char* target,
                                      const char* link_path);
int aim_bionic_fs_truncate_core(const char* path, int64_t length);
int aim_bionic_fs_unlinkat_core(int directory_fd, const char* path,
                                      int flags);
int aim_bionic_fs_utimensat_core(
    int directory_fd, const char* path,
    const AimAndroidTimespec times[2], int flags);

/* Darwin-only helpers. Their opaque stream never crosses the guest facade. */
__attribute__((visibility("hidden"))) void*
aim_bionic_fs_host_fdopendir(int fd, int* host_errno);
__attribute__((visibility("hidden"))) int aim_bionic_fs_host_readdir(
    void* directory, AimHostDirent* entry, int* host_errno);
__attribute__((visibility("hidden"))) int aim_bionic_fs_host_closedir(
    void* directory, int* host_errno);
__attribute__((visibility("hidden"))) int aim_bionic_fs_host_fpathconf(
    int fd, int semantic_name, int64_t* value, int* host_errno);
__attribute__((visibility("hidden"))) int aim_bionic_fs_host_fstatvfs(
    int fd, AimHostStatvfs* status, int* host_errno);
__attribute__((visibility("hidden"))) int
aim_bionic_fs_host_record_lock(int host_fd, int android_command,
                                      intptr_t android_lock,
                                      int* host_errno);
/* Host topology is queried through the native shim so Rust never infers it
 * from process affinity (which may describe a virtual guest restriction). */
__attribute__((visibility("hidden"))) long
aim_bionic_fs_host_cpu_count(int online);

/* Trusted synchronous group admission. Consumes distinct native FDs on every
 * outcome. Callback runs under the FS table lock: no RPC/wait/FS reentry or
 * managed-object destruction. Zero commits; positive Android errno rolls back.
 * Outputs are untouched on failure. count <= 16 and a complete nonnull input
 * array are call preconditions; violating them does not transfer ownership.
 * Central-only groups bypass this port and commit directly. */
typedef struct AimFsOwnedDescriptor {
  int host_fd;
  uint32_t descriptor_flags;
} AimFsOwnedDescriptor;
typedef int (*AimFsCommitGroup)(void*, const int*, size_t);
int aim_bionic_fs_adopt_group(
    const AimFsOwnedDescriptor* entries, size_t count,
    AimFsCommitGroup commit, void* context, int* guest_fds);

#ifdef __cplusplus
}
#endif

#endif
