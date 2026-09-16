#include "application_memory.h"
#include "system_region.h"
#include <cerrno>
#include <fcntl.h>
#include <map>
#include <memory>
#include <mutex>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

namespace darwin_art::memory {
namespace {
struct Region {
  std::unique_ptr<SystemRegion> storage;
  dev_t device;
  ino_t inode;
  bool Matches(const struct stat& st) const {
    return device == st.st_dev && inode == st.st_ino;
  }
};
std::mutex mutex;
// create() is followed by map() in the AOSP Java owner. Mapping takes over the
// region lifetime; closeFileDescriptor() can then close the Java FD early.
std::map<std::pair<dev_t, ino_t>, std::shared_ptr<Region>> pending;
std::map<void*, std::shared_ptr<Region>> mappings;
void DiscardPending(int fd) {
  const int error = errno;
  struct stat st{};
  if (fstat(fd, &st) == 0) {
    std::lock_guard lock(mutex);
    pending.erase({st.st_dev, st.st_ino});
  }
  errno = error;
}
}

int CreateApplicationMemory(size_t size) noexcept {
  auto storage = SystemRegion::Create(size);
  if (!storage) return -1;
  int fd = storage->DuplicateWriter();
  if (fd < 0) return -1;
  struct stat st{};
  if (fstat(fd, &st) != 0) { const int error = errno; close(fd); errno = error; return -1; }
  try {
    auto region = std::make_shared<Region>(Region{std::move(storage), st.st_dev, st.st_ino});
    std::lock_guard lock(mutex);
    pending.insert_or_assign(std::make_pair(st.st_dev, st.st_ino), std::move(region));
    return fd;
  } catch (...) { close(fd); errno = ENOMEM; return -1; }
}

void AbandonApplicationMemory(int fd) noexcept {
  const int error = errno;
  struct stat st{};
  if (fstat(fd, &st) == 0) {
    std::lock_guard lock(mutex);
    for (auto entry = pending.begin(); entry != pending.end();) {
      if (entry->second->Matches(st)) entry = pending.erase(entry);
      else ++entry;
    }
  }
  errno = error;
}

void* MapApplicationMemory(int fd, size_t size, bool writable) noexcept {
  struct stat st{};
  if (fstat(fd, &st) != 0) { DiscardPending(fd); return MAP_FAILED; }
  if (size == 0 || st.st_size < 0 || static_cast<uint64_t>(st.st_size) < size) {
    errno = EINVAL;
    DiscardPending(fd);
    return MAP_FAILED;
  }
  try {
    std::lock_guard lock(mutex);
    // The Java FD table owns the original descriptor; mapping borrows a dup.
    // Resolve the backing inode, not its process-local descriptor number.
    auto waiting = pending.begin();
    while (waiting != pending.end() && !waiting->second->Matches(st)) ++waiting;
    std::shared_ptr<Region> owner;
    if (waiting != pending.end() && waiting->second->Matches(st)) owner = waiting->second;
    if (!owner) {
      for (const auto& [address, candidate] : mappings) {
        if (candidate->Matches(st)) { owner = candidate; break; }
      }
    }
    void* address = mmap(nullptr, size, writable ? PROT_READ | PROT_WRITE : PROT_READ,
                         MAP_SHARED, fd, 0);
    if (address == MAP_FAILED) {
      if (waiting != pending.end()) pending.erase(waiting);
      return MAP_FAILED;
    }
    if (owner) {
      try { mappings.emplace(address, owner); }
      catch (...) {
        munmap(address, size);
        if (waiting != pending.end()) pending.erase(waiting);
        throw;
      }
    }
    if (waiting != pending.end()) pending.erase(waiting);
    return address;
  } catch (...) { errno = ENOMEM; return MAP_FAILED; }
}

int UnmapApplicationMemory(void* address, size_t size) noexcept {
  std::lock_guard lock(mutex);
  int result = munmap(address, size);
  if (result == 0) mappings.erase(address);
  return result;
}

int DuplicateApplicationMemoryReader(int fd) noexcept {
  struct stat st{};
  if (fstat(fd, &st) != 0) return -1;
  const int flags = fcntl(fd, F_GETFL);
  if (flags < 0) return -1;
  if ((flags & O_ACCMODE) == O_RDONLY) return fcntl(fd, F_DUPFD_CLOEXEC, 0);
  std::lock_guard lock(mutex);
  for (const auto& [key, region] : pending) {
    if (region->Matches(st)) return region->storage->DuplicateReader();
  }
  for (const auto& [key, region] : mappings) {
    if (region->Matches(st)) return region->storage->DuplicateReader();
  }
  errno = ENOTSUP;  // Never pretend dup(O_RDWR) is a read-only capability.
  return -1;
}
}
