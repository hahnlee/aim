#include "aim_elf_loader.h"

#include <fcntl.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <limits>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace {

constexpr uint32_t kEocd = 0x06054b50;
constexpr uint32_t kCentral = 0x02014b50;
constexpr uint32_t kLocal = 0x04034b50;
constexpr size_t kPageSize = 16 * 1024;
constexpr size_t kMaxArchive = 512 * 1024 * 1024;
constexpr size_t kMaxEntries = 4096;
constexpr size_t kMaxNativeFiles = 64;
constexpr size_t kMaxNativeFile = 64 * 1024 * 1024;
constexpr size_t kMaxNativeTotal = 256 * 1024 * 1024;
constexpr char kPrefix[] = "lib/arm64-v8a/";

struct ErrorStorage {
  char bytes[512] = {};
  AimElfErrorBuffer value{bytes, sizeof(bytes), 0};
};

struct Identity {
  dev_t device;
  ino_t inode;
  off_t size;
  timespec modified;
  timespec changed;
  mode_t mode;
};

struct Entry {
  std::string name;
  std::string leaf;
  uint16_t flags;
  uint16_t method;
  uint32_t crc;
  size_t compressed_size;
  size_t uncompressed_size;
  size_t local_offset;
  size_t data_offset;
  mode_t unix_mode;
  bool native;
};

class Mapping {
 public:
  ~Mapping() {
    if (bytes_ != MAP_FAILED) munmap(bytes_, size_);
    if (fd_ >= 0) close(fd_);
  }
  Mapping(const Mapping&) = delete;
  Mapping& operator=(const Mapping&) = delete;
  Mapping() = default;

  bool Open(const char* path, std::string* error) {
    fd_ = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd_ < 0) return FailErrno("open APK", error);
    if (flock(fd_, LOCK_SH | LOCK_NB) != 0) return FailErrno("lock APK", error);
    struct stat info {};
    if (fstat(fd_, &info) != 0) return FailErrno("fstat APK", error);
    if (!S_ISREG(info.st_mode) || (info.st_mode & 0222) != 0 || info.st_size <= 0 ||
        static_cast<uint64_t>(info.st_size) > kMaxArchive) {
      *error = "APK must be a nonempty, non-writable regular file within the size cap";
      return false;
    }
    size_ = static_cast<size_t>(info.st_size);
    identity_ = FromStat(info);
    bytes_ = mmap(nullptr, size_, PROT_READ, MAP_PRIVATE, fd_, 0);
    if (bytes_ == MAP_FAILED) return FailErrno("mmap APK", error);
    return true;
  }

  bool Unchanged(std::string* error) const {
    struct stat info {};
    if (fstat(fd_, &info) != 0) return FailErrno("re-fstat APK", error);
    const Identity current = FromStat(info);
    if (current.device != identity_.device || current.inode != identity_.inode ||
        current.size != identity_.size || current.modified.tv_sec != identity_.modified.tv_sec ||
        current.modified.tv_nsec != identity_.modified.tv_nsec ||
        current.changed.tv_sec != identity_.changed.tv_sec ||
        current.changed.tv_nsec != identity_.changed.tv_nsec ||
        current.mode != identity_.mode || (current.mode & 0222) != 0) {
      *error = "mutable APK changed after validation";
      return false;
    }
    return true;
  }

  const uint8_t* data() const { return static_cast<const uint8_t*>(bytes_); }
  size_t size() const { return size_; }

 private:
  static Identity FromStat(const struct stat& info) {
    return Identity{info.st_dev, info.st_ino, info.st_size, info.st_mtimespec,
                    info.st_ctimespec, info.st_mode};
  }
  static bool FailErrno(const char* operation, std::string* error) {
    *error = std::string(operation) + ": " + std::strerror(errno);
    return false;
  }
  int fd_ = -1;
  void* bytes_ = MAP_FAILED;
  size_t size_ = 0;
  Identity identity_{};
};

