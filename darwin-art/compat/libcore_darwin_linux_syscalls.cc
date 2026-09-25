#include "libcore_darwin_linux.h"

#include "darwin_art_bionic_fs.h"
#include "darwin_os_constants.h"

#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

#include <cerrno>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <map>
#include <mutex>
#include <string>
#include <unordered_set>

namespace darwin_art::libcore_darwin {
namespace {

LinuxSyscallProviders g_providers{};
std::mutex g_host_fd_mutex;
std::unordered_set<int> g_host_fds;

constexpr int kLinuxMapShared = 0x01;
constexpr int kLinuxMapPrivate = 0x02;
constexpr int kLinuxMapFixed = 0x10;
constexpr int kLinuxMapAnonymous = 0x20;
constexpr int kAndroidSeekSet = 0;
constexpr int kAndroidSeekCur = 1;
constexpr int kAndroidSeekEnd = 2;

bool IsPathWithin(const char* path, const char* root) {
  if (path == nullptr || root == nullptr || *root == '\0') return false;
  const size_t root_length = std::strlen(root);
  return std::strncmp(path, root, root_length) == 0 &&
         (path[root_length] == '\0' || path[root_length] == '/');
}

bool IsExplicitHostFile(const char* path, const char* environment_name) {
  const char* file = std::getenv(environment_name);
  return file != nullptr && path != nullptr && std::strcmp(path, file) == 0;
}

bool IsListedHostFile(const char* path) {
  const char* list = std::getenv("DARWIN_ART_RUNTIME_HOST_FILES");
  if (path == nullptr || list == nullptr) return false;
  const size_t path_length = std::strlen(path);
  const char* entry = list;
  while (*entry != '\0') {
    const char* end = std::strchr(entry, ':');
    const size_t length = end == nullptr ? std::strlen(entry)
                                         : static_cast<size_t>(end - entry);
    if (length == path_length && std::strncmp(path, entry, length) == 0) {
      return true;
    }
    if (end == nullptr) break;
    entry = end + 1;
  }
  return false;
}

void RememberHostFd(int fd) {
  if (fd < 0) return;
  std::lock_guard<std::mutex> lock(g_host_fd_mutex);
  g_host_fds.insert(fd);
}

bool IsHostFd(int fd) {
  std::lock_guard<std::mutex> lock(g_host_fd_mutex);
  return g_host_fds.find(fd) != g_host_fds.end();
}

bool ForgetHostFd(int fd) {
  std::lock_guard<std::mutex> lock(g_host_fd_mutex);
  return g_host_fds.erase(fd) != 0;
}

bool DarwinWhenceFromAndroid(int android_whence, int* darwin_whence) {
  if (darwin_whence == nullptr) {
    return false;
  }
  switch (android_whence) {
    case kAndroidSeekSet:
      *darwin_whence = SEEK_SET;
      return true;
    case kAndroidSeekCur:
      *darwin_whence = SEEK_CUR;
      return true;
    case kAndroidSeekEnd:
      *darwin_whence = SEEK_END;
      return true;
    default:
      return false;
  }
}

// errno in this file is a Darwin value (host calls set it, and ThrowErrno
// translates Darwin to Android). Providers report Android errno values, so
// publish the Darwin value that ThrowErrno maps back to the same Android one.
void PublishAndroidErrno() {
  const int android_error =
      g_providers.load_errno == nullptr ? 0 : g_providers.load_errno();
  for (int darwin_error = 1; darwin_error < 256; ++darwin_error) {
    int mapped = 0;
    if (os_constants::AndroidErrnoFromDarwin(darwin_error, &mapped) &&
        mapped == android_error) {
      errno = darwin_error;
      return;
    }
  }
  errno = EIO;
}

int AdoptHostFd(int host_fd) {
  RememberHostFd(host_fd);
  return host_fd;
}

void AndroidStatToDarwin(const DarwinArtAndroidStat& source,
                         struct stat* destination) {
  std::memset(destination, 0, sizeof(*destination));
  destination->st_dev = static_cast<dev_t>(source.st_dev);
  destination->st_ino = static_cast<ino_t>(source.st_ino);
  destination->st_mode = static_cast<mode_t>(source.st_mode);
  destination->st_nlink = static_cast<nlink_t>(source.st_nlink);
  destination->st_uid = static_cast<uid_t>(source.st_uid);
  destination->st_gid = static_cast<gid_t>(source.st_gid);
  destination->st_rdev = static_cast<dev_t>(source.st_rdev);
  destination->st_size = static_cast<off_t>(source.st_size);
  destination->st_blksize = static_cast<blksize_t>(source.st_blksize);
  destination->st_blocks = static_cast<blkcnt_t>(source.st_blocks);
  destination->st_atimespec.tv_sec = static_cast<time_t>(source.st_atim.tv_sec);
  destination->st_atimespec.tv_nsec = source.st_atim.tv_nsec;
  destination->st_mtimespec.tv_sec = static_cast<time_t>(source.st_mtim.tv_sec);
  destination->st_mtimespec.tv_nsec = source.st_mtim.tv_nsec;
  destination->st_ctimespec.tv_sec = static_cast<time_t>(source.st_ctim.tv_sec);
  destination->st_ctimespec.tv_nsec = source.st_ctim.tv_nsec;
}

int TranslateMmapFlags(int linux_flags) {
  // MAP_POPULATE is a best-effort prefault/read-ahead hint, not a different
  // mapping type. AOSP's APK digest reader requests it for ordinary files.
  int remaining = linux_flags & ~0x8000;
  int darwin_flags = 0;
  if ((remaining & kLinuxMapShared) != 0) {
    darwin_flags |= MAP_SHARED;
    remaining &= ~kLinuxMapShared;
  }
  if ((remaining & kLinuxMapPrivate) != 0) {
    darwin_flags |= MAP_PRIVATE;
    remaining &= ~kLinuxMapPrivate;
  }
  if ((remaining & kLinuxMapFixed) != 0) {
    darwin_flags |= MAP_FIXED;
    remaining &= ~kLinuxMapFixed;
  }
  if ((remaining & kLinuxMapAnonymous) != 0) {
    darwin_flags |= MAP_ANON;
    remaining &= ~kLinuxMapAnonymous;
  }
  if (remaining != 0 ||
      ((darwin_flags & MAP_SHARED) != 0 &&
       (darwin_flags & MAP_PRIVATE) != 0)) {
    errno = ENOTSUP;
    return -1;
  }
  return darwin_flags;
}

}  // namespace

void InstallLinuxSyscallProviders(uint32_t abi_version, size_t provider_size,
                                  const LinuxSyscallProviders& providers) {
  if (abi_version != kLinuxSyscallProviderAbiVersion ||
      provider_size != sizeof(LinuxSyscallProviders)) {
    std::fprintf(stderr,
                 "DARWIN libcore: syscall provider ABI mismatch version=%u "
                 "size=%zu expected_version=%u expected_size=%zu\n",
                 abi_version, provider_size, kLinuxSyscallProviderAbiVersion,
                 sizeof(LinuxSyscallProviders));
    std::abort();
  }
  g_providers = providers;
}

bool IsAuthorizedHostRuntimePath(const char* path) {
  return IsListedHostFile(path) ||
         IsPathWithin(path, std::getenv("ANDROID_I18N_ROOT")) ||
         IsPathWithin(path, std::getenv("ANDROID_DATA")) ||
         IsPathWithin(path, std::getenv("ANDROID_TZDATA_ROOT")) ||
         IsPathWithin(path, std::getenv("DARWIN_ART_APK_APP_NATIVE_DIR")) ||
         // ART consumes the resolved backing path when a native SDK creates a
         // DexClassLoader from its private Android /data cache. This root is
         // one app-scoped capability, not arbitrary host filesystem access.
         IsPathWithin(path,
                      std::getenv("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT")) ||
         IsExplicitHostFile(path, "DARWIN_ART_APK_APP_RESOURCE_APK") ||
         IsExplicitHostFile(path, "DARWIN_ART_APK_APP_SUPPORT_DEX") ||
         IsExplicitHostFile(path, "DARWIN_ART_FRAMEWORK_RES_APK") ||
         IsExplicitHostFile(path, "DARWIN_ART_HOST_FONTS_XML") ||
         IsExplicitHostFile(path, "DARWIN_ART_HOST_FONT");
}

int Open(const char* path, int linux_flags, mode_t mode) {
  // Boot JARs live at their Android paths in the immutable system image.
  // Keep NIO and libcore on that same filesystem, without a private BCP alias.
  const char* effective_path = path;
  const char* system_root = std::getenv("DARWIN_ART_ANDROID_SYSTEM_ROOT");
  const bool use_sealed_system_file =
      system_root != nullptr && effective_path != nullptr &&
      std::strncmp(effective_path, "/system/", 8) == 0;
  if (g_providers.open != nullptr &&
      !IsAuthorizedHostRuntimePath(effective_path) &&
      !use_sealed_system_file) {
    const int result = g_providers.open(
        path, linux_flags, static_cast<uint32_t>(mode));
    if (result == -1) PublishAndroidErrno();
    if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr) {
      std::fprintf(stderr, "DARWIN libcore IO: guest open path=%s fd=%d errno=%d\n",
                   path == nullptr ? "<null>" : path, result, errno);
    }
    return result;
  }
  int flags = 0;
  if (!os_constants::DarwinOpenFlagsFromAndroid(linux_flags, &flags)) {
    errno = ENOTSUP;
    return -1;
  }
  // /system is an immutable, capability-scoped host directory. Keeping its
  // descriptor native lets OpenJDK FileChannel and Android font mmap share the
  // same file without exposing arbitrary host paths.
  if (use_sealed_system_file) {
    const char* relative = effective_path + 8;
    if (*relative == '\0' || std::strstr(relative, "/../") != nullptr ||
        std::strncmp(relative, "../", 3) == 0 ||
        (std::strlen(relative) >= 3 &&
         std::strcmp(relative + std::strlen(relative) - 3, "/..") == 0) ||
        std::strcmp(relative, ".") == 0 || std::strstr(relative, "/./") != nullptr ||
        std::strncmp(relative, "./", 2) == 0 ||
        (std::strlen(relative) >= 2 &&
         std::strcmp(relative + std::strlen(relative) - 2, "/.") == 0)) {
      errno = EINVAL;
      return -1;
    }
    std::string redirected(system_root);
    redirected.push_back('/');
    redirected.append(relative);
    int result;
    do {
      result = open(redirected.c_str(), flags, mode);
    } while (result == -1 && errno == EINTR);
    return AdoptHostFd(result);
  }
  int result;
  do {
    result = open(effective_path, flags, mode);
  } while (result == -1 && errno == EINTR);
  return AdoptHostFd(result);
}

