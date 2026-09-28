#include "aim_bionic_fs.h"
#include "aim_bionic_fortify_io.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <libproc.h>
#include <stddef.h>
#include <stdlib.h>
#include <stdatomic.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/statvfs.h>
#include <unistd.h>

extern void aim_bionic_errno_store(int32_t android_errno);
extern int32_t aim_bionic_errno_load(void);

static int g_trace_fs_failures;
_Static_assert(ATOMIC_INT_LOCK_FREE == 2, "diagnostic counter must be lock-free");
__attribute__((constructor)) static void initialize_fs_diagnostics(void) {
  g_trace_fs_failures = getenv("AIM_TRACE_FS_FAILURES") != NULL;
}

static size_t trace_append(char* output, size_t used, const char* text,
                           size_t limit) {
  for (size_t i = 0; i < limit && text[i] != '\0'; ++i) output[used++] = text[i];
  return used;
}

static size_t trace_append_int(char* output, size_t used, int value) {
  unsigned magnitude = (unsigned)value;
  if (value < 0) { output[used++] = '-'; magnitude = 0u - magnitude; }
  char digits[10];
  size_t count = 0;
  do { digits[count++] = (char)('0' + magnitude % 10); magnitude /= 10; }
  while (magnitude != 0);
  while (count != 0) output[used++] = digits[--count];
  return used;
}

// Guest GC can suspend another thread holding host getenv/stdio locks. Keep
// diagnostics lock-free after eager initialization, including formatting.
// Never emit file contents; standard-FD failures have a separate small budget
// so repeated stderr failures cannot hide later resource-path failures.
static void trace_fs_failure(const char* operation, const char* path, int fd) {
  if (!g_trace_fs_failures) return;
  const int guest_error = aim_bionic_errno_load();
  const int host_error = errno;
  static _Atomic unsigned emitted;
  static _Atomic unsigned standard_emitted;
  const int standard_fd = fd >= 0 && fd <= STDERR_FILENO;
  const unsigned index = atomic_fetch_add_explicit(
      standard_fd ? &standard_emitted : &emitted, 1, memory_order_relaxed);
  if (index < (standard_fd ? 8u : 512u)) {
    char line[640];
    size_t used = trace_append(line, 0, "ART filesystem failure: op=", 32);
    used = trace_append(line, used, operation, 8);
    used = trace_append(line, used, " fd=", 4);
    used = trace_append_int(line, used, fd);
    used = trace_append(line, used, " errno=", 7);
    used = trace_append_int(line, used, guest_error);
    used = trace_append(line, used, " path=", 6);
    used = trace_append(line, used,
        path != NULL && guest_error != 14 ? path : "(unavailable)", 512);
    line[used++] = '\n';
    (void)write(STDERR_FILENO, line, used);
  }
  aim_bionic_errno_store(guest_error);
  errno = host_error;
}

typedef int (*AimProcMapsRegionCallback)(void* context, uint64_t start,
                                               uint64_t end,
                                               int protection,
                                               const char* path,
                                               uint64_t file_offset);

int aim_bionic_fs_host_enumerate_regions(
    AimProcMapsRegionCallback callback, void* context) {
  if (callback == NULL) return EINVAL;
  mach_vm_address_t address = 0;
  natural_t depth = 0;
  for (;;) {
    mach_vm_size_t size = 0;
    vm_region_submap_info_data_64_t info = {0};
    mach_msg_type_number_t count = VM_REGION_SUBMAP_INFO_COUNT_64;
    kern_return_t result = mach_vm_region_recurse(
        mach_task_self(), &address, &size, &depth,
        (vm_region_recurse_info_t)&info, &count);
    if (result == KERN_INVALID_ADDRESS) return 0;
    if (result != KERN_SUCCESS || size == 0) return EIO;
    if (info.is_submap) {
      ++depth;
      continue;
    }
    if (address > UINT64_MAX - size) return EOVERFLOW;
    const uint64_t end = address + size;
    // Mach's region API reports protection and bounds, but not the backing
    // vnode name.  proc_pidinfo is the Darwin equivalent needed to expose
    // Android's /proc/*/{maps,smaps} pathname and offset fields.  Anonymous
    // regions, and regions for which the diagnostic query is unavailable,
    // deliberately remain unnamed rather than leaking an unrelated host
    // path or failing the complete proc snapshot.
    const char* path = NULL;
    uint64_t file_offset = 0;
    struct proc_regionwithpathinfo path_info = {0};
    const int path_result = proc_pidinfo(
        getpid(), PROC_PIDREGIONPATHINFO, address, &path_info,
        sizeof(path_info));
    if (path_result == (int)sizeof(path_info) &&
        path_info.prp_prinfo.pri_address <= address &&
        address - path_info.prp_prinfo.pri_address < path_info.prp_prinfo.pri_size &&
        path_info.prp_vip.vip_path[0] != '\0') {
      path = path_info.prp_vip.vip_path;
      file_offset = path_info.prp_prinfo.pri_offset +
          (address - path_info.prp_prinfo.pri_address);
    }
    if (callback(context, address, end, info.protection, path, file_offset) != 0) {
      return ENOMEM;
    }
    address = end;
  }
}

