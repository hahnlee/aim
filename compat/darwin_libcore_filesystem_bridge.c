#include "darwin_libcore_filesystem_bridge.h"

#include "aim_bionic_errno.h"
#include "aim_bionic_fs.h"

#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdio.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>

static AimLibcoreStatProvider g_stat_provider;
static AimLibcoreModeProvider g_mkdir_provider;
static AimLibcoreModeProvider g_chmod_provider;
static AimLibcoreErrnoProvider g_errno_provider;
static unsigned g_debug_stat_count;

typedef struct AimLibcoreDirectoryRecord {
  DIR* directory;
  int host_directory;
  struct AimLibcoreDirectoryRecord* next;
} AimLibcoreDirectoryRecord;

static AimLibcoreDirectoryRecord* g_directory_records;
static pthread_mutex_t g_directory_records_mutex = PTHREAD_MUTEX_INITIALIZER;

extern int aim_bionic_fs_statvfs_core(const char*, AimAndroidStatvfs*)
    __attribute__((weak_import));
extern void* aim_bionic_opendir(const char*) __attribute__((weak_import));
extern AimAndroidDirent* aim_bionic_readdir(void*)
    __attribute__((weak_import));
extern int aim_bionic_closedir(void*) __attribute__((weak_import));
extern int aim_bionic_open(const char*, int, uint32_t)
    __attribute__((weak_import));
extern int aim_bionic_close(int) __attribute__((weak_import));
extern char* aim_bionic_realpath(const char*, char*)
    __attribute__((weak_import));
extern int aim_bionic_fstat(int, AimAndroidStat*)
    __attribute__((weak_import));
extern intptr_t aim_bionic_fs_resolve_private_host_path(
    const char*, char*, size_t) __attribute__((weak_import));
extern int32_t aim_bionic_errno_load(void) __attribute__((weak_import));
extern void aim_bionic_errno_store(int32_t) __attribute__((weak_import));

static int path_is_within(const char* path, const char* root) {
  if (path == NULL || root == NULL || root[0] == '\0') return 0;
  const size_t root_length = strlen(root);
  return strncmp(path, root, root_length) == 0 &&
         (path[root_length] == '\0' || path[root_length] == '/');
}

static int listed_host_file(const char* path) {
  const char* cursor = getenv("AIM_RUNTIME_HOST_FILES");
  if (path == NULL || cursor == NULL) return 0;
  const size_t path_length = strlen(path);
  while (*cursor != '\0') {
    const char* separator = strchr(cursor, ':');
    const size_t length = separator == NULL ? strlen(cursor)
                                           : (size_t)(separator - cursor);
    if (length == path_length && memcmp(cursor, path, length) == 0) return 1;
    if (separator == NULL) break;
    cursor = separator + 1;
  }
  return 0;
}

static int explicit_host_file(const char* path, const char* variable) {
  const char* value = getenv(variable);
  return path != NULL && value != NULL && strcmp(path, value) == 0;
}

static int authorized_directory_within(const char* path, const char* root) {
  char resolved_path[PATH_MAX];
  char resolved_root[PATH_MAX];
  if (path == NULL || root == NULL || root[0] != '/') return 0;
  // A lexical prefix alone would allow root/../outside or a symlink escape.
  return realpath(path, resolved_path) != NULL &&
         realpath(root, resolved_root) != NULL &&
         path_is_within(resolved_path, resolved_root);
}

// Host-side NIO operations may create the final component (Files.copy does
// this for its destination), so realpath(path) cannot authorize the path yet.
// Resolve only its existing parent and retain the same per-process capability
// boundary used by the rest of the filesystem bridge.
static int authorized_private_host_path(const char* path) {
  const char* root = getenv("AIM_ANDROID_PRIVATE_DATA_ROOT");
  if (path == NULL || root == NULL || root[0] != '/' ||
      !path_is_within(path, root)) {
    return 0;
  }
  char resolved_root[PATH_MAX];
  if (realpath(root, resolved_root) == NULL) return 0;
  char parent[PATH_MAX];
  const size_t length = strlen(path);
  if (length == 0 || length >= sizeof(parent)) return 0;
  memcpy(parent, path, length + 1);
  char* slash = strrchr(parent, '/');
  if (slash == NULL || slash == parent) return 0;
  *slash = '\0';
  char resolved_parent[PATH_MAX];
  return realpath(parent, resolved_parent) != NULL &&
         path_is_within(resolved_parent, resolved_root);
}