int Dup(int fd) {
  if (g_providers.dup != nullptr && !IsHostFd(fd)) {
    const int result = g_providers.dup(fd);
    if (result == -1) PublishAndroidErrno();
    if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr) {
      std::fprintf(stderr,
                   "DARWIN libcore IO: guest dup fd=%d result=%d errno=%d\n",
                   fd, result, errno);
    }
    return result;
  }
  int result;
  do {
    result = dup(fd);
  } while (result == -1 && errno == EINTR);
  if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN libcore IO: host dup fd=%d result=%d errno=%d\n", fd,
                 result, errno);
  }
  return AdoptHostFd(result);
}

int Fcntl(int fd, int android_command, intptr_t argument) {
  if (g_providers.fcntl != nullptr && !IsHostFd(fd)) {
    const int result = g_providers.fcntl(fd, android_command, argument);
    if (result == -1) PublishAndroidErrno();
    if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr) {
      std::fprintf(stderr,
                   "DARWIN libcore IO: guest fcntl fd=%d command=%d "
                   "argument=%lld result=%d errno=%d\n",
                   fd, android_command, static_cast<long long>(argument),
                   result, errno);
    }
    return result;
  }

  constexpr int kAndroidFDupfd = 0;
  constexpr int kAndroidFGetfd = 1;
  constexpr int kAndroidFSetfd = 2;
  constexpr int kAndroidFGetfl = 3;
  constexpr int kAndroidFSetfl = 4;
  constexpr int kAndroidFDupfdCloexec = 1030;
  int darwin_command = -1;
  switch (android_command) {
    case kAndroidFDupfd: darwin_command = F_DUPFD; break;
    case kAndroidFDupfdCloexec: darwin_command = F_DUPFD_CLOEXEC; break;
    case kAndroidFGetfd: darwin_command = F_GETFD; break;
    case kAndroidFSetfd: darwin_command = F_SETFD; break;
    case kAndroidFGetfl: darwin_command = F_GETFL; break;
    case kAndroidFSetfl: darwin_command = F_SETFL; break;
    default:
      errno = ENOTSUP;
      return -1;
  }
  int result;
  do {
    result = (android_command == kAndroidFGetfd ||
              android_command == kAndroidFGetfl)
                 ? fcntl(fd, darwin_command)
                 : fcntl(fd, darwin_command, argument);
  } while (result == -1 && errno == EINTR);
  if (result >= 0 && (android_command == kAndroidFDupfd ||
                      android_command == kAndroidFDupfdCloexec)) {
    RememberHostFd(result);
  }
  return result;
}