_Static_assert(sizeof(void*) == 8, "Android and Darwin arm64 pointer width drift");
_Static_assert(sizeof(size_t) == 8, "Android and Darwin arm64 size_t width drift");
_Static_assert(sizeof(intptr_t) == 8, "Android ssize_t/Darwin intptr_t width drift");
_Static_assert(_SC_NPROCESSORS_CONF == 57 && _SC_NPROCESSORS_ONLN == 58,
               "Darwin sysconf processor identifiers drift");
_Static_assert(sizeof(AimAndroidStat) == 128, "Android arm64 stat size drift");
_Static_assert(offsetof(AimAndroidStat, st_mode) == 16, "stat mode offset drift");
_Static_assert(offsetof(AimAndroidStat, st_size) == 48, "stat size offset drift");
_Static_assert(offsetof(AimAndroidStat, st_blocks) == 64,
               "stat blocks offset drift");
_Static_assert(offsetof(AimAndroidStat, st_atim) == 72,
               "stat timestamp offset drift");
_Static_assert(sizeof(AimAndroidDirent) == 280,
               "Android arm64 dirent size drift");
_Static_assert(offsetof(AimAndroidDirent, d_name) == 19,
               "Android arm64 dirent name offset drift");
_Static_assert(sizeof(AimAndroidTimespec) == 16,
               "Android arm64 timespec size drift");
_Static_assert(sizeof(AimAndroidStatvfs) == 112,
               "Android arm64 statvfs size drift");
_Static_assert(offsetof(AimAndroidStatvfs, f_flag) == 72,
               "Android arm64 statvfs flag offset drift");
_Static_assert(sizeof(AimHostStatvfs) == 88,
               "portable host statvfs size drift");
_Static_assert(DT_UNKNOWN == 0 && DT_FIFO == 1 && DT_CHR == 2 && DT_DIR == 4 &&
                   DT_BLK == 6 && DT_REG == 8 && DT_LNK == 10 &&
                   DT_SOCK == 12 && DT_WHT == 14,
               "Darwin directory type values drifted from explicit translator");

typedef struct AimAndroidFlock {
  int16_t l_type;
  int16_t l_whence;
  int32_t __pad0;
  int64_t l_start;
  int64_t l_len;
  int32_t l_pid;
  int32_t __pad1;
} AimAndroidFlock;

static _Atomic uint32_t g_android_umask = 0022;

uint32_t aim_bionic___umask_chk(uint32_t mask) {
  if ((mask & ~0777u) != 0) abort();
  return atomic_exchange_explicit(&g_android_umask, mask, memory_order_relaxed);
}

_Static_assert(sizeof(AimAndroidFlock) == 32,
               "Android arm64 flock size drift");
_Static_assert(offsetof(AimAndroidFlock, l_start) == 8,
               "Android arm64 flock start offset drift");

int aim_bionic_fs_host_record_lock(int host_fd, int android_command,
                                          intptr_t android_lock,
                                          int* host_errno) {
  if (android_lock == 0 || host_errno == NULL) return -2;
  AimAndroidFlock* android = (AimAndroidFlock*)android_lock;
  struct flock host = {0};
  switch (android->l_type) {
    case 0:
      host.l_type = F_RDLCK;
      break;
    case 1:
      host.l_type = F_WRLCK;
      break;
    case 2:
      host.l_type = F_UNLCK;
      break;
    default:
      return -2;
  }
  if (android->l_whence < 0 || android->l_whence > 2) return -2;
  host.l_whence = android->l_whence;
  host.l_start = android->l_start;
  host.l_len = android->l_len;
  int command = 0;
  switch (android_command) {
    case 5:
      command = F_GETLK;
      break;
    case 6:
      command = F_SETLK;
      break;
    case 7:
      command = F_SETLKW;
      break;
    default:
      return -2;
  }
  const int result = fcntl(host_fd, command, &host);
  *host_errno = result == 0 ? 0 : errno;
  if (result != 0) return -1;
  if (android_command == 5) {
    android->l_type = host.l_type == F_RDLCK ? 0
                      : host.l_type == F_WRLCK ? 1
                                               : 2;
    android->l_whence = host.l_whence;
    android->l_start = host.l_start;
    android->l_len = host.l_len;
    android->l_pid = host.l_pid;
  }
  return 0;
}