bool Add(size_t left, size_t right, size_t* output) {
  if (right > std::numeric_limits<size_t>::max() - left) return false;
  *output = left + right;
  return true;
}

bool U16(const uint8_t* bytes, size_t size, size_t offset, uint16_t* output) {
  if (offset > size || size - offset < 2) return false;
  *output = static_cast<uint16_t>(bytes[offset]) |
            static_cast<uint16_t>(bytes[offset + 1]) << 8;
  return true;
}

bool U32(const uint8_t* bytes, size_t size, size_t offset, uint32_t* output) {
  if (offset > size || size - offset < 4) return false;
  *output = static_cast<uint32_t>(bytes[offset]) |
            static_cast<uint32_t>(bytes[offset + 1]) << 8 |
            static_cast<uint32_t>(bytes[offset + 2]) << 16 |
            static_cast<uint32_t>(bytes[offset + 3]) << 24;
  return true;
}

uint32_t Crc32(const uint8_t* bytes, size_t size) {
  uint32_t crc = ~uint32_t{0};
  for (size_t index = 0; index < size; ++index) {
    crc ^= bytes[index];
    for (int bit = 0; bit < 8; ++bit) {
      crc = (crc >> 1) ^ (0xedb88320u & (0u - (crc & 1u)));
    }
  }
  return ~crc;
}

bool SafeName(const std::string& name) {
  if (name.empty() || name.front() == '/' || name.find('\0') != std::string::npos ||
      name.find('\\') != std::string::npos) return false;
  size_t start = 0;
  while (start < name.size()) {
    const size_t slash = name.find('/', start);
    const size_t end = slash == std::string::npos ? name.size() : slash;
    const std::string component = name.substr(start, end - start);
    if (component.empty() || component == "." || component == "..") {
      return end == name.size() && component.empty() && name.back() == '/';
    }
    if (slash == std::string::npos) break;
    start = slash + 1;
  }
  return true;
}