int Close(int fd) {
  if (g_providers.close != nullptr && !ForgetHostFd(fd)) {
    const int result = g_providers.close(fd);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  return close(fd);
}

ssize_t Read(int fd, void* bytes, size_t byte_count) {
  if (g_providers.read != nullptr && !IsHostFd(fd)) {
    const intptr_t result = g_providers.read(fd, bytes, byte_count);
    if (result == -1) PublishAndroidErrno();
    if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr && result == -1) {
      std::fprintf(stderr, "DARWIN libcore IO: read fd=%d count=%zu result=%ld errno=%d\n",
                   fd, byte_count, static_cast<long>(result), errno);
    }
    return static_cast<ssize_t>(result);
  }
  ssize_t result;
  do {
    result = read(fd, bytes, byte_count);
  } while (result == -1 && errno == EINTR);
  return result;
}

ssize_t Write(int fd, const void* bytes, size_t byte_count) {
  if (g_providers.write != nullptr && !IsHostFd(fd)) {
    const intptr_t result = g_providers.write(fd, bytes, byte_count);
    if (result == -1) PublishAndroidErrno();
    return static_cast<ssize_t>(result);
  }
  ssize_t result;
  do {
    result = write(fd, bytes, byte_count);
  } while (result == -1 && errno == EINTR);
  return result;
}