// Keep this allowlist in lockstep with libcore_darwin_linux_syscalls.cc's
// IsAuthorizedHostRuntimePath. Only immutable runtime roots and exact files
// may use Darwin DIR handles; every other path remains a guest capability.
static int authorized_host_runtime_path(const char* path) {
  // Guest namespaces cannot become host capabilities through a test setting
  // or an artifact list. In particular /system fonts must be resolved by the
  // same VFS owner as FileInputStream, not stat() against the macOS root.
  static const char* guest_roots[] = {
      "/system", "/system_ext", "/vendor", "/product", "/apex", "/data",
      "/storage", "/sdcard", "/proc", "/dev"};
  for (size_t i = 0; i < sizeof(guest_roots) / sizeof(guest_roots[0]); ++i) {
    if (path_is_within(path, guest_roots[i])) return 0;
  }
  return listed_host_file(path) ||
         authorized_directory_within(path, getenv("ANDROID_I18N_ROOT")) ||
         authorized_directory_within(path, getenv("ANDROID_DATA")) ||
         authorized_directory_within(path, getenv("ANDROID_TZDATA_ROOT")) ||
         authorized_directory_within(path, getenv("AIM_APK_APP_NATIVE_DIR")) ||
         authorized_directory_within(path, getenv("AIM_ANDROID_PRIVATE_DATA_ROOT")) ||
         explicit_host_file(path, "AIM_APK_APP_RESOURCE_APK") ||
         explicit_host_file(path, "AIM_APK_APP_SUPPORT_DEX") ||
         explicit_host_file(path, "AIM_FRAMEWORK_RES_APK") ||
         explicit_host_file(path, "AIM_HOST_FONTS_XML") ||
         explicit_host_file(path, "AIM_HOST_FONT");
}

static int remember_directory(DIR* directory, int host_directory) {
  AimLibcoreDirectoryRecord* record =
      (AimLibcoreDirectoryRecord*)malloc(sizeof(*record));
  if (record == NULL) return 0;
  record->directory = directory;
  record->host_directory = host_directory;
  pthread_mutex_lock(&g_directory_records_mutex);
  record->next = g_directory_records;
  g_directory_records = record;
  pthread_mutex_unlock(&g_directory_records_mutex);
  return 1;
}

static int find_directory(DIR* directory, int* host_directory) {
  int found = 0;
  pthread_mutex_lock(&g_directory_records_mutex);
  for (AimLibcoreDirectoryRecord* record = g_directory_records;
       record != NULL; record = record->next) {
    if (record->directory == directory) {
      if (host_directory != NULL) *host_directory = record->host_directory;
      found = 1;
      break;
    }
  }
  pthread_mutex_unlock(&g_directory_records_mutex);
  return found;
}

static int forget_directory(DIR* directory, int* host_directory) {
  int found = 0;
  pthread_mutex_lock(&g_directory_records_mutex);
  AimLibcoreDirectoryRecord** cursor = &g_directory_records;
  while (*cursor != NULL) {
    AimLibcoreDirectoryRecord* record = *cursor;
    if (record->directory == directory) {
      *cursor = record->next;
      if (host_directory != NULL) *host_directory = record->host_directory;
      free(record);
      found = 1;
      break;
    }
    cursor = &record->next;
  }
  pthread_mutex_unlock(&g_directory_records_mutex);
  return found;
}

void aim_libcore_install_filesystem_provider(
    AimLibcoreStatProvider stat_provider,
    AimLibcoreModeProvider mkdir_provider,
    AimLibcoreModeProvider chmod_provider,
    AimLibcoreErrnoProvider errno_provider) {
  g_stat_provider = stat_provider;
  g_mkdir_provider = mkdir_provider;
  g_chmod_provider = chmod_provider;
  g_errno_provider = errno_provider;
  if (getenv("AIM_DEBUG_FS_BRIDGE") != NULL) {
    fprintf(stderr, "DARWIN libcore FS: provider installed stat=%p\n",
            (void*)stat_provider);
  }
}

static void publish_android_errno(void) {
  if (g_errno_provider != NULL) errno = g_errno_provider();
}

