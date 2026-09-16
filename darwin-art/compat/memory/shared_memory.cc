// Darwin backing for Android shared-memory descriptors. No framework policy.
#include "../darwin_android_platform.h"
#include "shared_memory.h"
#include <cerrno>
#include <climits>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <limits>
#include <mutex>
#include <pthread.h>
#include <sys/mman.h>
#include <sys/xattr.h>
#include <unistd.h>
#include <unordered_map>

extern "C" int darwin_art_bionic_errno_set_from_darwin(int error);
using DarwinArtSharedMemoryIoctl = int (*)(int, uint32_t, void*, int*, int*);
extern "C" int darwin_art_bionic_ioctl_bind_shared_memory(DarwinArtSharedMemoryIoctl);
extern "C" int darwin_art_android_shared_memory_ioctl(int, uint32_t, void*, int*, int*);

namespace {
struct SharedMemoryState {
  size_t size;
  int protection;
};

struct SharedMemoryMarker {
  uint64_t magic;
  uint64_t size;
  int32_t protection;
  uint32_t reserved;
};

constexpr uint64_t kSharedMemoryMarkerMagic = UINT64_C(0x444152544153484d);
constexpr char kSharedMemoryMarkerName[] = "com.darwinart.ashmem";

bool WriteSharedMemoryMarker(int fd, const SharedMemoryState& state) {
  const SharedMemoryMarker marker{kSharedMemoryMarkerMagic,
                                  static_cast<uint64_t>(state.size),
                                  state.protection, 0};
  return fsetxattr(fd, kSharedMemoryMarkerName, &marker, sizeof(marker), 0, 0) ==
         0;
}

bool ReadSharedMemoryMarker(int fd, SharedMemoryState* state) {
  if (state == nullptr) return false;
  SharedMemoryMarker marker{};
  const ssize_t size =
      fgetxattr(fd, kSharedMemoryMarkerName, &marker, sizeof(marker), 0, 0);
  if (size != static_cast<ssize_t>(sizeof(marker)) ||
      marker.magic != kSharedMemoryMarkerMagic || marker.size == 0 ||
      marker.size > std::numeric_limits<size_t>::max()) {
    return false;
  }
  constexpr int kAndroidProtectionMask = 0x1 | 0x2 | 0x4;
  if ((marker.protection & ~kAndroidProtectionMask) != 0) return false;
  *state = SharedMemoryState{static_cast<size_t>(marker.size),
                             marker.protection};
  return true;
}

std::mutex g_shared_memory_mutex;
std::unordered_map<int, SharedMemoryState> g_shared_memory;
uint64_t DebugThreadId() {
  uint64_t thread_id = 0;
  (void)pthread_threadid_np(nullptr, &thread_id);
  return thread_id;
}
}  // namespace

extern "C" int ASharedMemory_create(const char*, size_t size) {
  if (darwin_art_bionic_ioctl_bind_shared_memory(
          &darwin_art_android_shared_memory_ioctl) != 0) {
    darwin_art_bionic_errno_set_from_darwin(EIO);
    return -1;
  }
  if (size == 0) {
    darwin_art_bionic_errno_set_from_darwin(EINVAL);
    return -1;
  }
  char path[] = "/tmp/darwin-art-ashmem.XXXXXX";
  const int fd = mkstemp(path);
  if (fd < 0) {
    darwin_art_bionic_errno_set_from_darwin(errno);
    return -1;
  }
  // Android ashmem objects are anonymous file descriptors: unlinking the
  // Darwin backing file immediately gives the descriptor the same lifetime.
  (void)unlink(path);
  if (ftruncate(fd, static_cast<off_t>(size)) != 0) {
    const int error = errno;
    (void)close(fd);
    darwin_art_bionic_errno_set_from_darwin(error);
    return -1;
  }
  const SharedMemoryState state{size, PROT_READ | PROT_WRITE};
  if (!WriteSharedMemoryMarker(fd, state)) {
    const int error = errno;
    (void)close(fd);
    darwin_art_bionic_errno_set_from_darwin(error);
    return -1;
  }
  try {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.insert_or_assign(fd, state);
  } catch (...) {
    (void)close(fd);
    darwin_art_bionic_errno_set_from_darwin(ENOMEM);
    return -1;
  }
  if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
    std::fprintf(stderr,
                 "ART Android ashmem: pid=%d tid=%llu create fd=%d size=%zu\n",
                 getpid(), static_cast<unsigned long long>(DebugThreadId()), fd,
                 size);
  return fd;
}