ssize_t Pread(int fd, void* bytes, size_t byte_count, int64_t offset) {
  if (g_providers.pread != nullptr && !IsHostFd(fd)) {
    const intptr_t result = g_providers.pread(fd, bytes, byte_count, offset);
    if (result == -1) PublishAndroidErrno();
    return static_cast<ssize_t>(result);
  }
  ssize_t result;
  do {
    result = pread(fd, bytes, byte_count, static_cast<off_t>(offset));
  } while (result == -1 && errno == EINTR);
  return result;
}

ssize_t Pwrite(int fd, const void* bytes, size_t byte_count, int64_t offset) {
  if (g_providers.pwrite != nullptr && !IsHostFd(fd)) {
    const intptr_t result = g_providers.pwrite(fd, bytes, byte_count, offset);
    if (result == -1) PublishAndroidErrno();
    return static_cast<ssize_t>(result);
  }
  ssize_t result;
  do {
    result = pwrite(fd, bytes, byte_count, static_cast<off_t>(offset));
  } while (result == -1 && errno == EINTR);
  return result;
}

int Fstat(int fd, struct stat* status) {
  if (g_providers.fstat != nullptr && !IsHostFd(fd)) {
    DarwinArtAndroidStat android_status{};
    const int result = g_providers.fstat(fd, &android_status);
    if (result == -1) {
      PublishAndroidErrno();
      return -1;
    }
    AndroidStatToDarwin(android_status, status);
    return 0;
  }
  int result;
  do {
    result = fstat(fd, status);
  } while (result == -1 && errno == EINTR);
  return result;
}