// UnixNativeDispatcher is compiled from the host OpenJDK sources, so its
// open(2) flags use Darwin's numeric ABI.  The bionic facade below consumes
// Android's Linux flag values.  Keep this conversion at the JNI filesystem
// boundary; passing Darwin O_CREAT (0x200) as Android O_TRUNC is what makes a
// missing private destination look like ENOTDIR to java.nio Files.copy.
static int android_open_flags(int flags) {
  int translated = flags & O_ACCMODE;
#define MAP_OPEN_FLAG(host_flag, android_flag) \
  do {                                      \
    if ((flags & (host_flag)) != 0) translated |= (android_flag); \
  } while (0)
  MAP_OPEN_FLAG(O_CREAT, 64);
  MAP_OPEN_FLAG(O_EXCL, 128);
  MAP_OPEN_FLAG(O_TRUNC, 512);
  MAP_OPEN_FLAG(O_APPEND, 1024);
  MAP_OPEN_FLAG(O_NONBLOCK, 2048);
  MAP_OPEN_FLAG(O_DSYNC, 4096);
  MAP_OPEN_FLAG(O_SYNC, 1052672);
  MAP_OPEN_FLAG(O_DIRECTORY, 16384);
  MAP_OPEN_FLAG(O_NOFOLLOW, 32768);
#ifdef O_LARGEFILE
  MAP_OPEN_FLAG(O_LARGEFILE, 131072);
#endif
#ifdef O_CLOEXEC
  MAP_OPEN_FLAG(O_CLOEXEC, 524288);
#endif
#undef MAP_OPEN_FLAG
  return translated;
}

char* aim_libcore_realpath(const char* path, char* resolved) {
  if (g_stat_provider == NULL) return realpath(path, resolved);
  if (aim_bionic_realpath == NULL) {
    errno = ENOSYS;
    return NULL;
  }
  char* result = aim_bionic_realpath(path, resolved);
  if (result == NULL) publish_android_errno();
  return result;
}

static void copy_android_stat(const AimAndroidStat* source,
                              struct stat* destination) {
  memset(destination, 0, sizeof(*destination));
  destination->st_dev = (dev_t)source->st_dev;
  destination->st_ino = (ino_t)source->st_ino;
  destination->st_mode = (mode_t)source->st_mode;
  destination->st_nlink = (nlink_t)source->st_nlink;
  destination->st_uid = (uid_t)source->st_uid;
  destination->st_gid = (gid_t)source->st_gid;
  destination->st_rdev = (dev_t)source->st_rdev;
  destination->st_size = (off_t)source->st_size;
  destination->st_blksize = (blksize_t)source->st_blksize;
  destination->st_blocks = (blkcnt_t)source->st_blocks;
  destination->st_atimespec.tv_sec = (time_t)source->st_atim.tv_sec;
  destination->st_atimespec.tv_nsec = source->st_atim.tv_nsec;
  destination->st_mtimespec.tv_sec = (time_t)source->st_mtim.tv_sec;
  destination->st_mtimespec.tv_nsec = source->st_mtim.tv_nsec;
  destination->st_ctimespec.tv_sec = (time_t)source->st_ctim.tv_sec;
  destination->st_ctimespec.tv_nsec = source->st_ctim.tv_nsec;
  destination->st_birthtimespec = destination->st_ctimespec;
}