int aim_bionic_open(const char* path, int flags, uint32_t mode) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_open_core(path, flags, mode);
  if (result < 0) trace_fs_failure("open", path, -1);
  errno = saved_host_errno;
  return result;
}

long aim_bionic_fs_host_cpu_count(int online) {
  const int saved_errno = errno;
  const long result =
      sysconf(online ? _SC_NPROCESSORS_ONLN : _SC_NPROCESSORS_CONF);
  errno = saved_errno;
  return result;
}

int aim_bionic_openat(int directory_fd, const char* path, int flags,
                             uint32_t mode) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_fs_openat_core(directory_fd, path, flags, mode);
  if (result < 0) trace_fs_failure("openat", path, directory_fd);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_chmod(const char* path, uint32_t mode) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_chmod_core(path, mode);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_read(int fd, void* buffer, size_t count) {
  const int saved_host_errno = errno;
  const intptr_t result = aim_bionic_fs_read_core(fd, buffer, count);
  if (result < 0) trace_fs_failure("read", NULL, fd);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_pread(int fd, void* buffer, size_t count,
                                 int64_t offset) {
  const int saved_host_errno = errno;
  const intptr_t result =
      aim_bionic_fs_pread_core(fd, buffer, count, offset);
  if (result < 0) trace_fs_failure("pread", NULL, fd);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_pwrite(int fd, const void* buffer, size_t count,
                                  int64_t offset) {
  const int saved_host_errno = errno;
  const intptr_t result =
      aim_bionic_fs_pwrite_core(fd, buffer, count, offset);
  if (result < 0) trace_fs_failure("pwrite", NULL, fd);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_write(int fd, const void* buffer, size_t count) {
  const int saved_host_errno = errno;
  const intptr_t result = aim_bionic_fs_write_core(fd, buffer, count);
  if (result < 0) trace_fs_failure("write", NULL, fd);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_readv(int fd, const struct iovec* vectors, int count) {
  const int saved_host_errno = errno;
  const intptr_t result = aim_bionic_fs_readv_core(fd, vectors, count);
  if (result < 0) trace_fs_failure("readv", NULL, fd);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_writev(int fd, const struct iovec* vectors, int count) {
  const int saved_host_errno = errno;
  const intptr_t result = aim_bionic_fs_writev_core(fd, vectors, count);
  if (result < 0) trace_fs_failure("writev", NULL, fd);
  errno = saved_host_errno;
  return result;
}

int64_t aim_bionic_lseek(int fd, int64_t offset, int whence) {
  const int saved_host_errno = errno;
  const int64_t result = aim_bionic_fs_lseek_core(fd, offset, whence);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_access(const char* path, int mode) {
  if ((mode & ~7) != 0) {
    aim_bionic_errno_store(22);
    return -1;
  }
  AimAndroidStat status;
  return aim_bionic_stat(path, &status);
}

int aim_bionic_fdatasync(int fd) {
  AimAndroidStat status;
  return aim_bionic_fstat(fd, &status);
}

int aim_bionic_fsync(int fd) {
  if (fd < 0) {
    errno = EBADF;
    return -1;
  }
  return aim_bionic_fs_fsync_core(fd);
}

int aim_bionic_unlink(const char* path) {
  return aim_bionic_unlinkat(AIM_ANDROID_AT_FDCWD, path, 0);
}

int aim_bionic_rmdir(const char* path) {
  return aim_bionic_unlinkat(AIM_ANDROID_AT_FDCWD, path,
                                    AIM_ANDROID_AT_REMOVEDIR);
}

int aim_bionic___open_2(const char* path, int flags) {
  return aim_bionic_open(path, flags, 0);
}

int aim_bionic___openat_2(int directory_fd, const char* path,
                                 int flags) {
  return aim_bionic_openat(directory_fd, path, flags, 0);
}

intptr_t aim_bionic___read_chk(int fd, void* buffer, size_t count,
                                      size_t buffer_size) {
  aim_bionic_check_io_buffer(count, buffer_size);
  return aim_bionic_read(fd, buffer, count);
}

intptr_t aim_bionic___write_chk(int fd, const void* buffer,
                                       size_t count, size_t buffer_size) {
  aim_bionic_check_io_buffer(count, buffer_size);
  return aim_bionic_write(fd, buffer, count);
}

intptr_t aim_bionic___pread_chk(int fd, void* buffer, size_t count,
                                       int64_t offset, size_t buffer_size) {
  aim_bionic_check_io_buffer(count, buffer_size);
  return aim_bionic_pread(fd, buffer, count, offset);
}

intptr_t aim_bionic___pwrite_chk(int fd, const void* buffer,
                                        size_t count, int64_t offset,
                                        size_t buffer_size) {
  aim_bionic_check_io_buffer(count, buffer_size);
  return aim_bionic_pwrite(fd, buffer, count, offset);
}

intptr_t aim_bionic___readlink_chk(const char* path, char* buffer,
                                          size_t size, size_t buffer_size) {
  aim_bionic_check_io_buffer(size, buffer_size);
  return aim_bionic_readlink(path, buffer, size);
}

int aim_bionic_creat(const char* path, uint32_t mode) {
  return aim_bionic_open(path, 1 | 64 | 512, mode);
}

int aim_bionic_close(int fd) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_close_core(fd);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_fstat(int fd, AimAndroidStat* status) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_fstat_core(fd, status);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_stat(const char* path, AimAndroidStat* status) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_stat_core(path, status);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_lstat(const char* path, AimAndroidStat* status) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_lstat_core(path, status);
  errno = saved_host_errno;
  return result;
}

intptr_t aim_bionic_readlink(const char* path, char* buffer,
                                    size_t size) {
  const int saved_host_errno = errno;
  const intptr_t result = aim_bionic_fs_readlink_core(path, buffer, size);
  errno = saved_host_errno;
  return result;
}

char* aim_bionic_getcwd(char* buffer, size_t size) {
  const int saved_host_errno = errno;
  char* result = aim_bionic_fs_getcwd_core(buffer, size);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_chdir(const char* path) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_chdir_core(path);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_fchdir(int fd) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_fchdir_core(fd);
  errno = saved_host_errno;
  return result;
}

void* aim_bionic_opendir(const char* path) {
  const int saved_host_errno = errno;
  void* result = aim_bionic_fs_opendir_core(path);
  errno = saved_host_errno;
  return result;
}

void* aim_bionic_fdopendir(int fd) {
  const int saved_host_errno = errno;
  void* result = aim_bionic_fs_fdopendir_core(fd);
  errno = saved_host_errno;
  return result;
}

AimAndroidDirent* aim_bionic_readdir(void* directory) {
  const int saved_host_errno = errno;
  AimAndroidDirent* result = aim_bionic_fs_readdir_core(directory);
  errno = saved_host_errno;
  return result;
}

void aim_bionic_rewinddir(void* directory) {
  const int saved_host_errno = errno;
  aim_bionic_fs_rewinddir_core(directory);
  errno = saved_host_errno;
}

int64_t aim_bionic_lseek64(int fd, int64_t offset, int whence) {
  return aim_bionic_lseek(fd, offset, whence);
}

int aim_bionic_fstat64(int fd, AimAndroidStat* status) {
  return aim_bionic_fstat(fd, status);
}

int aim_bionic_lstat64(const char* path, AimAndroidStat* status) {
  return aim_bionic_lstat(path, status);
}

int aim_bionic_posix_fadvise(int fd, int64_t offset, int64_t length,
                                    int advice) {
  (void)fd;
  (void)offset;
  (void)length;
  (void)advice;
  return 0;
}

int aim_bionic_mkstemp(char* path_template) {
  static _Atomic uint32_t sequence = UINT32_C(0x13579bdf);
  if (path_template == NULL) {
    aim_bionic_errno_store(14);
    return -1;
  }
  size_t length = 0;
  while (path_template[length] != '\0') ++length;
  if (length < 6) {
    aim_bionic_errno_store(22);
    return -1;
  }
  for (size_t index = length - 6; index < length; ++index) {
    if (path_template[index] != 'X') {
      aim_bionic_errno_store(22);
      return -1;
    }
  }
  static const char alphabet[] =
      "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
  for (uint32_t attempt = 0; attempt < 256; ++attempt) {
    uint32_t value = atomic_fetch_add_explicit(
        &sequence, UINT32_C(0x9e3779b9), memory_order_relaxed);
    value ^= (uint32_t)(uintptr_t)path_template;
    for (size_t index = 0; index < 6; ++index) {
      value = value * UINT32_C(1664525) + UINT32_C(1013904223);
      path_template[length - 6 + index] = alphabet[value % 62];
    }
    const int fd = aim_bionic_open(path_template, 2 | 64 | 128, 0600);
    if (fd >= 0) return fd;
  }
  for (size_t index = length - 6; index < length; ++index)
    path_template[index] = 'X';
  return -1;
}

int aim_bionic_mkstemp64(char* path_template) {
  return aim_bionic_mkstemp(path_template);
}

char* aim_bionic_mkdtemp(char* path_template) {
  static _Atomic uint32_t sequence = UINT32_C(0x2468ace1);
  if (path_template == NULL) {
    aim_bionic_errno_store(14);
    return NULL;
  }
  size_t length = 0;
  while (path_template[length] != '\0') ++length;
  if (length < 6) {
    aim_bionic_errno_store(22);
    return NULL;
  }
  for (size_t index = length - 6; index < length; ++index)
    if (path_template[index] != 'X') {
      aim_bionic_errno_store(22);
      return NULL;
    }
  static const char alphabet[] =
      "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
  for (uint32_t attempt = 0; attempt < 256; ++attempt) {
    uint32_t value = atomic_fetch_add_explicit(
        &sequence, UINT32_C(0x9e3779b9), memory_order_relaxed);
    for (size_t index = 0; index < 6; ++index) {
      value = value * UINT32_C(1664525) + UINT32_C(1013904223);
      path_template[length - 6 + index] = alphabet[value % 62];
    }
    if (aim_bionic_mkdir(path_template, 0700) == 0)
      return path_template;
  }
  for (size_t index = length - 6; index < length; ++index)
    path_template[index] = 'X';
  return NULL;
}

int aim_bionic_fs_path_unsupported(const char* path, void* value) {
  (void)path;
  (void)value;
  aim_bionic_errno_store(38);
  return -1;
}

unsigned aim_bionic_cfget_speed(const void* termios_value) {
  (void)termios_value;
  return 0;
}

int aim_bionic_cfset_speed(void* termios_value, unsigned speed) {
  (void)termios_value;
  (void)speed;
  return 0;
}

int aim_bionic_terminal_unsupported(int fd, uintptr_t value,
                                           uintptr_t extra) {
  (void)fd;
  (void)value;
  (void)extra;
  aim_bionic_errno_store(25);
  return -1;
}

int aim_bionic_flock_unsupported(int fd, int operation) {
  return aim_bionic_fs_flock_core(fd, operation);
}

int aim_bionic_fstatfs_unsupported(int fd, void* status) {
  (void)fd;
  (void)status;
  aim_bionic_errno_store(38);
  return -1;
}

int aim_bionic_dirfd(void* directory) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_dirfd_core(directory);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_fstatat(int directory_fd, const char* path,
                                         AimAndroidStat* status,
                                         int flags) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_fstatat_core(directory_fd, path, status, flags);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_readdir_r(void* directory, AimAndroidDirent* entry,
                                AimAndroidDirent** result) {
  if (entry == NULL || result == NULL) return 22;
  AimAndroidDirent* found = aim_bionic_readdir(directory);
  if (found == NULL) {
    *result = NULL;
    const int error = aim_bionic_errno_load();
    return error == 0 ? 0 : error;
  }
  *entry = *found;
  *result = entry;
  return 0;
}

int aim_bionic_closedir(void* directory) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_closedir_core(directory);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_fchmod(int fd, uint32_t mode) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_fchmod_core(fd, mode);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_fchown(int fd, uint32_t owner, uint32_t group) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_fchown_core(fd, owner, group);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_fchmodat(int directory_fd, const char* path,
                               uint32_t mode, int flags) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_fs_fchmodat_core(directory_fd, path, mode, flags);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_ftruncate(int fd, int64_t length) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_ftruncate_core(fd, length);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_posix_fallocate(int fd, int64_t offset, int64_t length) {
  return aim_bionic_fs_posix_fallocate_core(fd, offset, length);
}

int aim_bionic_isatty(int fd) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_isatty_core(fd);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_link(const char* old_path, const char* new_path) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_link_core(old_path, new_path);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_mkdir(const char* path, uint32_t mode) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_mkdir_core(path, mode);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_mkdirat(int directory_fd, const char* path, uint32_t mode) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_mkdirat_core(directory_fd, path, mode);
  errno = saved_host_errno;
  return result;
}

int64_t aim_bionic_pathconf(const char* path, int name) {
  const int saved_host_errno = errno;
  const int64_t result = aim_bionic_fs_pathconf_core(path, name);
  errno = saved_host_errno;
  return result;
}

char* aim_bionic_realpath(const char* path, char* resolved) {
  const int saved_host_errno = errno;
  char* result = aim_bionic_fs_realpath_core(path, resolved);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_remove(const char* path) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_remove_core(path);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_rename(const char* old_path, const char* new_path) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_rename_core(old_path, new_path);
  if (result < 0) trace_fs_failure("rename", old_path, -1);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_statvfs(const char* path,
                              AimAndroidStatvfs* status) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_statvfs_core(path, status);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_symlink(const char* target, const char* link_path) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_symlink_core(target, link_path);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_truncate(const char* path, int64_t length) {
  const int saved_host_errno = errno;
  const int result = aim_bionic_fs_truncate_core(path, length);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_unlinkat(int directory_fd, const char* path, int flags) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_fs_unlinkat_core(directory_fd, path, flags);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_utimensat(int directory_fd, const char* path,
                                const AimAndroidTimespec times[2],
                                int flags) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_fs_utimensat_core(directory_fd, path, times, flags);
  errno = saved_host_errno;
  return result;
}

int aim_bionic_futimens(int fd,
                               const AimAndroidTimespec times[2]) {
  const int saved_host_errno = errno;
  // Android's futimens is the descriptor form of utimensat.  Reuse the
  // descriptor-relative core so virtual descriptors never escape to Darwin's
  // host filesystem and both entry points share timestamp validation.
  const int result =
      aim_bionic_fs_utimensat_core(fd, NULL, times, 0);
  errno = saved_host_errno;
  return result;
}

void* aim_bionic_fs_host_fdopendir(int fd, int* host_errno) {
  errno = 0;
  DIR* result = fdopendir(fd);
  if (host_errno != NULL) *host_errno = result == NULL ? errno : 0;
  return result;
}

int aim_bionic_fs_host_readdir(void* directory,
                                      AimHostDirent* entry,
                                      int* host_errno) {
  if (directory == NULL || entry == NULL || host_errno == NULL) return -1;
  errno = 0;
  struct dirent* source = readdir((DIR*)directory);
  if (source == NULL) {
    *host_errno = errno;
    return errno == 0 ? 0 : -1;
  }
  entry->d_ino = source->d_ino;
  entry->d_type = source->d_type;
  entry->d_name_length = source->d_namlen;
  if (entry->d_name_length >= sizeof(entry->d_name)) {
    *host_errno = EIO;
    return -1;
  }
  for (size_t index = 0; index <= entry->d_name_length; ++index) {
    entry->d_name[index] = (uint8_t)source->d_name[index];
  }
  *host_errno = 0;
  return 1;
}

void aim_bionic_fs_host_rewinddir(void* directory) {
  if (directory != NULL) rewinddir((DIR*)directory);
}

int aim_bionic_fs_host_closedir(void* directory, int* host_errno) {
  if (directory == NULL || host_errno == NULL) return -1;
  errno = 0;
  const int result = closedir((DIR*)directory);
  *host_errno = result == 0 ? 0 : errno;
  return result;
}

int aim_bionic_fs_host_fpathconf(int fd, int semantic_name,
                                        int64_t* value, int* host_errno) {
  if (value == NULL || host_errno == NULL) return -1;
  int host_name;
  switch (semantic_name) {
    case 0: host_name = _PC_FILESIZEBITS; break;
    case 1: host_name = _PC_LINK_MAX; break;
    case 2: host_name = _PC_MAX_CANON; break;
    case 3: host_name = _PC_MAX_INPUT; break;
    case 4: host_name = _PC_NAME_MAX; break;
    case 5: host_name = _PC_PATH_MAX; break;
    case 6: host_name = _PC_PIPE_BUF; break;
    case 7: host_name = _PC_2_SYMLINKS; break;
    case 8: host_name = _PC_ALLOC_SIZE_MIN; break;
    case 9: host_name = _PC_REC_INCR_XFER_SIZE; break;
    case 10: host_name = _PC_REC_MAX_XFER_SIZE; break;
    case 11: host_name = _PC_REC_MIN_XFER_SIZE; break;
    case 12: host_name = _PC_REC_XFER_ALIGN; break;
    case 13: host_name = _PC_SYMLINK_MAX; break;
    case 14: host_name = _PC_CHOWN_RESTRICTED; break;
    case 15: host_name = _PC_NO_TRUNC; break;
    case 16: host_name = _PC_VDISABLE; break;
    case 17: host_name = _PC_ASYNC_IO; break;
    case 18: host_name = _PC_PRIO_IO; break;
    case 19: host_name = _PC_SYNC_IO; break;
    default:
      *host_errno = EINVAL;
      return -1;
  }
  errno = 0;
  const long result = fpathconf(fd, host_name);
  *value = (int64_t)result;
  *host_errno = errno;
  if (result != -1) return 1;
  return errno == 0 ? 0 : -1;
}

int aim_bionic_fs_host_fstatvfs(int fd,
                                       AimHostStatvfs* status,
                                       int* host_errno) {
  if (status == NULL || host_errno == NULL) return -1;
  struct statvfs host;
  errno = 0;
  if (fstatvfs(fd, &host) != 0) {
    *host_errno = errno;
    return -1;
  }
  status->f_bsize = host.f_bsize;
  status->f_frsize = host.f_frsize;
  status->f_blocks = host.f_blocks;
  status->f_bfree = host.f_bfree;
  status->f_bavail = host.f_bavail;
  status->f_files = host.f_files;
  status->f_ffree = host.f_ffree;
  status->f_favail = host.f_favail;
  status->f_fsid = host.f_fsid;
  status->f_flag = 0;
  if ((host.f_flag & ST_RDONLY) != 0) status->f_flag |= 0x0001;
  if ((host.f_flag & ST_NOSUID) != 0) status->f_flag |= 0x0002;
  status->f_namemax = host.f_namemax;
  *host_errno = 0;
  return 0;
}

static int NameCompare(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return (unsigned char)*left < (unsigned char)*right
             ? -1
             : ((unsigned char)*left != (unsigned char)*right);
}

typedef struct Binding {
  const char* name;
  AimBionicFsFunction address;
} Binding;

static const Binding kBindings[] = {
    {"__open_2", (AimBionicFsFunction)aim_bionic___open_2},
    {"__openat_2", (AimBionicFsFunction)aim_bionic___openat_2},
    {"__pread64_chk", (AimBionicFsFunction)aim_bionic___pread_chk},
    {"__pread_chk", (AimBionicFsFunction)aim_bionic___pread_chk},
    {"__pwrite64_chk", (AimBionicFsFunction)aim_bionic___pwrite_chk},
    {"__pwrite_chk", (AimBionicFsFunction)aim_bionic___pwrite_chk},
    {"__read_chk", (AimBionicFsFunction)aim_bionic___read_chk},
    {"__readlink_chk", (AimBionicFsFunction)aim_bionic___readlink_chk},
    {"__umask_chk", (AimBionicFsFunction)aim_bionic___umask_chk},
    {"__write_chk", (AimBionicFsFunction)aim_bionic___write_chk},
    {"access", (AimBionicFsFunction)aim_bionic_access},
    {"cfgetispeed", (AimBionicFsFunction)aim_bionic_cfget_speed},
    {"cfgetospeed", (AimBionicFsFunction)aim_bionic_cfget_speed},
    {"cfsetispeed", (AimBionicFsFunction)aim_bionic_cfset_speed},
    {"cfsetospeed", (AimBionicFsFunction)aim_bionic_cfset_speed},
    {"chdir", (AimBionicFsFunction)aim_bionic_chdir},
    {"chmod", (AimBionicFsFunction)aim_bionic_chmod},
    {"close", (AimBionicFsFunction)aim_bionic_close},
    {"closedir", (AimBionicFsFunction)aim_bionic_closedir},
    {"creat", (AimBionicFsFunction)aim_bionic_creat},
    {"dirfd", (AimBionicFsFunction)aim_bionic_dirfd},
    {"fchdir", (AimBionicFsFunction)aim_bionic_fchdir},
    {"fchmod", (AimBionicFsFunction)aim_bionic_fchmod},
    {"fchmodat", (AimBionicFsFunction)aim_bionic_fchmodat},
    {"fchown", (AimBionicFsFunction)aim_bionic_fchown},
    {"fdatasync", (AimBionicFsFunction)aim_bionic_fdatasync},
    {"fdopendir", (AimBionicFsFunction)aim_bionic_fdopendir},
    {"flock", (AimBionicFsFunction)aim_bionic_flock_unsupported},
    {"fstat", (AimBionicFsFunction)aim_bionic_fstat},
    {"fstat64", (AimBionicFsFunction)aim_bionic_fstat64},
    {"fstatat", (AimBionicFsFunction)aim_bionic_fstatat},
    {"fstatfs", (AimBionicFsFunction)aim_bionic_fstatfs_unsupported},
    {"fsync", (AimBionicFsFunction)aim_bionic_fsync},
    {"ftruncate", (AimBionicFsFunction)aim_bionic_ftruncate},
    {"ftruncate64", (AimBionicFsFunction)aim_bionic_ftruncate},
    {"futimens", (AimBionicFsFunction)aim_bionic_futimens},
    {"futimes", (AimBionicFsFunction)aim_bionic_fs_path_unsupported},
    {"getcwd", (AimBionicFsFunction)aim_bionic_getcwd},
    {"isatty", (AimBionicFsFunction)aim_bionic_isatty},
    {"link", (AimBionicFsFunction)aim_bionic_link},
    {"lseek", (AimBionicFsFunction)aim_bionic_lseek},
    {"lseek64", (AimBionicFsFunction)aim_bionic_lseek64},
    {"lstat", (AimBionicFsFunction)aim_bionic_lstat},
    {"lstat64", (AimBionicFsFunction)aim_bionic_lstat64},
    {"mkdir", (AimBionicFsFunction)aim_bionic_mkdir},
    {"mkdirat", (AimBionicFsFunction)aim_bionic_mkdirat},
    {"mkdtemp", (AimBionicFsFunction)aim_bionic_mkdtemp},
    {"mkstemp", (AimBionicFsFunction)aim_bionic_mkstemp},
    {"mkstemp64", (AimBionicFsFunction)aim_bionic_mkstemp64},
    {"open", (AimBionicFsFunction)aim_bionic_open},
    {"openat", (AimBionicFsFunction)aim_bionic_openat},
    {"opendir", (AimBionicFsFunction)aim_bionic_opendir},
    {"pathconf", (AimBionicFsFunction)aim_bionic_pathconf},
    {"posix_fadvise", (AimBionicFsFunction)aim_bionic_posix_fadvise},
    {"posix_fallocate", (AimBionicFsFunction)aim_bionic_posix_fallocate},
    {"pread", (AimBionicFsFunction)aim_bionic_pread},
    {"pwrite", (AimBionicFsFunction)aim_bionic_pwrite},
    {"read", (AimBionicFsFunction)aim_bionic_read},
    {"readv", (AimBionicFsFunction)aim_bionic_readv},
    {"readdir", (AimBionicFsFunction)aim_bionic_readdir},
    {"readdir_r", (AimBionicFsFunction)aim_bionic_readdir_r},
    {"readlink", (AimBionicFsFunction)aim_bionic_readlink},
    {"realpath", (AimBionicFsFunction)aim_bionic_realpath},
    {"remove", (AimBionicFsFunction)aim_bionic_remove},
    {"rename", (AimBionicFsFunction)aim_bionic_rename},
    {"rewinddir", (AimBionicFsFunction)aim_bionic_rewinddir},
    {"rmdir", (AimBionicFsFunction)aim_bionic_rmdir},
    {"stat", (AimBionicFsFunction)aim_bionic_stat},
    {"statfs", (AimBionicFsFunction)aim_bionic_fs_path_unsupported},
    {"statvfs", (AimBionicFsFunction)aim_bionic_statvfs},
    {"symlink", (AimBionicFsFunction)aim_bionic_symlink},
    {"tcdrain", (AimBionicFsFunction)aim_bionic_terminal_unsupported},
    {"tcflush", (AimBionicFsFunction)aim_bionic_terminal_unsupported},
    {"tcgetattr", (AimBionicFsFunction)aim_bionic_terminal_unsupported},
    {"tcsetattr", (AimBionicFsFunction)aim_bionic_terminal_unsupported},
    {"truncate", (AimBionicFsFunction)aim_bionic_truncate},
    {"unlink", (AimBionicFsFunction)aim_bionic_unlink},
    {"unlinkat", (AimBionicFsFunction)aim_bionic_unlinkat},
    {"utimensat", (AimBionicFsFunction)aim_bionic_utimensat},
    {"utimes", (AimBionicFsFunction)aim_bionic_fs_path_unsupported},
    {"write", (AimBionicFsFunction)aim_bionic_write},
    {"writev", (AimBionicFsFunction)aim_bionic_writev},
};

AimBionicFsFunction aim_bionic_fs_resolve(const char* import_name) {
  if (import_name == NULL) return NULL;
  size_t low = 0;
  size_t high = sizeof(kBindings) / sizeof(kBindings[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    const int order = NameCompare(import_name, kBindings[middle].name);
    if (order == 0) return kBindings[middle].address;
    if (order < 0)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}
