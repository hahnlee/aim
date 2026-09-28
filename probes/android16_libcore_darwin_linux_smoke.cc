#include "libcore_darwin_linux.h"

#include <sys/mman.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>

namespace {

uint64_t Fnv1a(const unsigned char* bytes, size_t size) {
  uint64_t hash = 1469598103934665603ULL;
  for (size_t index = 0; index < size; ++index) {
    hash ^= bytes[index];
    hash *= 1099511628211ULL;
  }
  return hash;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: %s INPUT\n", argv[0]);
    return 2;
  }
  errno = 0;
  constexpr int kAndroidScNprocessorsConf = 96;
  const long processors =
      aim::libcore_darwin::Sysconf(kAndroidScNprocessorsConf);
  if (processors <= 0) {
    std::fprintf(stderr, "sysconf(_SC_NPROCESSORS_CONF): result=%ld errno=%s\n",
                 processors, std::strerror(errno));
    return 3;
  }
  // Linux O_PATH has no faithful Darwin file-description equivalent. The
  // boundary must reject it explicitly instead of silently weakening access.
  errno = 0;
  if (aim::libcore_darwin::Open(argv[1], 010000000, 0) != -1 ||
      errno != ENOTSUP) {
    std::fprintf(stderr, "O_PATH did not fail with ENOTSUP\n");
    return 4;
  }
  const int fd = aim::libcore_darwin::Open(argv[1], 0, 0);
  if (fd == -1) {
    std::fprintf(stderr, "open: %s\n", std::strerror(errno));
    return 5;
  }
  struct stat status {};
  if (aim::libcore_darwin::Fstat(fd, &status) == -1 ||
      status.st_size <= 0) {
    std::fprintf(stderr, "fstat: %s\n", std::strerror(errno));
    return 6;
  }
  std::array<unsigned char, 64> prefix {};
  const ssize_t read_count = aim::libcore_darwin::Read(
      fd, prefix.data(), prefix.size());
  if (read_count != static_cast<ssize_t>(prefix.size())) {
    std::fprintf(stderr, "read: count=%zd errno=%s\n", read_count,
                 std::strerror(errno));
    return 7;
  }
  if (aim::libcore_darwin::Lseek(fd, 0, 0) != 0) {
    std::fprintf(stderr, "lseek(SEEK_SET): %s\n", std::strerror(errno));
    return 12;
  }
  std::array<unsigned char, 64> replay {};
  if (aim::libcore_darwin::Read(fd, replay.data(), replay.size()) !=
          static_cast<ssize_t>(replay.size()) ||
      replay != prefix) {
    std::fprintf(stderr, "lseek/read replay mismatch\n");
    return 13;
  }
  const int64_t tail_position = aim::libcore_darwin::Lseek(fd, -16, 2);
  if (tail_position != static_cast<int64_t>(status.st_size) - 16) {
    std::fprintf(stderr, "lseek(SEEK_END): result=%lld errno=%s\n",
                 static_cast<long long>(tail_position), std::strerror(errno));
    return 14;
  }
  std::array<unsigned char, 16> tail {};
  if (aim::libcore_darwin::Read(fd, tail.data(), tail.size()) !=
      static_cast<ssize_t>(tail.size())) {
    std::fprintf(stderr, "tail read: %s\n", std::strerror(errno));
    return 15;
  }
  errno = 0;
  if (aim::libcore_darwin::Lseek(fd, 0, 99) != -1 || errno != EINVAL) {
    std::fprintf(stderr, "invalid lseek whence did not fail with EINVAL\n");
    return 16;
  }
  void* mapping = aim::libcore_darwin::Mmap(
      nullptr, static_cast<size_t>(status.st_size), 1, 1, fd, 0);
  if (mapping == MAP_FAILED) {
    std::fprintf(stderr, "mmap: %s\n", std::strerror(errno));
    return 8;
  }
  const uint64_t prefix_hash = Fnv1a(prefix.data(), prefix.size());
  const uint64_t mapped_hash = Fnv1a(
      static_cast<const unsigned char*>(mapping), prefix.size());
  if (aim::libcore_darwin::Munmap(
          mapping, static_cast<size_t>(status.st_size)) == -1) {
    std::fprintf(stderr, "munmap: %s\n", std::strerror(errno));
    return 9;
  }
  void* populated = aim::libcore_darwin::Mmap(
      nullptr, static_cast<size_t>(status.st_size), 1, 1 | 0x8000, fd, 0);
  if (populated == MAP_FAILED ||
      Fnv1a(static_cast<const unsigned char*>(populated), prefix.size()) != prefix_hash) {
    std::fprintf(stderr, "MAP_POPULATE mapping failed or changed content\n");
    return 17;
  }
  if (aim::libcore_darwin::Munmap(populated, static_cast<size_t>(status.st_size)) != 0) {
    return 18;
  }
  if (aim::libcore_darwin::Close(fd) == -1) {
    std::fprintf(stderr, "close: %s\n", std::strerror(errno));
    return 10;
  }
  if (prefix_hash != mapped_hash) {
    std::fprintf(stderr, "read/mmap hash mismatch\n");
    return 11;
  }
  std::printf("libcore-darwin-linux: processors=%ld bytes=%lld prefix-hash=%016llx seek=pass\n",
              processors,
              static_cast<long long>(status.st_size),
              static_cast<unsigned long long>(prefix_hash));
  return 0;
}