int aim_libcore_open(const char* path, int flags, ...) {
  mode_t mode = 0;
  if ((flags & O_CREAT) != 0) {
    va_list arguments;
    va_start(arguments, flags);
    mode = (mode_t)va_arg(arguments, int);
    va_end(arguments);
  }
  if (g_stat_provider == NULL) return open(path, flags, mode);
  // UnixFileSystem.createFileExclusively0 opens only long enough to atomically
  // create and close the file. Resolve writable guest /data through the
  // process capability and return a real descriptor, because the unmodified
  // OpenJDK helper immediately applies host fstat()/close() to this private FD.
  if (aim_bionic_fs_resolve_private_host_path != NULL) {
    char private_path[PATH_MAX];
    if (aim_bionic_fs_resolve_private_host_path(
            path, private_path, sizeof(private_path)) >= 0) {
      return open(private_path, flags, mode);
    }
  }
  // Files.copy and related java.nio operations use the host OpenJDK NIO
  // contract and pass Darwin flags.  Keep those operations on a real host FD
  // when the path is inside this process's private staging capability; sending
  // the host absolute path to the Android bionic facade would interpret it as
  // a guest pathname and can turn O_CREAT into ENOTDIR.
  if (authorized_private_host_path(path)) {
    return open(path, flags, mode);
  }
  if (aim_bionic_open == NULL) {
    errno = ENOSYS;
    return -1;
  }
  const int translated_flags = android_open_flags(flags);
  const int result = aim_bionic_open(
      path, translated_flags, (uint32_t)mode);
  if (getenv("AIM_DEBUG_FS_BRIDGE") != NULL &&
      path != NULL && strstr(path, "arttest") != NULL) {
    fprintf(stderr, "DARWIN libcore FS: open path=%s host_flags=%#x "
                    "android_flags=%#x result=%d errno=%d\n",
            path, flags, translated_flags, result, errno);
  }
  if (result < 0) publish_android_errno();
  return result;
}

int aim_libcore_close(int fd) {
  if (g_stat_provider == NULL) return close(fd);
  if (aim_bionic_close == NULL) {
    errno = ENOSYS;
    return -1;
  }
  const int result = aim_bionic_close(fd);
  if (result != 0) publish_android_errno();
  return result;
}

int aim_libcore_fstat(int fd, struct stat* status) {
  if (g_stat_provider == NULL) return fstat(fd, status);
  if (aim_bionic_fstat == NULL) {
    errno = ENOSYS;
    return -1;
  }
  AimAndroidStat android_status;
  const int result = aim_bionic_fstat(fd, &android_status);
  if (result != 0) {
    publish_android_errno();
    return result;
  }
  copy_android_stat(&android_status, status);
  return 0;
}

int aim_libcore_statfs(const char* path, struct statfs* status) {
  // Standalone foundation tests use the host filesystem. Managed Android
  // processes install the guest provider before registering UnixFileSystem.
  if (g_stat_provider == NULL ||
      path_is_within(path, getenv("AIM_ANDROID_PRIVATE_DATA_ROOT"))) {
    return statfs(path, status);
  }
  if (aim_bionic_fs_statvfs_core == NULL) {
    errno = ENOSYS;
    return -1;
  }
  AimAndroidStatvfs android_status;
  if (aim_bionic_fs_statvfs_core(path, &android_status) != 0) {
    publish_android_errno();
    return -1;
  }
  memset(status, 0, sizeof(*status));
  // statvfs block counts use f_frsize; Java's Darwin branch multiplies by
  // statfs.f_bsize, so retain that unit rather than the preferred I/O size.
  status->f_bsize = (uint32_t)android_status.f_frsize;
  status->f_blocks = android_status.f_blocks;
  status->f_bfree = android_status.f_bfree;
  status->f_bavail = android_status.f_bavail;
  status->f_files = android_status.f_files;
  status->f_ffree = android_status.f_ffree;
  return 0;
}

DIR* aim_libcore_opendir(const char* path) {
  const int host_directory =
      g_stat_provider == NULL || authorized_host_runtime_path(path);
  if (host_directory) {
    DIR* const directory = opendir(path);
    if (directory == NULL) return NULL;
    if (!remember_directory(directory, 1)) {
      const int saved_errno = ENOMEM;
      (void)closedir(directory);
      errno = saved_errno;
      return NULL;
    }
    errno = 0;
    return directory;
  }
  if (aim_bionic_opendir == NULL ||
      aim_bionic_errno_load == NULL ||
      aim_bionic_closedir == NULL) {
    errno = ENOSYS;
    return NULL;
  }
  // UnixFileSystem_md.c is compiled as host C.  Its list0() must enumerate
  // the same authorized guest path as the rest of libcore rather than asking
  // Darwin's host filesystem to open /data/... .
  void* const directory = aim_bionic_opendir(path);
  if (directory == NULL) {
    publish_android_errno();
  } else {
    // POSIX leaves errno unchanged at readdir EOF.  A successful opendir must
    // establish the clean value expected by list0's first readdir call.
    if (!remember_directory((DIR*)directory, 0)) {
      const int saved_errno = ENOMEM;
      (void)aim_bionic_closedir(directory);
      errno = saved_errno;
      return NULL;
    }
    errno = 0;
  }
  return (DIR*)directory;
}