int Ftruncate(int fd, int64_t length) {
  if (g_providers.ftruncate != nullptr && !IsHostFd(fd)) {
    const int result = g_providers.ftruncate(fd, length);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  int result;
  do {
    result = ftruncate(fd, static_cast<off_t>(length));
  } while (result == -1 && errno == EINTR);
  return result;
}

int Stat(const char* path, struct stat* status) {
  const char* effective_path = path;
  const bool authorized = IsAuthorizedHostRuntimePath(effective_path);
  if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr &&
      path != nullptr && std::strstr(path, "app_resources_lib") != nullptr) {
    std::fprintf(stderr, "DARWIN libcore stat path=%s authorized=%d\n", path,
                 authorized ? 1 : 0);
  }
  if (g_providers.stat != nullptr && !authorized) {
    DarwinArtAndroidStat android_status{};
    const int result = g_providers.stat(path, &android_status);
    if (result == -1) {
      PublishAndroidErrno();
      return -1;
    }
    AndroidStatToDarwin(android_status, status);
    return 0;
  }
  int result;
  do {
    result = stat(effective_path, status);
  } while (result == -1 && errno == EINTR);
  return result;
}

int64_t Lseek(int fd, int64_t offset, int android_whence) {
  if (g_providers.lseek != nullptr && !IsHostFd(fd)) {
    const int64_t result = g_providers.lseek(fd, offset, android_whence);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  int darwin_whence = 0;
  if (!DarwinWhenceFromAndroid(android_whence, &darwin_whence)) {
    errno = EINVAL;
    return -1;
  }
  const off_t darwin_offset = static_cast<off_t>(offset);
  if (static_cast<int64_t>(darwin_offset) != offset) {
    errno = EOVERFLOW;
    return -1;
  }
  off_t result;
  do {
    result = lseek(fd, darwin_offset, darwin_whence);
  } while (result == static_cast<off_t>(-1) && errno == EINTR);
  if (result != static_cast<off_t>(-1) &&
      static_cast<off_t>(static_cast<int64_t>(result)) != result) {
    errno = EOVERFLOW;
    return -1;
  }
  return static_cast<int64_t>(result);
}

intptr_t Sendfile(int output_fd, int input_fd, int64_t* offset,
                  size_t byte_count) {
  if (g_providers.sendfile == nullptr || IsHostFd(output_fd) ||
      IsHostFd(input_fd)) {
    errno = ENOTSUP;
    return -1;
  }
  const intptr_t result =
      g_providers.sendfile(output_fd, input_fd, offset, byte_count);
  if (result == -1) PublishAndroidErrno();
  return result;
}

int Access(const char* path, int mode) {
  if ((mode & ~7) != 0) {
    errno = EINVAL;
    return -1;
  }
  const bool authorized = IsAuthorizedHostRuntimePath(path);
  if (std::getenv("DARWIN_ART_DEBUG_LIBCORE_IO") != nullptr &&
      path != nullptr && std::strstr(path, "app_resources_lib") != nullptr) {
    std::fprintf(stderr, "DARWIN libcore access path=%s mode=%d authorized=%d\n",
                 path, mode, authorized ? 1 : 0);
  }
  if (g_providers.access != nullptr && !authorized) {
    const int result = g_providers.access(path, mode);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  return access(path, mode);
}

int Remove(const char* path) {
  if (g_providers.remove != nullptr && !IsAuthorizedHostRuntimePath(path)) {
    const int result = g_providers.remove(path);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  return remove(path);
}

int Rename(const char* old_path, const char* new_path) {
  const bool guest_paths = !IsAuthorizedHostRuntimePath(old_path) &&
                           !IsAuthorizedHostRuntimePath(new_path);
  if (g_providers.rename != nullptr && guest_paths) {
    const int result = g_providers.rename(old_path, new_path);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  if (!guest_paths && IsAuthorizedHostRuntimePath(old_path) !=
                          IsAuthorizedHostRuntimePath(new_path)) {
    errno = EXDEV;
    return -1;
  }
  return rename(old_path, new_path);
}

namespace {
// Mappings made through the Bionic VM owner, which must also unmap them.
std::mutex g_provider_mappings_mutex;
std::map<uintptr_t, size_t> g_provider_mappings;
}  // namespace

void* Mmap(void* address, size_t byte_count, int linux_prot,
           int linux_flags, int fd, off_t offset) {
  // A descriptor that is not a host fd is a guest descriptor (for example a
  // central-broker token for a file opened through the guest filesystem).
  // Only the Bionic VM owner can resolve and map it; it takes Linux flags.
  if (fd >= 0 && g_providers.mmap != nullptr && fcntl(fd, F_GETFD) == -1 &&
      errno == EBADF) {
    // Linux MAP_POPULATE only prefaults; apply it as the host path does.
    constexpr int kLinuxMapPopulate = 0x8000;
    void* mapped = g_providers.mmap(address, byte_count, linux_prot,
                                    linux_flags & ~kLinuxMapPopulate, fd,
                                    static_cast<int64_t>(offset));
    if (mapped == MAP_FAILED) {
      PublishAndroidErrno();
      return MAP_FAILED;
    }
    if ((linux_flags & kLinuxMapPopulate) != 0) {
      (void)madvise(mapped, byte_count, MADV_WILLNEED);
    }
    std::lock_guard<std::mutex> lock(g_provider_mappings_mutex);
    g_provider_mappings[reinterpret_cast<uintptr_t>(mapped)] = byte_count;
    return mapped;
  }
  const int flags = TranslateMmapFlags(linux_flags);
  if (flags == -1) {
    return MAP_FAILED;
  }
  void* result = mmap(address, byte_count, linux_prot, flags, fd, offset);
  if (result != MAP_FAILED && (linux_flags & 0x8000) != 0) {
    const int saved_errno = errno;
    (void)madvise(result, byte_count, MADV_WILLNEED);
    errno = saved_errno;
  }
  return result;
}

int Munmap(void* address, size_t byte_count) {
  bool provider_owned = false;
  {
    std::lock_guard<std::mutex> lock(g_provider_mappings_mutex);
    auto found = g_provider_mappings.find(reinterpret_cast<uintptr_t>(address));
    if (found != g_provider_mappings.end() && found->second == byte_count) {
      g_provider_mappings.erase(found);
      provider_owned = true;
    }
  }
  if (provider_owned && g_providers.munmap != nullptr) {
    const int result = g_providers.munmap(address, byte_count);
    if (result == -1) PublishAndroidErrno();
    return result;
  }
  return munmap(address, byte_count);
}

long Sysconf(int name) {
  int darwin_name = 0;
  if (!os_constants::DarwinSysconfNameFromAndroid(name, &darwin_name)) {
    errno = EINVAL;
    return -1;
  }
  return sysconf(darwin_name);
}

}  // namespace darwin_art::libcore_darwin