extern "C" int ASharedMemory_setProt(int fd, int protection) {
  constexpr int kAndroidProtectionMask = 0x1 | 0x2 | 0x4;
  if (fd < 0 || (protection & ~kAndroidProtectionMask) != 0) {
    darwin_art_bionic_errno_set_from_darwin(EINVAL);
    return -1;
  }
  SharedMemoryState state{};
  if (!ReadSharedMemoryMarker(fd, &state)) {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.erase(fd);
    if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
      std::fprintf(stderr, "ART Android ashmem: setProt missing fd=%d\n", fd);
    darwin_art_bionic_errno_set_from_darwin(EBADF);
    return -1;
  }
  // Ashmem protection can only be reduced after publication.
  if ((protection | state.protection) != state.protection) {
    darwin_art_bionic_errno_set_from_darwin(EINVAL);
    return -1;
  }
  state.protection = protection;
  if (!WriteSharedMemoryMarker(fd, state)) {
    darwin_art_bionic_errno_set_from_darwin(errno);
    return -1;
  }
  {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.insert_or_assign(fd, state);
  }
  // Darwin has no ashmem-wide future-mapping protection seal. Individual
  // mappings still receive the requested protection through mmap/mprotect.
  return 0;
}

extern "C" int darwin_art_android_shared_memory_close(int fd) {
  SharedMemoryState state{};
  const bool is_shared_memory = ReadSharedMemoryMarker(fd, &state);
  {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.erase(fd);
  }
  if (!is_shared_memory) {
    if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
      std::fprintf(stderr,
                   "ART Android ashmem: pid=%d tid=%llu close miss fd=%d\n",
                   getpid(), static_cast<unsigned long long>(DebugThreadId()),
                   fd);
    return 0;
  }
  const int result = close(fd) == 0 ? 1 : -1;
  if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
    std::fprintf(stderr,
                 "ART Android ashmem: pid=%d tid=%llu close fd=%d result=%d\n",
                 getpid(), static_cast<unsigned long long>(DebugThreadId()), fd,
                 result);
  return result;
}

extern "C" int darwin_art_android_shared_memory_dup(int fd) {
  SharedMemoryState state{};
  if (!ReadSharedMemoryMarker(fd, &state)) {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.erase(fd);
    return -2;
  }
  const int duplicate = dup(fd);
  if (duplicate < 0) return -1;
  try {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.insert_or_assign(duplicate, state);
  } catch (...) {
    (void)close(duplicate);
    return -1;
  }
  if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
    std::fprintf(stderr,
                 "ART Android ashmem: pid=%d tid=%llu dup fd=%d new=%d\n",
                 getpid(), static_cast<unsigned long long>(DebugThreadId()), fd,
                 duplicate);
  return duplicate;
}

extern "C" int darwin_art_android_shared_memory_get_info(
    int fd, size_t* size, int* protection) {
  if (size == nullptr || protection == nullptr) return -1;
  SharedMemoryState marked{};
  if (ReadSharedMemoryMarker(fd, &marked)) {
    try {
      std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
      g_shared_memory.insert_or_assign(fd, marked);
    } catch (...) {
      return -1;
    }
    *size = marked.size;
    *protection = marked.protection;
    return 1;
  }
  std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
  g_shared_memory.erase(fd);
  return 0;
}

extern "C" int darwin_art_android_shared_memory_adopt(
    int fd, size_t size, int protection) {
  constexpr int kAndroidProtectionMask = 0x1 | 0x2 | 0x4;
  if (fd < 0 || size == 0 || (protection & ~kAndroidProtectionMask) != 0) {
    return -1;
  }
  try {
    const SharedMemoryState state{size, protection};
    if (!WriteSharedMemoryMarker(fd, state)) return -1;
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.insert_or_assign(fd, state);
    return 0;
  } catch (...) {
    return -1;
  }
}