struct dirent* aim_libcore_readdir(DIR* directory) {
  int host_directory = 0;
  if (!find_directory(directory, &host_directory)) {
    errno = EBADF;
    return NULL;
  }
  if (host_directory) return readdir(directory);
  if (g_stat_provider == NULL) return readdir(directory);
  if (aim_bionic_readdir == NULL ||
      aim_bionic_errno_store == NULL ||
      aim_bionic_errno_load == NULL) {
    errno = ENOSYS;
    return NULL;
  }
  // The facade intentionally does not publish an Android errno for ordinary
  // EOF. Clear its TLS cell before each call so a prior unrelated operation
  // cannot turn EOF into a spurious list0 failure.
  aim_bionic_errno_store(0);
  errno = 0;
  AimAndroidDirent* const android_entry =
      aim_bionic_readdir((void*)directory);
  if (android_entry == NULL) {
    publish_android_errno();
    return NULL;
  }

  static _Thread_local struct dirent entry;
  memset(&entry, 0, sizeof(entry));
  entry.d_ino = (ino_t)android_entry->d_ino;
  entry.d_type = android_entry->d_type;
  const size_t length = strnlen(android_entry->d_name,
                                sizeof(android_entry->d_name));
  const size_t copied = length < sizeof(entry.d_name) - 1
                            ? length
                            : sizeof(entry.d_name) - 1;
  memcpy(entry.d_name, android_entry->d_name, copied);
  entry.d_name[copied] = '\0';
  entry.d_namlen = (uint16_t)copied;
  errno = 0;
  return &entry;
}

int aim_libcore_closedir(DIR* directory) {
  int host_directory = 0;
  if (!forget_directory(directory, &host_directory)) {
    errno = EBADF;
    return -1;
  }
  if (host_directory) return closedir(directory);
  if (g_stat_provider == NULL) return closedir(directory);
  if (aim_bionic_closedir == NULL ||
      aim_bionic_errno_load == NULL) {
    errno = ENOSYS;
    return -1;
  }
  const int result = aim_bionic_closedir((void*)directory);
  if (result != 0) publish_android_errno();
  return result;
}

int aim_libcore_stat(const char* path, struct stat* status) {
  if (g_stat_provider == NULL) return stat(path, status);
  // DexPathList checks File.isFile() before asking ART to open a native SDK's
  // extracted helper JAR. The JNI boundary has already resolved the guest
  // /data pathname into this one app-private backing root, so keep that exact
  // capability on the host stat path instead of feeding it back into the
  // guest VFS a second time.
  const char* private_root = getenv("AIM_ANDROID_PRIVATE_DATA_ROOT");
  if (path_is_within(path, private_root) ||
      authorized_host_runtime_path(path)) {
    const int result = stat(path, status);
    if (getenv("AIM_DEBUG_FS_BRIDGE") != NULL) {
      fprintf(stderr,
              "DARWIN libcore FS: authorized-host stat path=%s result=%d "
              "errno=%d\n",
              path, result, errno);
    }
    return result;
  }
  AimAndroidStat android_status;
  const int provider_result = g_stat_provider(path, &android_status);
  if (getenv("AIM_DEBUG_FS_BRIDGE") != NULL &&
      (g_debug_stat_count++ < 64 ||
       (path != NULL && strstr(path, "local") != NULL))) {
    fprintf(stderr, "DARWIN libcore FS: stat path=%s result=%d errno=%d\n",
            path, provider_result,
            g_errno_provider == NULL ? -1 : g_errno_provider());
  }
  if (provider_result != 0) {
    publish_android_errno();
    return -1;
  }
  copy_android_stat(&android_status, status);
  return 0;
}

int aim_libcore_mkdir(const char* path, mode_t mode) {
  if (g_mkdir_provider == NULL) return mkdir(path, mode);
  const int result = g_mkdir_provider(path, (uint32_t)mode);
  if (result != 0) publish_android_errno();
  return result;
}

int aim_libcore_chmod(const char* path, mode_t mode) {
  if (g_chmod_provider == NULL) return chmod(path, mode);
  const int result = g_chmod_provider(path, (uint32_t)mode);
  if (result != 0) publish_android_errno();
  return result;
}