bool Parse(const Mapping& mapping, std::vector<Entry>* entries,
           size_t* mutation_offset, std::string* error) {
  const uint8_t* bytes = mapping.data();
  const size_t size = mapping.size();
  if (size < 22) { *error = "archive is shorter than EOCD"; return false; }
  const size_t floor = size > 22 + 65535 ? size - 22 - 65535 : 0;
  size_t eocd = size;
  for (size_t candidate = size - 22;; --candidate) {
    uint32_t signature;
    uint16_t comment;
    if (U32(bytes, size, candidate, &signature) && signature == kEocd &&
        U16(bytes, size, candidate + 20, &comment) &&
        candidate + 22 + comment == size) {
      eocd = candidate;
      break;
    }
    if (candidate == floor) break;
  }
  if (eocd == size) { *error = "terminal EOCD not found"; return false; }
  uint16_t disk, central_disk, disk_count, count;
  uint32_t central_size32, central_offset32;
  if (!U16(bytes, size, eocd + 4, &disk) ||
      !U16(bytes, size, eocd + 6, &central_disk) ||
      !U16(bytes, size, eocd + 8, &disk_count) ||
      !U16(bytes, size, eocd + 10, &count) ||
      !U32(bytes, size, eocd + 12, &central_size32) ||
      !U32(bytes, size, eocd + 16, &central_offset32)) {
    *error = "truncated EOCD"; return false;
  }
  if (disk != 0 || central_disk != 0 || disk_count != count) {
    *error = "multi-disk ZIP is forbidden"; return false;
  }
  if (count == 0xffff || central_size32 == 0xffffffffu ||
      central_offset32 == 0xffffffffu) {
    *error = "ZIP64 is forbidden"; return false;
  }
  if (count > kMaxEntries) { *error = "entry count cap exceeded"; return false; }
  const size_t central_offset = central_offset32;
  size_t central_end;
  if (!Add(central_offset, central_size32, &central_end) || central_end != eocd) {
    *error = "central directory bounds mismatch"; return false;
  }

  std::unordered_set<std::string> names;
  std::vector<std::pair<size_t, size_t>> ranges;
  size_t cursor = central_offset;
  size_t native_total = 0;
  for (size_t index = 0; index < count; ++index) {
    uint32_t signature, crc, compressed32, uncompressed32, external, local32;
    uint16_t made_by, flags, method, name_length, extra_length, comment_length, start_disk;
    if (!U32(bytes, size, cursor, &signature) || signature != kCentral ||
        !U16(bytes, size, cursor + 4, &made_by) ||
        !U16(bytes, size, cursor + 8, &flags) ||
        !U16(bytes, size, cursor + 10, &method) ||
        !U32(bytes, size, cursor + 16, &crc) ||
        !U32(bytes, size, cursor + 20, &compressed32) ||
        !U32(bytes, size, cursor + 24, &uncompressed32) ||
        !U16(bytes, size, cursor + 28, &name_length) ||
        !U16(bytes, size, cursor + 30, &extra_length) ||
        !U16(bytes, size, cursor + 32, &comment_length) ||
        !U16(bytes, size, cursor + 34, &start_disk) ||
        !U32(bytes, size, cursor + 38, &external) ||
        !U32(bytes, size, cursor + 42, &local32)) {
      *error = "truncated central record"; return false;
    }
    if (compressed32 == 0xffffffffu || uncompressed32 == 0xffffffffu ||
        local32 == 0xffffffffu) { *error = "ZIP64 entry is forbidden"; return false; }
    if (start_disk != 0) { *error = "multi-disk entry is forbidden"; return false; }
    if ((flags & 0x2049) != 0) {
      *error = (flags & 0x0008) != 0 ? "data descriptor is forbidden"
                                     : "encrypted or masked entry is forbidden";
      return false;
    }
    size_t name_start, name_end, record_end;
    if (!Add(cursor, 46, &name_start) || !Add(name_start, name_length, &name_end) ||
        !Add(name_end, extra_length, &record_end) ||
        !Add(record_end, comment_length, &record_end) || record_end > central_end) {
      *error = "central record exceeds bounds"; return false;
    }
    std::string name(reinterpret_cast<const char*>(bytes + name_start), name_length);
    if (!SafeName(name)) { *error = "unsafe ZIP path"; return false; }
    if (!names.insert(name).second) { *error = "duplicate ZIP entry"; return false; }
    const mode_t unix_mode = made_by >> 8 == 3 ? external >> 16 : 0;
    if ((unix_mode & S_IFMT) == S_IFLNK) { *error = "ZIP symlink is forbidden"; return false; }

    uint32_t local_signature, local_crc, local_compressed, local_uncompressed;
    uint16_t local_flags, local_method, local_name_length, local_extra_length;
    const size_t local = local32;
    if (!U32(bytes, size, local, &local_signature) || local_signature != kLocal ||
        !U16(bytes, size, local + 6, &local_flags) ||
        !U16(bytes, size, local + 8, &local_method) ||
        !U32(bytes, size, local + 14, &local_crc) ||
        !U32(bytes, size, local + 18, &local_compressed) ||
        !U32(bytes, size, local + 22, &local_uncompressed) ||
        !U16(bytes, size, local + 26, &local_name_length) ||
        !U16(bytes, size, local + 28, &local_extra_length)) {
      *error = "truncated local header"; return false;
    }
    size_t local_name_start, local_name_end, data_offset, data_end;
    if (!Add(local, 30, &local_name_start) ||
        !Add(local_name_start, local_name_length, &local_name_end) ||
        !Add(local_name_end, local_extra_length, &data_offset) ||
        !Add(data_offset, compressed32, &data_end) || data_end > central_offset) {
      *error = "local data exceeds bounds"; return false;
    }
    if (local_flags != flags || local_method != method || local_crc != crc ||
        local_compressed != compressed32 || local_uncompressed != uncompressed32 ||
        local_name_length != name_length ||
        std::memcmp(bytes + local_name_start, name.data(), name_length) != 0) {
      *error = "central/local metadata mismatch"; return false;
    }
    ranges.emplace_back(local, data_end);

    const bool under_abi = name.rfind(kPrefix, 0) == 0;
    const std::string leaf = under_abi ? name.substr(sizeof(kPrefix) - 1) : std::string();
    const bool native = under_abi && !leaf.empty();
    if (native) {
      if (leaf.find('/') != std::string::npos || leaf.size() < 3 ||
          leaf.substr(leaf.size() - 3) != ".so") {
        *error = "arm64 native entry is not a direct .so child"; return false;
      }
      if ((unix_mode & S_IFMT) != 0 && (unix_mode & S_IFMT) != S_IFREG) {
        *error = "arm64 native entry is not a regular file"; return false;
      }
      if (method != 0) { *error = "native entry is not STORED"; return false; }
      if (compressed32 == 0 || compressed32 != uncompressed32 ||
          compressed32 > kMaxNativeFile) {
        *error = "native entry size is invalid"; return false;
      }
      if (data_offset % kPageSize != 0) {
        *error = "native entry data offset is not 16 KiB aligned"; return false;
      }
      native_total += compressed32;
      if (native_total > kMaxNativeTotal ||
          std::count_if(entries->begin(), entries->end(),
                        [](const Entry& item) { return item.native; }) >=
              static_cast<ptrdiff_t>(kMaxNativeFiles)) {
        *error = "native graph cap exceeded"; return false;
      }
      if (Crc32(bytes + data_offset, compressed32) != crc) {
        *error = "native entry CRC mismatch"; return false;
      }
    }
    if (name == "assets/race-byte") *mutation_offset = data_offset;
    entries->push_back(Entry{name, leaf, flags, method, crc, compressed32,
                             uncompressed32, local, data_offset, unix_mode, native});
    cursor = record_end;
  }
  if (cursor != central_end) { *error = "central count/size mismatch"; return false; }
  std::sort(ranges.begin(), ranges.end());
  for (size_t index = 1; index < ranges.size(); ++index) {
    if (ranges[index - 1].second > ranges[index].first) {
      *error = "local entry ranges overlap"; return false;
    }
  }
  return true;
}