extern "C" int darwin_art_android_shared_memory_fcntl(
    int fd, int command, intptr_t argument, int* result) {
  if (result == nullptr) return 0;
  SharedMemoryState state{};
  if (!ReadSharedMemoryMarker(fd, &state)) {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.erase(fd);
    return 0;
  }
  std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
  g_shared_memory.insert_or_assign(fd, state);
  auto found = g_shared_memory.find(fd);
  if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
    std::fprintf(stderr,
                 "ART Android ashmem: pid=%d tid=%llu fcntl fd=%d command=%d "
                 "argument=%lld\n",
                 getpid(), static_cast<unsigned long long>(DebugThreadId()), fd,
                 command, static_cast<long long>(argument));
  constexpr int kAndroidFDupfd = 0;
  constexpr int kAndroidFGetfd = 1;
  constexpr int kAndroidFSetfd = 2;
  constexpr int kAndroidFGetfl = 3;
  constexpr int kAndroidFSetfl = 4;
  constexpr int kAndroidFAddSeals = 1033;
  constexpr int kAndroidFGetSeals = 1034;
  constexpr int kAndroidFDupfdCloexec = 1030;
  constexpr int kSealShrink = 0x2;
  constexpr int kSealGrow = 0x4;
  constexpr int kSealFutureWrite = 0x10;
  if (command == kAndroidFGetSeals) {
    *result = kSealShrink | kSealGrow |
              ((found->second.protection & PROT_WRITE) == 0
                   ? kSealFutureWrite
                   : 0);
    if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
      std::fprintf(stderr,
                   "ART Android ashmem: fcntl fd=%d F_GET_SEALS result=%#x "
                   "protection=%#x\n",
                   fd, *result, found->second.protection);
    return 1;
  }
  if (command == kAndroidFAddSeals) {
    if ((argument & kSealFutureWrite) != 0) {
      found->second.protection &= ~PROT_WRITE;
      if (!WriteSharedMemoryMarker(fd, found->second)) {
        *result = -1;
        return 1;
      }
    }
    *result = 0;
    if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
      std::fprintf(stderr,
                   "ART Android ashmem: fcntl fd=%d F_ADD_SEALS result=0 "
                   "protection=%#x\n",
                   fd, found->second.protection);
    return 1;
  }
  int host_command = -1;
  switch (command) {
    case kAndroidFDupfd: host_command = F_DUPFD; break;
    case kAndroidFDupfdCloexec: host_command = F_DUPFD_CLOEXEC; break;
    case kAndroidFGetfd: host_command = F_GETFD; break;
    case kAndroidFSetfd: host_command = F_SETFD; break;
    case kAndroidFGetfl: host_command = F_GETFL; break;
    case kAndroidFSetfl: host_command = F_SETFL; break;
    default:
      *result = -1;
      return 1;
  }
  *result = (command == kAndroidFGetfd || command == kAndroidFGetfl)
                ? fcntl(fd, host_command)
                : fcntl(fd, host_command, argument);
  if (*result >= 0 &&
      (command == kAndroidFDupfd || command == kAndroidFDupfdCloexec)) {
    g_shared_memory.emplace(*result, found->second);
  }
  return 1;
}

extern "C" int darwin_art_android_shared_memory_ioctl(
    int fd, uint32_t request, void*, int* result, int* android_errno) {
  if (result == nullptr || android_errno == nullptr) return 0;
  SharedMemoryState state{};
  if (!ReadSharedMemoryMarker(fd, &state)) {
    std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
    g_shared_memory.erase(fd);
    if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
      std::fprintf(stderr, "ART Android ashmem: ioctl miss fd=%d request=%#x\n",
                   fd, request);
    return 0;
  }
  std::lock_guard<std::mutex> lock(g_shared_memory_mutex);
  g_shared_memory.insert_or_assign(fd, state);
  const auto found = g_shared_memory.find(fd);
  if (std::getenv("DARWIN_ART_DEBUG_SHARED_MEMORY") != nullptr)
    std::fprintf(stderr, "ART Android ashmem: ioctl fd=%d request=%#x\n", fd,
                 request);
  constexpr uint32_t kAshmemGetSize = 0x00007704;
  constexpr uint32_t kAshmemGetProtectionMask = 0x00007706;
  if (request == kAshmemGetSize) {
    *result = found->second.size > static_cast<size_t>(INT_MAX)
                  ? -1
                  : static_cast<int>(found->second.size);
    *android_errno = *result < 0 ? EOVERFLOW : 0;
    return 1;
  }
  if (request == kAshmemGetProtectionMask) {
    *result = found->second.protection;
    *android_errno = 0;
    return 1;
  }
  *result = -1;
  *android_errno = ENOTTY;
  return 1;
}