bool InjectMutation(const char* path, size_t offset, std::string* error) {
  if (offset == std::numeric_limits<size_t>::max()) {
    *error = "race fixture lacks mutation byte"; return false;
  }
  if (chmod(path, 0600) != 0) { *error = "race chmod failed"; return false; }
  const int fd = open(path, O_WRONLY | O_CLOEXEC);
  uint8_t value = 0xa5;
  const bool wrote = fd >= 0 && pwrite(fd, &value, 1, static_cast<off_t>(offset)) == 1;
  if (fd >= 0) close(fd);
  const bool sealed = chmod(path, 0400) == 0;
  if (!wrote || !sealed) { *error = "race mutation failed"; return false; }
  return true;
}

int Fail(const std::string& error) {
  std::fprintf(stderr, "apk-native-direct-load: %s\n", error.c_str());
  return 2;
}

}  // namespace

int main(int argc, char** argv) {
  const bool inject = argc == 4 && std::strcmp(argv[3], "--inject-mutation") == 0;
  if (argc != 3 && !inject) {
    std::fprintf(stderr, "usage: apk-native-direct-load APK ROOT [--inject-mutation]\n");
    return 64;
  }
  std::string error;
  Mapping mapping;
  if (!mapping.Open(argv[1], &error)) return Fail(error);
  std::vector<Entry> entries;
  size_t mutation_offset = std::numeric_limits<size_t>::max();
  if (!Parse(mapping, &entries, &mutation_offset, &error)) return Fail(error);

  std::unordered_map<std::string, const Entry*> native;
  for (const Entry& entry : entries) if (entry.native) native.emplace(entry.leaf, &entry);
  if (native.find(argv[2]) == native.end()) return Fail("root entry is absent");

  std::deque<std::string> queue{argv[2]};
  std::unordered_set<std::string> queued{argv[2]};
  std::vector<std::string> graph_names;
  while (!queue.empty()) {
    std::string name = std::move(queue.front());
    queue.pop_front();
    const Entry& entry = *native.at(name);
    ErrorStorage ffi_error;
    AimElfInspection* inspection = nullptr;
    AimElfStatus status = aim_elf_inspect_bytes(
        mapping.data() + entry.data_offset, entry.uncompressed_size, &inspection,
        &ffi_error.value);
    if (status != AIM_ELF_OK || inspection == nullptr) {
      return Fail(std::string("ELF inspection failed: ") + ffi_error.bytes);
    }
    const char* soname = nullptr;
    size_t needed_count = 0;
    status = aim_elf_inspection_soname(inspection, &soname, &ffi_error.value);
    if (status != AIM_ELF_OK || soname == nullptr || name != soname) {
      aim_elf_inspection_destroy(&inspection);
      return Fail("entry leaf and DT_SONAME differ");
    }
    status = aim_elf_inspection_needed_count(inspection, &needed_count,
                                                    &ffi_error.value);
    if (status != AIM_ELF_OK) {
      aim_elf_inspection_destroy(&inspection);
      return Fail("could not inspect DT_NEEDED count");
    }
    for (size_t index = 0; index < needed_count; ++index) {
      const char* needed = nullptr;
      status = aim_elf_inspection_needed_at(inspection, index, &needed,
                                                   &ffi_error.value);
      if (status != AIM_ELF_OK || needed == nullptr || native.find(needed) == native.end()) {
        aim_elf_inspection_destroy(&inspection);
        return Fail("DT_NEEDED escaped the APK sibling namespace");
      }
      if (queued.insert(needed).second) queue.emplace_back(needed);
    }
    aim_elf_inspection_destroy(&inspection);
    graph_names.push_back(std::move(name));
  }

  if (inject && !InjectMutation(argv[1], mutation_offset, &error)) return Fail(error);
  if (!mapping.Unchanged(&error)) return Fail(error);
  std::vector<AimElfGraphSource> sources;
  sources.reserve(graph_names.size());
  for (const std::string& name : graph_names) {
    const Entry& entry = *native.at(name);
    sources.push_back(AimElfGraphSource{
        name.c_str(), mapping.data() + entry.data_offset, entry.uncompressed_size});
  }
  ErrorStorage ffi_error;
  AimElfGraphHandle* graph = nullptr;
  AimElfStatus status = aim_elf_graph_load(
      argv[2], sources.data(), sources.size(), nullptr, 0, nullptr, &graph,
      &ffi_error.value);
  if (status != AIM_ELF_OK || graph == nullptr) {
    return Fail(std::string("graph load failed: ") + ffi_error.bytes);
  }
  if (!mapping.Unchanged(&error)) {
    aim_elf_graph_unload(&graph, &ffi_error.value);
    return Fail(error);
  }
  uintptr_t address = 0;
  status = aim_elf_graph_lookup_root(graph, "JNI_OnLoad", &address,
                                            &ffi_error.value);
  using OnLoad = int (*)(void*, void*);
  const int version = status == AIM_ELF_OK && address != 0
                          ? reinterpret_cast<OnLoad>(address)(
                                reinterpret_cast<void*>(uintptr_t{1}), nullptr)
                          : -1;
  if (version != 0x00010006) {
    aim_elf_graph_unload(&graph, &ffi_error.value);
    return Fail("JNI_OnLoad self-test failed");
  }
  status = aim_elf_graph_unload(&graph, &ffi_error.value);
  if (status != AIM_ELF_OK || graph != nullptr) {
    return Fail(std::string("graph unload failed: ") + ffi_error.bytes);
  }
  if (!mapping.Unchanged(&error)) return Fail(error);
  std::printf(
      "apk-native-direct-load: PASS source=readonly-mmap fd-slices=%zu copy=0 "
      "extract=0 alignment=16384 crc=verified graph=root+child+grandchild "
      "ctor=dependency-first JNI_OnLoad=0x00010006 unload=success fallback=none\n",
      sources.size());
  return 0;
}
